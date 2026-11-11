//! Provider Razer (experimental): protocolo de informes de función de 90 bytes
//! documentado por openrazer y usado por razer-battery-tray.
//!
//! ```text
//! [0]  status            0x00 = nueva orden
//! [1]  transaction id    0x1F / 0x3F / 0xFF según modelo (se sondea)
//! [2,3] remaining packets
//! [4]  protocol type     0x00
//! [5]  data size
//! [6]  command class     0x07 = energía
//! [7]  command id        0x80 = nivel de batería, 0x84 = estado de carga
//! [8..88] argumentos     respuesta: arg[1] = nivel 0..255
//! [88] crc               XOR de los bytes 2..87
//! [89] reservado
//! ```
//!
//! Sirve para ratones y teclados. Los auriculares HyperSpeed recientes
//! (BlackShark V2 Pro 2023) usan "Audio MXIC": ver `razer_mxic.rs`. Otros
//! auriculares Razer quedan como "sin soporte" (solo en diagnóstico).

use super::razer_mxic;
use crate::controls::{choice, BandPreset, Control, DeviceControls, Kind, Section, Trust, Value};
use crate::device_ids;
use crate::i18n::tr;
use crate::hid::HidEntry;
use crate::model::{BatteryReading, ChargeState, Connection, DeviceInfo, DeviceKind};
use crate::provider::{Capabilities, DeviceProvider, ListenSpec, PollContext, ProviderError};
use hidapi::HidDevice;
use std::collections::HashMap;
use std::ffi::CString;
use std::thread::sleep;
use std::time::Duration;

const VID: u16 = 0x1532;
const LEN: usize = 90;
const TRANSACTION_IDS: [u8; 4] = [0x1F, 0x3F, 0xFF, 0x9F];

const STATUS_BUSY: u8 = 0x01;
const STATUS_OK: u8 = 0x02;
const STATUS_FAIL: u8 = 0x03;
const STATUS_TIMEOUT: u8 = 0x04;
const STATUS_UNSUPPORTED: u8 = 0x05;

#[derive(Clone)]
enum Probe {
    Supported { path: CString, tid: u8 },
    /// Auriculares HyperSpeed con protocolo "Audio MXIC" (ver `razer_mxic`).
    Mxic { path: CString },
    /// Habla el protocolo pero no tiene batería (p. ej. teclado con cable).
    NoBattery,
    /// No responde a este protocolo.
    Unsupported,
}

pub struct RazerProvider {
    /// Resultado del sondeo por PID (se repite si el dispositivo desaparece y vuelve).
    probes: HashMap<u16, Probe>,
}

#[derive(Debug)]
enum RzError {
    Io(String),
    Status(u8),
    Mismatch,
}

impl RazerProvider {
    pub fn new() -> Self {
        Self {
            probes: HashMap::new(),
        }
    }

    fn probe_of(&self, device: &DeviceInfo) -> Option<&Probe> {
        let pid = u16::from_str_radix(device.id.trim_start_matches("razer:"), 16).ok()?;
        self.probes.get(&pid)
    }
}

fn build(tid: u8, class: u8, id: u8, size: u8, args: &[u8]) -> [u8; LEN + 1] {
    let mut b = [0u8; LEN + 1]; // b[0] = report id 0
    let r = &mut b[1..];
    r[1] = tid;
    r[5] = size;
    r[6] = class;
    r[7] = id;
    r[8..8 + args.len()].copy_from_slice(args);
    r[88] = r[2..88].iter().fold(0, |a, x| a ^ x);
    b
}

fn query(dev: &HidDevice, tid: u8, class: u8, id: u8, size: u8) -> Result<[u8; LEN], RzError> {
    command(dev, tid, class, id, size, &[])
}

/// Orden con argumentos (los argumentos de la respuesta empiezan en `[8]`).
fn command(dev: &HidDevice, tid: u8, class: u8, id: u8, size: u8, args: &[u8]) -> Result<[u8; LEN], RzError> {
    let req = build(tid, class, id, size, args);
    dev.send_feature_report(&req)
        .map_err(|e| RzError::Io(e.to_string()))?;
    for attempt in 0..6 {
        sleep(Duration::from_millis(if attempt == 0 { 35 } else { 60 }));
        let mut resp = [0u8; LEN + 1];
        let n = dev
            .get_feature_report(&mut resp)
            .map_err(|e| RzError::Io(e.to_string()))?;
        if n < LEN {
            return Err(RzError::Io(format!("respuesta corta ({n} bytes)")));
        }
        let mut r = [0u8; LEN];
        r.copy_from_slice(&resp[1..]);
        match r[0] {
            STATUS_BUSY | 0x00 => continue,
            STATUS_OK if r[6] == class && r[7] == id => return Ok(r),
            STATUS_OK => return Err(RzError::Mismatch),
            s => return Err(RzError::Status(s)),
        }
    }
    Err(RzError::Status(STATUS_TIMEOUT))
}

