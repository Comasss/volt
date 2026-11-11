//! Provider Logitech: HID++ 2.0 sobre receptores (LIGHTSPEED, Unifying, Bolt),
//! cable USB y Bluetooth directo.
//!
//! Flujo por ciclo:
//! 1. Agrupar las colecciones HID++ (usage page 0xFF00/0xFF43) por interfaz física.
//! 2. Abrir corto+largo, hacer ping a cada ranura del receptor (o a 0xFF si es directo).
//! 3. Para cada dispositivo vivo: resolver (una vez) nombre, tipo y feature de batería.
//! 4. Leer batería. Al acabar el ciclo se cierran todos los handles.

mod hidpp;
mod settings;

use crate::controls::{choice, Control, DeviceControls, Kind, Section, Value};
use crate::device_ids::{self, KnownDevice};
use crate::i18n::tr;
use crate::hid::{interface_key, short_hash};
use crate::model::{BatteryReading, Connection, DeviceInfo, DeviceKind};
use crate::provider::{Capabilities, DeviceProvider, PollContext, ProviderError};
use hidpp::{BatteryFeature, Channel, HidppError, Ping};
use std::collections::{BTreeMap, HashMap};
use std::ffi::CString;

const VID: u16 = 0x046D;
const UP_HIDPP: u16 = 0xFF00;
/// Usage page vendor de los dispositivos Logitech por Bluetooth LE.
const UP_HIDPP_BLE: u16 = 0xFF43;

pub struct LogitechProvider {
    /// Canales abiertos durante el ciclo actual (clave = interfaz física).
    channels: HashMap<String, Channel>,
    /// id de dispositivo → dónde está.
    targets: HashMap<String, Target>,
    /// Lo que no cambia entre ciclos: nombre, tipo, feature de batería.
    cache: HashMap<String, DevCache>,
    /// Rutas (corta, larga) de cada interfaz vista en el último `discover`, para
    /// reabrir el canal al leer o escribir ajustes fuera del sondeo.
    paths: HashMap<String, (Option<CString>, Option<CString>)>,
}

struct Target {
    group: String,
    index: u8,
    online: bool,
}

#[derive(Default)]
struct DevCache {
    name: Option<String>,
    kind: Option<DeviceKind>,
    protocol: (u8, u8),
    battery: Option<(BatteryFeature, u8)>,
    /// Ya intentamos resolver features y no hay batería.
    no_battery: bool,
    /// Features de ajustes (DPI, tasa, memoria interna), resueltas la primera vez.
    settings: Option<settings::Features>,
}

/// Colecciones HID++ de una misma interfaz.
struct Group {
    pid: u16,
    short: Option<CString>,
    long: Option<CString>,
    product: String,
    bluetooth: bool,
}

impl LogitechProvider {
    pub fn new() -> Self {
        Self {
            channels: HashMap::new(),
            targets: HashMap::new(),
            cache: HashMap::new(),
            paths: HashMap::new(),
        }
    }

    /// Canal y ranura de un dispositivo en línea, reabriendo el canal si hace falta.
    fn channel_for(&mut self, ctx: &PollContext, device: &DeviceInfo) -> Result<(&mut Channel, u8), ProviderError> {
        let target = self.targets.get(&device.id).ok_or(ProviderError::NoResponse)?;
        if !target.online {
            return Err(ProviderError::Offline { last: None });
        }
        let index = target.index;
        let group = target.group.clone();
        if !self.channels.contains_key(&group) {
            let hid = ctx.hid.ok_or(ProviderError::NoResponse)?;
            let (short, long) = self.paths.get(&group).ok_or(ProviderError::NoResponse)?;
            let open = |p: &Option<CString>| p.as_ref().and_then(|p| hid.open(p).ok());
            let ch = Channel::new(open(short), open(long))
                .ok_or_else(|| ProviderError::Busy("no se pudo abrir el receptor".into()))?;
            self.channels.insert(group.clone(), ch);
        }
        Ok((self.channels.get_mut(&group).expect("recién insertado"), index))
    }

    /// Features de ajustes del dispositivo (se preguntan una sola vez).
    fn settings_for(&mut self, ctx: &PollContext, device: &DeviceInfo) -> Result<settings::Features, ProviderError> {
        if let Some(f) = self.cache.get(&device.id).and_then(|c| c.settings) {
            return Ok(f);
        }
        let (ch, index) = self.channel_for(ctx, device)?;
        let f = settings::Features::resolve(ch, index).map_err(hidpp_err)?;
        self.cache.entry(device.id.clone()).or_default().settings = Some(f);
        Ok(f)
    }

