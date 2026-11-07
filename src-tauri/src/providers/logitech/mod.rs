//! Provider Logitech: HID++ 2.0 sobre receptores (LIGHTSPEED, Unifying, Bolt),
//! cable USB y Bluetooth directo.
//!
//! Flujo por ciclo:
//! 1. Agrupar las colecciones HID++ (usage page 0xFF00/0xFF43) por interfaz física.
//! 2. Abrir corto+largo, hacer ping a cada ranura del receptor (o a 0xFF si es directo).
//! 3. Para cada dispositivo vivo: resolver (una vez) nombre, tipo y feature de batería.
//! 4. Leer batería. Al acabar el ciclo se cierran todos los handles.

mod hidpp;

use crate::device_ids::{self, KnownDevice};
use crate::hid::{interface_key, short_hash};
use crate::model::{BatteryReading, Connection, DeviceInfo, DeviceKind};
use crate::provider::{DeviceProvider, PollContext, ProviderError};
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
        }
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

        for (key, group) in group_collections(ctx) {
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
        hidpp::read_battery(ch, target.index, feature, idx).map_err(|e| match e {
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
        })
    }

    fn release(&mut self) {
        // Cerrar handles: mientras la app duerme no queda nada abierto.
        self.channels.clear();
    }
}