fn read_battery(dev: &HidDevice, tid: u8) -> Result<BatteryReading, RzError> {
    let level = query(dev, tid, 0x07, 0x80, 0x02)?[9];
    let charging = query(dev, tid, 0x07, 0x84, 0x02)
        .map(|r| r[9] == 1)
        .unwrap_or(false);
    // Con el ratón dormido el receptor contesta OK con nivel 0: no es un 0 % real.
    if level == 0 && !charging {
        return Err(RzError::Status(STATUS_TIMEOUT));
    }
    let percent = ((level as u16 * 100 + 127) / 255) as u8;
    Ok(BatteryReading::percent(
        percent,
        if charging {
            ChargeState::Charging
        } else {
            ChargeState::Discharging
        },
    ))
}

/// Guardar en la memoria del dispositivo (openrazer: `VARSTORE`).
const VARSTORE: u8 = 0x01;

/// Código de tasa de sondeo de openrazer (`razer_chroma_misc_set_polling_rate`).
fn poll_code(hz: i64) -> Option<u8> {
    match hz {
        1000 => Some(0x01),
        500 => Some(0x02),
        125 => Some(0x08),
        _ => None,
    }
}

/// Rango de las bandas del ecualizador que ofrece Volt. Los presets conocidos de
/// Synapse se mueven entre −5 y +5; se deja un poco de margen, sin ir más allá.
const EQ_RANGE: i64 = 6;
const EQ_LABELS: [&str; razer_mxic::EQ_BANDS] = ["31", "63", "125", "250", "500", "1K", "2K", "4K", "8K", "16K"];
/// Curvas publicadas por rzr (capturadas de Synapse 4).
const EQ_PRESETS: [(&str, &str, [i64; razer_mxic::EQ_BANDS]); 4] = [
    ("PLANO", "FLAT", [0; 10]),
    ("JUEGO", "GAME", [-3, -3, -4, 0, 5, 5, 4, 1, 0, -1]),
    ("MÚSICA", "MUSIC", [2, 2, 1, 1, 2, 3, 3, 3, 1, 0]),
    ("CINE", "MOVIE", [4, 4, 3, 0, -3, -1, 3, 5, 2, 1]),
];

fn bad_value() -> ProviderError {
    ProviderError::Protocol("valor fuera de rango".into())
}

fn bands_from(value: &Value) -> Result<[u8; razer_mxic::EQ_BANDS], ProviderError> {
    let Value::List(v) = value else { return Err(bad_value()) };
    if v.len() != razer_mxic::EQ_BANDS || v.iter().any(|b| b.abs() > EQ_RANGE) {
        return Err(bad_value());
    }
    let mut out = [0u8; razer_mxic::EQ_BANDS];
    for (o, b) in out.iter_mut().zip(v) {
        *o = *b as i8 as u8;
    }
    Ok(out)
}

fn mxic_err(e: razer_mxic::MxicError) -> ProviderError {
    match e {
        razer_mxic::MxicError::NoReply => ProviderError::Offline { last: None },
        razer_mxic::MxicError::Io(s) => ProviderError::Io(s),
    }
}

fn rz_err(e: RzError) -> ProviderError {
    match e {
        RzError::Io(s) => ProviderError::Io(s),
        RzError::Status(STATUS_TIMEOUT) => ProviderError::Offline { last: None },
        RzError::Status(STATUS_UNSUPPORTED) => ProviderError::Unsupported("el dispositivo no admite esta orden".into()),
        RzError::Status(s) => ProviderError::Protocol(format!("estado 0x{s:02X}")),
        RzError::Mismatch => ProviderError::Busy("respuesta cruzada (¿Razer Synapse?)".into()),
    }
}