    fn device_id(group: &str, pid: u16, index: u8) -> String {
        format!("logitech:{pid:04x}:{}:{index:02x}", short_hash(group))
    }

    /// Nombre, tipo y feature de batería: solo la primera vez que lo vemos vivo.
    fn ensure_cache(ch: &mut Channel, index: u8, cache: &mut DevCache) {
        if cache.protocol.0 < 2 {
            return;
        }
        if cache.name.is_none() {
            if let Ok(Some(idx)) = hidpp::feature_index(ch, index, hidpp::FEAT_DEVICE_NAME) {
                cache.name = hidpp::device_name(ch, index, idx).ok().filter(|n| !n.is_empty());
                cache.kind = hidpp::device_kind(ch, index, idx).ok();
            }
        }
        if cache.battery.is_none() && !cache.no_battery {
            let mut timed_out = false;
            for (feature, id) in BatteryFeature::ALL {
                match hidpp::feature_index(ch, index, id) {
                    Ok(Some(idx)) => {
                        cache.battery = Some((feature, idx));
                        break;
                    }
                    Ok(None) => {}
                    Err(_) => timed_out = true,
                }
            }
            // Solo concluimos "sin batería" si todas las consultas respondieron.
            cache.no_battery = cache.battery.is_none() && !timed_out;
        }
    }
}

fn hidpp_err(e: HidppError) -> ProviderError {
    match e {
        HidppError::Timeout => ProviderError::NoResponse,
        HidppError::Hidpp10(hidpp::ERR10_RESOURCE | hidpp::ERR10_CONNECT_FAIL) => {
            ProviderError::Offline { last: None }
        }
        HidppError::Hidpp10(hidpp::ERR10_BUSY) | HidppError::Hidpp20(hidpp::ERR20_BUSY) => {
            ProviderError::Busy("el dispositivo está atendiendo otra petición".into())
        }
        HidppError::Hidpp10(c) => ProviderError::Protocol(format!("HID++ 1.0 error 0x{c:02X}")),
        HidppError::Hidpp20(c) => ProviderError::Protocol(format!("HID++ 2.0 error 0x{c:02X}")),
        HidppError::Io(s) => ProviderError::Io(s),
    }
}

fn hz_label(hz: u32) -> String {
    if hz >= 1000 && hz % 1000 == 0 {
        format!("{} KHZ", hz / 1000)
    } else {
        format!("{hz} HZ")
    }
}

/// Proceso en segundo plano de Logitech G HUB.
const GHUB_AGENT: &str = "lghub_agent.exe";

fn group_collections(ctx: &PollContext) -> BTreeMap<String, Group> {
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let Some(hid) = ctx.hid else { return groups };
    for e in hid.devices.iter().filter(|e| e.vid == VID) {
        let (is_short, is_long) = match (e.usage_page, e.usage) {
            (UP_HIDPP, 0x0001) | (UP_HIDPP_BLE, 0x0201) => (true, false),
            (UP_HIDPP, 0x0002) | (UP_HIDPP_BLE, 0x0202) => (false, true),
            _ => continue,
        };
        let g = groups.entry(interface_key(e)).or_insert_with(|| Group {
            pid: e.pid,
            short: None,
            long: None,
            product: e.product.clone(),
            bluetooth: e.usage_page == UP_HIDPP_BLE || e.bus.contains("Bluetooth"),
        });
        if is_short {
            g.short = Some(e.path.clone());
        }
        if is_long {
            g.long = Some(e.path.clone());
        }
    }
    groups
}

