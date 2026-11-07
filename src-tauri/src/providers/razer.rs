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
use crate::device_ids;
use crate::hid::HidEntry;
use crate::model::{BatteryReading, ChargeState, Connection, DeviceInfo, DeviceKind};
use crate::provider::{DeviceProvider, ListenSpec, PollContext, ProviderError};
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
}

fn build(tid: u8, class: u8, id: u8, size: u8) -> [u8; LEN + 1] {
    let mut b = [0u8; LEN + 1]; // b[0] = report id 0
    let r = &mut b[1..];
    r[1] = tid;
    r[5] = size;
    r[6] = class;
    r[7] = id;
    r[88] = r[2..88].iter().fold(0, |a, x| a ^ x);
    b
}

fn query(dev: &HidDevice, tid: u8, class: u8, id: u8, size: u8) -> Result<[u8; LEN], RzError> {
    let req = build(tid, class, id, size);
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
        let b = build(0x1F, 0x07, 0x80, 0x02);
        let r = &b[1..];
        assert_eq!(r[1], 0x1F);
        assert_eq!(r[88], 0x02 ^ 0x07 ^ 0x80);
    }
}