fn mxic_controls(dev: &HidDevice) -> Result<DeviceControls, ProviderError> {
    use razer_mxic::{PRESET_CUSTOM, PRESET_GAME, PRESET_MOVIE, PRESET_MUSIC};
    let st = razer_mxic::read_audio(dev).map_err(mxic_err)?;
    let mut controls = Vec::new();
    if let Some(p) = st.preset {
        let mut options = vec![
            (PRESET_GAME as i64, tr("JUEGO", "GAME").to_string()),
            (PRESET_MUSIC as i64, tr("MÚSICA", "MUSIC").to_string()),
            (PRESET_MOVIE as i64, tr("CINE", "MOVIE").to_string()),
            (PRESET_CUSTOM as i64, tr("PERSONALIZADO", "CUSTOM").to_string()),
        ];
        if !options.iter().any(|(v, _)| *v == p as i64) {
            let name = if (0xFA..=0xFE).contains(&p) { "ESPORTS".to_string() } else { format!("0x{p:02X}") };
            options.push((p as i64, name));
        }
        controls.push(
            Control::new("eq.preset", Section::Audio, tr("PRESET DE ECUALIZADOR", "EQ PRESET"), choice(options), Some(Value::Int(p as i64)))
                .trust(Trust::Experimental)
                .volatile()
                .hint(tr(
                    "Los valores de juego, música y cine se han visto al leer el auricular, pero escribirlos no está verificado (solo «personalizado» lo está).",
                    "The game, music and movie values were seen when reading the headset, but writing them is unverified (only \"custom\" is).",
                )),
        );
    }
    controls.push(
        Control::new(
            "eq.bands",
            Section::Audio,
            tr("ECUALIZADOR PERSONALIZADO", "CUSTOM EQUALISER"),
            Kind::Bands {
                labels: EQ_LABELS.iter().map(|s| s.to_string()).collect(),
                min: -EQ_RANGE,
                max: EQ_RANGE,
                unit: "dB",
                presets: EQ_PRESETS
                    .iter()
                    .map(|(es, en, values)| BandPreset { label: tr(es, en).into(), values: values.to_vec() })
                    .collect(),
            },
            st.bands.map(|b| Value::List(b.iter().map(|&x| x as i64).collect())),
        )
        .volatile()
        .hint(tr(
            "10 bandas. Al cambiarlas el auricular pasa al preset personalizado.",
            "10 bands. Changing them switches the headset to the custom preset.",
        )),
    );
    controls.push(
        Control::new(
            "audio.enhancement",
            Section::Audio,
            tr("MEJORA DE AUDIO", "AUDIO ENHANCEMENT"),
            Kind::Toggle,
            st.enhancement.map(Value::Bool),
        )
        .volatile()
        .hint(tr(
            "El procesado que Synapse activa al cargar un perfil.",
            "The processing Synapse turns on when it loads a profile.",
        )),
    );
    // El tono lateral solo se ofrece si el auricular contesta a su lectura.
    if let Some(on) = st.sidetone {
        controls.push(
            Control::new("audio.sidetone", Section::Audio, tr("TONO LATERAL", "SIDETONE"), Kind::Toggle, Some(Value::Bool(on)))
                .trust(Trust::Experimental)
                .hint(tr("Oír tu propia voz por los auriculares.", "Hear your own voice in the headset.")),
        );
    }
    if let Some(level) = st.sidetone_level {
        controls.push(
            Control::new(
                "audio.sidetone_level",
                Section::Audio,
                tr("NIVEL DEL TONO LATERAL", "SIDETONE LEVEL"),
                Kind::Range { min: 0, max: 100, step: 5, unit: "%", presets: vec![] },
                Some(Value::Int(level as i64)),
            )
            .trust(Trust::Experimental),
        );
    }
    let mut notes = Vec::new();
    if crate::synapse::is_running() {
        notes.push(
            tr(
                "Razer Synapse está abierto y gestiona el auricular: puede deshacer estos cambios.",
                "Razer Synapse is running and manages the headset: it may undo these changes.",
            )
            .into(),
        );
    }
    Ok(DeviceControls {
        controls,
        source: tr(
            "Protocolo «Audio MXIC» según rzr / razer-device-control (MIT), que verificó en este modelo el ecualizador de 10 bandas, el preset personalizado y la mejora de audio. Sin probar desde Volt: tras cada cambio se relee lo que el auricular permite leer.",
            "\"Audio MXIC\" protocol after rzr / razer-device-control (MIT), which verified the 10-band equaliser, the custom preset and audio enhancement on this model. Untested from Volt: whatever the headset lets us read is read back after every change.",
        )
        .into(),
        notes,
    })
}