impl DeviceProvider for LogitechProvider {
    fn id(&self) -> &'static str {
        "logitech"
    }

    fn claims_hid(&self, vid: u16, _pid: u16, usage_page: u16, _usage: u16) -> bool {
        vid == VID && (usage_page == UP_HIDPP || usage_page == UP_HIDPP_BLE)
    }

    fn discover(&mut self, ctx: &PollContext) -> Result<Vec<DeviceInfo>, ProviderError> {
        let Some(hid) = ctx.hid else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        self.targets.clear();
        self.paths.clear();

        for (key, group) in group_collections(ctx) {
            self.paths.insert(key.clone(), (group.short.clone(), group.long.clone()));
            let open = |p: &Option<CString>| p.as_ref().and_then(|p| hid.open(p).ok());
            let Some(mut ch) = Channel::new(open(&group.short), open(&group.long)) else {
                log::warn!("logitech: no se pudo abrir {:04x} ({key})", group.pid);
                continue;
            };

            let known: Option<&KnownDevice> = device_ids::lookup(VID, group.pid);
            let mut slots: Vec<u8> = match known {
                Some(k) if k.receiver_slots > 0 => (1..=k.receiver_slots).collect(),
                _ => vec![hidpp::DEVICE_DIRECT],
            };
            let mut is_receiver = slots[0] != hidpp::DEVICE_DIRECT;

            let mut i = 0;
            while i < slots.len() {
                let index = slots[i];
                i += 1;
                let ping = match hidpp::ping(&mut ch, index) {
                    Ok(p) => {
                        log::debug!("logitech: ping {:04x}/{index:02x}: {p:?}", group.pid);
                        p
                    }
                    Err(e) => {
                        log::debug!("logitech: ping {:04x}/{index}: {e:?}", group.pid);
                        continue;
                    }
                };
                // Un PID desconocido que contesta como HID++ 1.0 en 0xFF es un receptor:
                // probamos sus 6 ranuras.
                if index == hidpp::DEVICE_DIRECT {
                    if let Ping::Online { major: 1, .. } = ping {
                        if !is_receiver {
                            is_receiver = true;
                            slots.extend(1..=6);
                        }
                        continue;
                    }
                }

                let id = Self::device_id(&key, group.pid, index);
                let online = match ping {
                    Ping::Absent => continue,
                    Ping::Offline => false,
                    Ping::Online { major, minor } => {
                        let cache = self.cache.entry(id.clone()).or_default();
                        cache.protocol = (major, minor);
                        Self::ensure_cache(&mut ch, index, cache);
                        true
                    }
                };

                let cache = self.cache.entry(id.clone()).or_default();
                let receiver_name = known.map(|k| k.name).unwrap_or("RECEPTOR");
                let name = cache.name.clone().unwrap_or_else(|| {
                    if is_receiver {
                        format!("{receiver_name} · SLOT {index}")
                    } else {
                        group.product.to_uppercase()
                    }
                });
                let connection = if is_receiver {
                    Connection::Receiver
                } else if group.bluetooth {
                    Connection::Bluetooth
                } else {
                    Connection::Wired
                };
                out.push(DeviceInfo {
                    id: id.clone(),
                    // Receptor + ranura: estable aunque se cambie de puerto USB.
                    fingerprint: format!("logitech:{:04x}:{index:02x}", group.pid),
                    provider: "logitech",
                    vendor: "Logitech".into(),
                    name_from_device: cache.name.is_some(),
                    name,
                    model: format!("{VID:04X}:{:04X}", group.pid),
                    kind: cache
                        .kind
                        .or_else(|| known.filter(|k| k.receiver_slots == 0).map(|k| k.kind))
                        .unwrap_or(DeviceKind::Other),
                    connection,
                    via: is_receiver.then(|| {
                        let short = receiver_name.trim_end_matches(" RECEIVER");
                        format!("{short} {:04X}", group.pid)
                    }),
                    slot: is_receiver.then_some(index),
                });
                self.targets.insert(
                    id,
                    Target {
                        group: key.clone(),
                        index,
                        online,
                    },
                );
            }
            self.channels.insert(key, ch);
        }
        Ok(out)
    }

    fn battery(
        &mut self,
        _ctx: &PollContext,
        device: &DeviceInfo,
    ) -> Result<BatteryReading, ProviderError> {
        let target = self
            .targets
            .get(&device.id)
            .ok_or(ProviderError::NoResponse)?;
        if !target.online {
            return Err(ProviderError::Offline { last: None });
        }
        let cache = self.cache.get_mut(&device.id).ok_or(ProviderError::NoResponse)?;
        if cache.protocol.0 < 2 {
            return Err(ProviderError::Unsupported(
                "HID++ 1.0 (dispositivo antiguo)".into(),
            ));
        }
        let ch = self
            .channels
            .get_mut(&target.group)
            .ok_or(ProviderError::NoResponse)?;
        if cache.battery.is_none() {
            Self::ensure_cache(ch, target.index, cache);
        }
        let Some((feature, idx)) = cache.battery else {
            return Err(if cache.no_battery {
                ProviderError::Unsupported("sin feature de batería".into())
            } else {
                ProviderError::NoResponse
            });
        };
        hidpp::read_battery(ch, target.index, feature, idx).map_err(hidpp_err)
    }

    /// Solo los ratones: es donde están el DPI y la tasa de sondeo. Las features
    /// concretas se preguntan al abrir la ficha (`controls`).
    fn capabilities(&self, device: &DeviceInfo) -> Capabilities {
        let known = self.cache.get(&device.id).and_then(|c| c.settings);
        let mouse = device.kind == DeviceKind::Mouse;
        Capabilities {
            dpi: known.map_or(mouse, |f| f.has_dpi()),
            polling_rate: known.map_or(mouse, |f| f.has_rate()),
            ..Capabilities::BATTERY_ONLY
        }
    }

    fn vendor_app(&self) -> Option<&'static str> {
        crate::synapse::process_running(GHUB_AGENT).then_some("Logitech G HUB")
    }

    fn controls(&mut self, ctx: &PollContext, device: &DeviceInfo) -> Result<DeviceControls, ProviderError> {
        let f = self.settings_for(ctx, device)?;
        if !f.any() {
            return Err(ProviderError::Unsupported(
                tr("este dispositivo no anuncia ajustes de DPI ni de sondeo", "this device advertises no DPI or report-rate settings").into(),
            ));
        }
        let (ch, index) = self.channel_for(ctx, device)?;
        let mut controls = Vec::new();
        let mut notes = Vec::new();
        // Cada ajuste se lee por separado: si el ratón rechaza una de las preguntas
        // (código de error HID++), ese ajuste se queda fuera y el resto sigue. Solo
        // se abandona si deja de contestar.
        let mut part = |what: &str, e: HidppError| -> Result<(), ProviderError> {
            match e {
                HidppError::Hidpp20(_) | HidppError::Hidpp10(hidpp::ERR10_INVALID_SUBID) => {
                    log::info!("logitech: {what}: {e:?}");
                    notes.push(format!(
                        "{what}: {} ({e:?})",
                        tr("el ratón rechazó la lectura; ajuste no disponible", "the mouse rejected the read; setting unavailable")
                    ));
                    Ok(())
                }
                e => Err(hidpp_err(e)),
            }
        };
        let mode = match settings::read_mode(ch, index, &f) {
            Ok(m) => m,
            Err(e) => part(tr("Memoria interna", "Onboard memory"), e).map(|_| None)?,
        };
        let dpi = match settings::read_dpi(ch, index, &f) {
            Ok(d) => d,
            Err(e) => part("DPI", e).map(|_| None)?,
        };
        let rate = match settings::read_rate(ch, index, &f) {
            Ok(r) => r,
            Err(e) => part(tr("Tasa de sondeo", "Report rate"), e).map(|_| None)?,
        };
        let onboard = mode == Some(settings::MODE_ONBOARD);
        if let Some(m) = mode {
            controls.push(
                Control::new(
                    "mode",
                    Section::Pointer,
                    tr("QUIÉN MANDA", "WHO IS IN CONTROL"),
                    choice([
                        (settings::MODE_ONBOARD as i64, tr("MEMORIA DEL RATÓN", "ONBOARD MEMORY").to_string()),
                        (settings::MODE_HOST as i64, tr("SOFTWARE (VOLT)", "SOFTWARE (VOLT)").to_string()),
                    ]),
                    Some(Value::Int(m as i64)),
                )
                .volatile()
                .hint(tr(
                    "Con la memoria del ratón manda el perfil guardado en él (el que dejó G HUB). Para que el DPI y la tasa de sondeo de abajo tengan efecto suele hacer falta el modo software; el ratón vuelve a su memoria al apagarse.",
                    "With onboard memory the profile stored in the mouse (the one G HUB left) is in charge. The DPI and report rate below usually need software mode to take effect; the mouse returns to its memory when switched off.",
                )),
            );
            if onboard {
                notes.push(
                    tr(
                        "El ratón está usando su memoria interna: puede ignorar el DPI y la tasa que pongas aquí hasta que cambies a modo software.",
                        "The mouse is using its onboard memory: it may ignore the DPI and report rate set here until you switch to software mode.",
                    )
                    .into(),
                );
            }
        }
        if let Some(d) = dpi {
            if let (Some(min), Some(max)) = (settings::dpi_min(&d.segs), settings::dpi_max(&d.segs)) {
                let presets = [400u32, 800, 1200, 1600, 3200]
                    .into_iter()
                    .filter_map(|p| settings::dpi_snap(&d.segs, p).filter(|&v| v as u32 == p))
                    .map(i64::from)
                    .collect();
                controls.push(
                    Control::new(
                        "dpi",
                        Section::Pointer,
                        "DPI",
                        Kind::Range {
                            min: min as i64,
                            max: max as i64,
                            step: settings::dpi_step(&d.segs).max(1) as i64,
                            unit: "DPI",
                            presets,
                        },
                        Some(Value::Int(d.current as i64)),
                    )
                    .volatile()
                    .hint(tr(
                        "Sensibilidad del sensor. Los límites y el paso son los que anuncia el propio ratón; el valor se ajusta al válido más cercano.",
                        "Sensor sensitivity. Limits and step are the ones the mouse itself reports; the value snaps to the nearest valid one.",
                    )),
                );
            }
        }
        if let Some(r) = rate {
            if !r.options.is_empty() {
                controls.push(
                    Control::new(
                        "rate",
                        Section::Pointer,
                        tr("TASA DE SONDEO", "REPORT RATE"),
                        choice(r.options.iter().map(|&hz| (hz as i64, hz_label(hz)))),
                        r.current.map(|hz| Value::Int(hz as i64)),
                    )
                    .volatile()
                    .hint(tr(
                        "Veces por segundo que el ratón informa de su posición. Más tasa gasta más batería.",
                        "How many times per second the mouse reports its position. Higher rates use more battery.",
                    )),
                );
            }
        }
        if let Some(app) = self.vendor_app() {
            notes.push(format!(
                "{app} {}",
                tr(
                    "está abierto y puede volver a imponer su perfil en cualquier momento.",
                    "is running and may re-apply its own profile at any time."
                )
            ));
        }
        Ok(DeviceControls {
            controls,
            source: tr(
                "Features estándar de HID++ 2.0 que anuncia el propio ratón (0x2201/0x2202 DPI, 0x8060/0x8061 tasa, 0x8100 memoria interna), implementadas según Solaar. Sin probar desde Volt en este ratón: tras cada cambio se relee el valor.",
                "Standard HID++ 2.0 features advertised by the mouse itself (0x2201/0x2202 DPI, 0x8060/0x8061 rate, 0x8100 onboard memory), implemented after Solaar. Untested from Volt on this mouse: the value is read back after every change.",
            )
            .into(),
            notes,
        })
    }

    fn set_control(
        &mut self,
        ctx: &PollContext,
        device: &DeviceInfo,
        control: &str,
        value: &Value,
    ) -> Result<(), ProviderError> {
        let f = self.settings_for(ctx, device)?;
        let (ch, index) = self.channel_for(ctx, device)?;
        let bad = || ProviderError::Protocol("valor fuera de rango".into());
        let n = value.as_int().ok_or_else(bad)?;
        match control {
            "mode" => {
                let mode = u8::try_from(n)
                    .ok()
                    .filter(|m| [settings::MODE_ONBOARD, settings::MODE_HOST].contains(m))
                    .ok_or_else(bad)?;
                settings::write_mode(ch, index, &f, mode).map_err(hidpp_err)
            }
            "dpi" => {
                let dpi = settings::read_dpi(ch, index, &f)
                    .map_err(hidpp_err)?
                    .ok_or_else(|| ProviderError::Unsupported("DPI".into()))?;
                let want = u32::try_from(n).map_err(|_| bad())?;
                let snapped = settings::dpi_snap(&dpi.segs, want).ok_or_else(bad)?;
                settings::write_dpi(ch, index, &f, &dpi, snapped).map_err(hidpp_err)
            }
            "rate" => {
                let rate = settings::read_rate(ch, index, &f)
                    .map_err(hidpp_err)?
                    .ok_or_else(|| ProviderError::Unsupported("report rate".into()))?;
                let hz = u32::try_from(n).ok().filter(|hz| rate.options.contains(hz)).ok_or_else(bad)?;
                settings::write_rate(ch, index, &f, hz).map_err(hidpp_err)
            }
            other => Err(ProviderError::Unsupported(other.into())),
        }
    }

    fn release(&mut self) {
        // Cerrar handles: mientras la app duerme no queda nada abierto.
        self.channels.clear();
    }
}
