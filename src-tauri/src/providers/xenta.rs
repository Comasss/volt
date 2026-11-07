//! Provider para receptores 2.4G con VID `1D57` (chip "Xenta"/Beken), usados por
//! ratones Attack Shark (X6, X7…) y otras marcas.
//!
//! Protocolo (sacado del driver oficial Attack Shark X6 por análisis estático de
//! `hiddriver.dll`, y verificado con un X7 en `1D57:FA60`):
//!
//! ```text
//! interfaz 2, colección con usage page 0x000A, informe de entrada de 5 bytes (ID 0x03)
//!   03 <código lo> <código hi> <valor lo> <valor hi>
//! código 0x4010 = batería:  valor lo = estado (1 batería, 2 lleno, 3 cargando)
//!                           valor hi = nivel 1..10 (×10 %)
//! ```
//!
//! El ratón **empuja** ese informe cada ~4 s mientras está encendido, así que no hay
//! que mandarle nada: se abre la colección, se espera un informe y se cierra. La
//! configuración (DPI, luces, macros) va por feature reports de 262 bytes en otra
//! colección; Volt no la toca.

use crate::device_ids::{self, Protocol};
use crate::hid::{interface_key, short_hash};
use crate::model::{BatteryReading, ChargeState, Connection, DeviceInfo, DeviceKind};
use crate::provider::{DeviceProvider, ListenEvent, ListenSpec, PollContext, ProviderError};
use std::collections::HashMap;
use std::ffi::CString;
use std::time::{Duration, Instant};

const VID: u16 = 0x1D57;
const USAGE_PAGE: u16 = 0x000A;
const REPORT_ID: u8 = 0x03;
const CODE_BATTERY: u16 = 0x4010;
/// El ratón manda el estado cada ~4 s; con 5 s basta para recibir uno.
const LISTEN: Duration = Duration::from_millis(5000);

pub struct XentaProvider {
    paths: HashMap<String, CString>,
}

impl XentaProvider {
    pub fn new() -> Self {
        Self {
            paths: HashMap::new(),
        }
    }
}

fn claims(vid: u16, pid: u16, usage_page: u16) -> bool {
    vid == VID
        && usage_page == USAGE_PAGE
        && device_ids::lookup(vid, pid).is_some_and(|k| k.protocol == Protocol::Xenta)
}

/// (estado, nivel) si `r` es un informe de batería.
fn parse(r: &[u8]) -> Option<(u8, u8)> {
    if r.len() < 5 || r[0] != REPORT_ID {
        return None;
    }
    (u16::from_le_bytes([r[1], r[2]]) == CODE_BATTERY).then_some((r[3], r[4]))
}

fn reading(state: u8, level: u8) -> Option<BatteryReading> {
    let charge = match state {
        3 => ChargeState::Charging,
        2 => ChargeState::Full,
        _ => ChargeState::Discharging,
    };
    let percent = match (state, level) {
        (2, _) => 100,
        (_, 1..=10) => level * 10,
        // Cargando sin nivel válido: se informa la carga sin cifra.
        (3, _) => {
            return Some(BatteryReading {
                percent: None,
                approximate: true,
                charge,
                voltage_mv: None,
                measured_at: None,
            })
        }
        _ => return None,
    };
    Some(BatteryReading {
        percent: Some(percent),
        approximate: true, // pasos de 10 %
        charge,
        voltage_mv: None,
        measured_at: None,
    })
}

impl DeviceProvider for XentaProvider {
    fn id(&self) -> &'static str {
        "xenta"
    }

    fn claims_hid(&self, vid: u16, pid: u16, up: u16, _u: u16) -> bool {
        claims(vid, pid, up)
    }

    fn discover(&mut self, ctx: &PollContext) -> Result<Vec<DeviceInfo>, ProviderError> {
        self.paths.clear();
        let Some(hid) = ctx.hid else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for e in hid
            .devices
            .iter()
            .filter(|e| claims(e.vid, e.pid, e.usage_page))
        {
            let known = device_ids::lookup(e.vid, e.pid);
            let id = format!("xenta:{:04x}:{}", e.pid, short_hash(&interface_key(e)));
            out.push(DeviceInfo {
                id: id.clone(),
                // El receptor no dice el modelo: la huella es el PID (y el usuario pone el nombre).
                fingerprint: format!("xenta:{:04x}", e.pid),
                provider: "xenta",
                vendor: "Xenta".into(),
                name: crate::i18n::tr("RATÓN 2.4G", "2.4G MOUSE").into(),
                name_from_device: false,
                model: format!("{VID:04X}:{:04X}", e.pid),
                kind: known.map(|k| k.kind).unwrap_or(DeviceKind::Mouse),
                // FA61 es el ratón conectado por cable (según el driver del X6).
                connection: if e.pid == 0xFA61 {
                    Connection::Wired
                } else {
                    Connection::Receiver
                },
                via: (e.pid != 0xFA61).then(|| format!("RECEPTOR 2.4G {:04X}", e.pid)),
                slot: None,
            });
            self.paths.insert(id, e.path.clone());
        }
        Ok(out)
    }

    /// El ratón empuja su estado cada ~4 s: con la escucha activa, un cambio a
    /// "cargando" se ve al instante y el sondeo no tiene que esperar el informe.
    fn listener(&self, device: &DeviceInfo, _current: Option<&BatteryReading>) -> Option<ListenSpec> {
        Some(ListenSpec {
            path: self.paths.get(&device.id)?.clone(),
            parser: Box::new(|r| parse(r).and_then(|(s, l)| reading(s, l)).map(ListenEvent::Reading)),
            // Informa cada ~4 s: tres informes perdidos = ratón apagado.
            silence: Some(Duration::from_secs(12)),
        })
    }

    fn battery(
        &mut self,
        ctx: &PollContext,
        device: &DeviceInfo,
    ) -> Result<BatteryReading, ProviderError> {
        let hid = ctx.hid.ok_or(ProviderError::NoResponse)?;
        let path = self.paths.get(&device.id).ok_or(ProviderError::NoResponse)?;
        let dev = hid
            .open(path)
            .map_err(|e| ProviderError::Busy(format!("no se pudo abrir ({e})")))?;
        // Escucha pasiva: la lectura bloquea en el kernel hasta que llega un informe.
        let deadline = Instant::now() + LISTEN;
        let mut buf = [0u8; 16];
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match dev.read_timeout(&mut buf, left.as_millis().max(1) as i32) {
                Ok(0) => {}
                Ok(n) => {
                    log::trace!("xenta rx {:02x?}", &buf[..n]);
                    if let Some((state, level)) = parse(&buf[..n]) {
                        return reading(state, level).ok_or(ProviderError::Protocol(format!(
                            "estado {state} nivel {level}"
                        )));
                    }
                }
                Err(e) => return Err(ProviderError::Io(e.to_string())),
            }
        }
        // Sin informes: ratón apagado o dormido.
        Err(ProviderError::Offline { last: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_report_captured_from_x7() {
        // Capturado de un Attack Shark X7 al 100 %.
        let (state, level) = parse(&[0x03, 0x10, 0x40, 0x01, 0x0A]).unwrap();
        let r = reading(state, level).unwrap();
        assert_eq!((r.percent, r.charge), (Some(100), ChargeState::Discharging));
        // Otros códigos (p. ej. 0x1010 = índice de DPI) no son batería.
        assert!(parse(&[0x03, 0x10, 0x10, 0x02, 0x00]).is_none());
        // Cargando.
        let r = reading(3, 4).unwrap();
        assert_eq!((r.percent, r.charge), (Some(40), ChargeState::Charging));
    }
}