fn mxic_set(dev: &HidDevice, control: &str, value: &Value) -> Result<(), ProviderError> {
    use razer_mxic::*;
    let byte = |max: i64| value.as_int().filter(|n| (0..=max).contains(n)).map(|n| n as u8).ok_or_else(bad_value);
    let flag = || value.as_bool().map(|b| b as u8).ok_or_else(bad_value);
    match control {
        "eq.preset" => write_audio(dev, &[(CMD_PRESET, &[byte(255)?])]),
        "eq.bands" => {
            let bands = bands_from(value)?;
            // Como rzr: EQ activo, preset personalizado y la curva.
            write_audio(dev, &[(CMD_PRESET_EQ_STATUS, &[0]), (CMD_PRESET, &[PRESET_CUSTOM]), (CMD_BANDS, &bands)])
        }
        "audio.enhancement" => write_audio(dev, &[(CMD_ENHANCEMENT, &[flag()?])]),
        "audio.sidetone" => write_audio(dev, &[(CMD_SIDETONE, &[flag()?])]),
        "audio.sidetone_level" => write_audio(dev, &[(CMD_SIDETONE_LEVEL, &[byte(100)?])]),
        other => return Err(ProviderError::Unsupported(other.into())),
    }
    .map_err(mxic_err)
}

/// Tiempos de reposo que acepta openrazer: entre 60 y 900 s.
const IDLE_PRESETS: [i64; 6] = [60, 120, 300, 600, 780, 900];

fn mouse_controls(dev: &HidDevice, tid: u8) -> Result<DeviceControls, ProviderError> {
    let mut controls = Vec::new();
    // Cada lectura que falle simplemente deja fuera su control.
    if let Ok(r) = command(dev, tid, 0x04, 0x85, 0x07, &[VARSTORE]) {
        let dpi = u16::from_be_bytes([r[9], r[10]]);
        if dpi > 0 {
            controls.push(
                Control::new(
                    "dpi",
                    Section::Pointer,
                    "DPI",
                    Kind::Range { min: 100, max: 30000, step: 50, unit: "DPI", presets: vec![400, 800, 1600, 3200] },
                    Some(Value::Int(dpi as i64)),
                )
                .trust(Trust::Experimental)
                .hint(tr(
                    "El máximo real depende del sensor: el ratón recorta lo que no admite y aquí se enseña lo que contesta.",
                    "The real maximum depends on the sensor: the mouse clamps what it cannot do and this shows what it answers.",
                )),
            );
        }
    }
    if let Ok(r) = command(dev, tid, 0x00, 0x85, 0x01, &[]) {
        let hz = match r[8] {
            0x01 => Some(1000),
            0x02 => Some(500),
            0x08 => Some(125),
            _ => None,
        };
        controls.push(
            Control::new(
                "rate",
                Section::Pointer,
                tr("TASA DE SONDEO", "POLLING RATE"),
                choice([(125, "125 HZ".to_string()), (500, "500 HZ".to_string()), (1000, "1 KHZ".to_string())]),
                hz.map(Value::Int),
            )
            .trust(Trust::Experimental),
        );
    }
    if let Ok(r) = command(dev, tid, 0x07, 0x83, 0x02, &[]) {
        let secs = u16::from_be_bytes([r[8], r[9]]) as i64;
        if secs > 0 {
            let mut times = IDLE_PRESETS.to_vec();
            if !times.contains(&secs) {
                times.push(secs);
                times.sort_unstable();
            }
            controls.push(
                Control::new(
                    "power.idle",
                    Section::Power,
                    tr("REPOSO", "SLEEP"),
                    choice(times.into_iter().map(|s| (s, format!("{} MIN", s / 60)))),
                    Some(Value::Int(secs)),
                )
                .trust(Trust::Experimental)
                .hint(tr("Tiempo sin uso hasta que el dispositivo ahorra batería.", "Idle time before the device saves power.")),
            );
        }
    }
    if controls.is_empty() {
        return Err(ProviderError::Unsupported(
            tr("el dispositivo no contesta a las órdenes de DPI, sondeo ni reposo", "the device answers none of the DPI, polling or sleep commands").into(),
        ));
    }
    Ok(DeviceControls {
        controls,
        source: tr(
            "Órdenes del driver openrazer (informe de 90 bytes: DPI 04/05, sondeo 00/05, reposo 07/03). Experimental: sin verificar con ningún ratón o teclado Razer desde Volt.",
            "Commands from the openrazer driver (90-byte report: DPI 04/05, polling 00/05, idle 07/03). Experimental: not verified with any Razer mouse or keyboard from Volt.",
        )
        .into(),
        notes: Vec::new(),
    })
}

fn mouse_set(dev: &HidDevice, tid: u8, control: &str, value: &Value) -> Result<(), ProviderError> {
    let n = value.as_int().ok_or_else(bad_value)?;
    match control {
        "dpi" => {
            let dpi = u16::try_from(n).ok().filter(|d| (100..=30000).contains(d)).ok_or_else(bad_value)?;
            let [hi, lo] = dpi.to_be_bytes();
            command(dev, tid, 0x04, 0x05, 0x07, &[VARSTORE, hi, lo, hi, lo, 0, 0])
        }
        "rate" => command(dev, tid, 0x00, 0x05, 0x01, &[poll_code(n).ok_or_else(bad_value)?]),
        "power.idle" => {
            let secs = u16::try_from(n).ok().filter(|s| (60..=900).contains(s)).ok_or_else(bad_value)?;
            command(dev, tid, 0x07, 0x03, 0x02, &secs.to_be_bytes())
        }
        other => return Err(ProviderError::Unsupported(other.into())),
    }
    .map(|_| ())
    .map_err(rz_err)
}

/// Orden de prueba: colección de ratón, de teclado, y el resto.
fn candidates<'a>(entries: &'a [&'a HidEntry]) -> Vec<&'a HidEntry> {
    let rank = |e: &HidEntry| match (e.usage_page, e.usage) {
        (0x0001, 0x0002) => 0,
        (0x0001, 0x0006) => 1,
        (0x000C, _) => 3,
        _ => 2,
    };
    let mut v: Vec<&HidEntry> = entries.to_vec();
    v.sort_by_key(|e| (e.interface, rank(e)));
    v
}

fn probe(hid: &crate::hid::HidContext, entries: &[&HidEntry]) -> Probe {
    let mut speaks_protocol = false;
    for e in candidates(entries) {
        let Ok(dev) = hid.open(&e.path) else { continue };
        for tid in TRANSACTION_IDS {
            match query(&dev, tid, 0x07, 0x80, 0x02) {
                Ok(_) => {
                    return Probe::Supported {
                        path: e.path.clone(),
                        tid,
                    }
                }
                Err(RzError::Status(STATUS_UNSUPPORTED | STATUS_FAIL)) => speaks_protocol = true,
                // Sin informe de función en esta colección: siguiente colección.
                Err(RzError::Io(_)) => break,
                Err(_) => {}
            }
        }
    }
    if speaks_protocol {
        Probe::NoBattery
    } else {
        Probe::Unsupported
    }
}

impl DeviceProvider for RazerProvider {
    fn id(&self) -> &'static str {
        "razer"
    }

    fn claims_hid(&self, vid: u16, pid: u16, up: u16, _u: u16) -> bool {
        vid == VID
            && match self.probes.get(&pid) {
                Some(Probe::Supported { .. }) => true,
                Some(Probe::Mxic { .. }) => up == razer_mxic::USAGE_PAGE,
                _ => false,
            }
    }

    /// Los auriculares MXIC avisan solos de cambios de batería y de carga.
    fn listener(&self, device: &DeviceInfo, current: Option<&BatteryReading>) -> Option<ListenSpec> {
        let pid = u16::from_str_radix(device.id.trim_start_matches("razer:"), 16).ok()?;
        match self.probes.get(&pid)? {
            Probe::Mxic { path } => Some(ListenSpec {
                path: path.clone(),
                parser: Box::new(razer_mxic::event_parser(current)),
                silence: None,
            }),
            _ => None,
        }
    }

    fn capabilities(&self, device: &DeviceInfo) -> Capabilities {
        match self.probe_of(device) {
            Some(Probe::Mxic { .. }) => Capabilities { audio: true, ..Capabilities::BATTERY_ONLY },
            Some(Probe::Supported { .. }) => Capabilities {
                dpi: device.kind == DeviceKind::Mouse,
                polling_rate: true,
                sleep: true,
                ..Capabilities::BATTERY_ONLY
            },
            _ => Capabilities::BATTERY_ONLY,
        }
    }

    fn vendor_app(&self) -> Option<&'static str> {
        crate::synapse::is_running().then_some("Razer Synapse")
    }

    fn controls(&mut self, ctx: &PollContext, device: &DeviceInfo) -> Result<DeviceControls, ProviderError> {
        let hid = ctx.hid.ok_or(ProviderError::NoResponse)?;
        let open = |path: &CString| {
            hid.open(path)
                .map_err(|e| ProviderError::Busy(format!("no se pudo abrir ({e})")))
        };
        match self.probe_of(device) {
            Some(Probe::Mxic { path }) => mxic_controls(&open(path)?),
            Some(Probe::Supported { path, tid }) => mouse_controls(&open(path)?, *tid),
            _ => Err(ProviderError::Unsupported("protocolo no reconocido".into())),
        }
    }

    fn set_control(
        &mut self,
        ctx: &PollContext,
        device: &DeviceInfo,
        control: &str,
        value: &Value,
    ) -> Result<(), ProviderError> {
        let hid = ctx.hid.ok_or(ProviderError::NoResponse)?;
        let open = |path: &CString| {
            hid.open(path)
                .map_err(|e| ProviderError::Busy(format!("no se pudo abrir ({e})")))
        };
        match self.probe_of(device) {
            Some(Probe::Mxic { path }) => mxic_set(&open(path)?, control, value),
            Some(Probe::Supported { path, tid }) => mouse_set(&open(path)?, *tid, control, value),
            _ => Err(ProviderError::Unsupported("protocolo no reconocido".into())),
        }
    }

    fn discover(&mut self, ctx: &PollContext) -> Result<Vec<DeviceInfo>, ProviderError> {
        let Some(hid) = ctx.hid else {
            return Ok(Vec::new());
        };
        let mut by_pid: HashMap<u16, Vec<&HidEntry>> = HashMap::new();
        for e in hid.devices.iter().filter(|e| e.vid == VID) {
            by_pid.entry(e.pid).or_default().push(e);
        }
        // Olvidar sondeos de lo que ya no está conectado.
        self.probes.retain(|pid, _| by_pid.contains_key(pid));

        let mut out = Vec::new();
        for (pid, entries) in by_pid {
            let probe = self
                .probes
                .entry(pid)
                .or_insert_with(|| {
                    // Los MXIC se conocen por PID: no se les manda el sondeo de 90 bytes.
                    if razer_mxic::PIDS.contains(&pid) {
                        entries
                            .iter()
                            .find(|e| e.usage_page == razer_mxic::USAGE_PAGE)
                            .map(|e| Probe::Mxic { path: e.path.clone() })
                            .unwrap_or(Probe::Unsupported)
                    } else {
                        probe(hid, &entries)
                    }
                })
                .clone();
            if matches!(probe, Probe::Mxic { .. }) {
                // El descriptor USB trae el nombre comercial ("Razer BlackShark V2 Pro 2.4").
                let product = entries.iter().map(|e| e.product.as_str()).find(|p| !p.is_empty());
                let (name, from_device) = match product {
                    Some(p) => (
                        p.trim_start_matches("Razer ").trim_end_matches(" 2.4").to_uppercase(),
                        true,
                    ),
                    None => (
                        device_ids::lookup(VID, pid).map(|k| k.name).unwrap_or("RAZER HEADSET").into(),
                        false,
                    ),
                };
                out.push(DeviceInfo {
                    id: format!("razer:{pid:04x}"),
                    fingerprint: format!("razer:{pid:04x}"),
                    provider: "razer",
                    vendor: "Razer".into(),
                    name,
                    name_from_device: from_device,
                    model: format!("{VID:04X}:{pid:04X}"),
                    kind: DeviceKind::Headset,
                    connection: Connection::Receiver,
                    via: Some(format!("HYPERSPEED {pid:04X}")),
                    slot: None,
                });
                continue;
            }
            if !matches!(probe, Probe::Supported { .. }) {
                continue;
            }
            let known = device_ids::lookup(VID, pid);
            let product = entries
                .iter()
                .map(|e| e.product.as_str())
                .find(|p| !p.is_empty())
                .unwrap_or("Razer");
            let name = known.map(|k| k.name.to_string()).unwrap_or_else(|| {
                product
                    .trim_start_matches("Razer ")
                    .to_uppercase()
            });
            let lower = product.to_lowercase();
            let kind = known.map(|k| k.kind).unwrap_or(if lower.contains("keyboard")
                || lower.contains("huntsman")
                || lower.contains("blackwidow")
            {
                DeviceKind::Keyboard
            } else {
                DeviceKind::Mouse
            });
            let wireless = lower.contains("wireless") || lower.contains("dongle") || lower.contains("receiver");
            out.push(DeviceInfo {
                id: format!("razer:{pid:04x}"),
                fingerprint: format!("razer:{pid:04x}"),
                provider: "razer",
                vendor: "Razer".into(),
                name_from_device: known.is_none() && product != "Razer",
                name,
                model: format!("{VID:04X}:{pid:04X}"),
                kind,
                connection: if wireless {
                    Connection::Receiver
                } else {
                    Connection::Wired
                },
                via: wireless.then(|| format!("HYPERSPEED {pid:04X}")),
                slot: None,
            });
        }
        Ok(out)
    }

    fn battery(
        &mut self,
        ctx: &PollContext,
        device: &DeviceInfo,
    ) -> Result<BatteryReading, ProviderError> {
        let hid = ctx.hid.ok_or(ProviderError::NoResponse)?;
        let pid = u16::from_str_radix(device.id.trim_start_matches("razer:"), 16)
            .map_err(|_| ProviderError::NoResponse)?;
        let (path, tid) = match self.probes.get(&pid).cloned() {
            Some(Probe::Supported { path, tid }) => (path, tid),
            Some(Probe::Mxic { path }) => {
                let dev = hid
                    .open(&path)
                    .map_err(|e| ProviderError::Busy(format!("no se pudo abrir ({e})")))?;
                // Solo se toca el EQ si el ajuste está activo y Synapse no gestiona el auricular.
                let keep_eq = ctx.razer_keep_eq && !crate::synapse::is_running();
                return razer_mxic::read_battery(&dev, keep_eq).map_err(|e| match e {
                    razer_mxic::MxicError::NoReply => ProviderError::Offline { last: None },
                    razer_mxic::MxicError::Io(s) => ProviderError::Io(s),
                });
            }
            _ => return Err(ProviderError::Unsupported("protocolo no reconocido".into())),
        };
        let dev = hid
            .open(&path)
            .map_err(|e| ProviderError::Busy(format!("no se pudo abrir ({e})")))?;
        read_battery(&dev, tid).map_err(|e| match e {
            RzError::Io(s) => ProviderError::Io(s),
            RzError::Status(STATUS_TIMEOUT) => ProviderError::Offline { last: None },
            RzError::Status(STATUS_UNSUPPORTED) => {
                ProviderError::Unsupported("el dispositivo no expone batería".into())
            }
            RzError::Status(s) => ProviderError::Protocol(format!("estado 0x{s:02X}")),
            // Otra app (Synapse) contestó en medio: lo tratamos como ocupado.
            RzError::Mismatch => ProviderError::Busy("respuesta cruzada (¿Razer Synapse?)".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_xor_of_payload() {
        let b = build(0x1F, 0x07, 0x80, 0x02, &[]);
        let r = &b[1..];
        assert_eq!(r[1], 0x1F);
        assert_eq!(r[88], 0x02 ^ 0x07 ^ 0x80);
    }

    #[test]
    fn dpi_command_matches_openrazer() {
        // razer_chroma_misc_set_dpi_xy(VARSTORE, 1600, 1600): clase 04, id 05, 7 bytes.
        let b = build(0x1F, 0x04, 0x05, 0x07, &[VARSTORE, 0x06, 0x40, 0x06, 0x40, 0, 0]);
        let r = &b[1..];
        assert_eq!(&r[5..15], &[0x07, 0x04, 0x05, 0x01, 0x06, 0x40, 0x06, 0x40, 0, 0]);
        assert_eq!(r[88], 0x07 ^ 0x04 ^ 0x05 ^ 0x01);
        assert_eq!((poll_code(1000), poll_code(500), poll_code(125), poll_code(333)), (Some(1), Some(2), Some(8), None));
    }

    #[test]
    fn eq_values_are_validated() {
        assert_eq!(bands_from(&Value::List(vec![1, -2, 1, -3, 1, -3, -5, 2, 2, 3])).unwrap()[6], (-5i8) as u8);
        assert!(bands_from(&Value::List(vec![0; 9])).is_err());
        assert!(bands_from(&Value::List(vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 99])).is_err());
        assert!(bands_from(&Value::Int(3)).is_err());
    }
}
