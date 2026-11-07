//! Protocolo "Audio MXIC" de los auriculares Razer HyperSpeed recientes
//! (BlackShark V2 Pro 2023, `1532:0555`).
//!
//! Fuentes: rzr / razer-device-control (MIT, docs/PROTOCOL.md) y el driver de
//! OpenRazer para este modelo (openrazer/openrazer#2862), ambos verificados en hardware.
//!
//! Transporte: colección vendor `FF00:0001` de la interfaz 3, informes de 64 bytes
//! con ID `0x02` en ambos sentidos (no hay feature reports, por eso el sondeo de
//! 90 bytes de openrazer no lo encuentra).
//!
//! ```text
//! petición  [0]=02 [1]=80 [2]=8+len [5..7]="PA" [7]=inner_len [9]=tipo [10]=cmd [11]=flag [12]=len [13..]=datos
//! respuesta [0]=02 [1]=total, luego tramas "PI": 'P' 'I' + 6 bytes + tamaño (u16 LE) + cuerpo
//!           cuerpo = cmd, flag (01 = respuesta, 02 = evento), len, datos
//! ```
//!
//! Peculiaridades del firmware: el enlace "dormita" tras ~0,3 s sin tráfico y se
//! traga el primer paquete, así que cada secuencia empieza con un `remote mode` de
//! sacrificio; al acabar se devuelve el control al auricular (`remote mode` = 0).

use crate::model::{BatteryReading, ChargeState};
use crate::provider::ListenEvent;
use hidapi::HidDevice;
use std::thread::sleep;
use std::time::{Duration, Instant};

pub const PIDS: &[u16] = &[0x0555];
pub const USAGE_PAGE: u16 = 0xFF00;

const LEN: usize = 64;
const REPORT_ID: u8 = 0x02;
const TYPE_REMOTE: u8 = 0x02;
const TYPE_QUERY: u8 = 0x03;
const CMD_REMOTE: u8 = 0xE1;
const CMD_BATTERY: u8 = 0x21;
const CMD_CHARGING: u8 = 0x2A;
const FLAG_ACK: u8 = 0x01;

const GAP: Duration = Duration::from_millis(35);
/// Por intento; debe quedar por debajo del umbral en que el enlace se duerme.
const REPLY_TIMEOUT: Duration = Duration::from_millis(150);
const ATTEMPTS: usize = 3;

#[derive(Debug)]
pub enum MxicError {
    Io(String),
    /// Sin respuesta: auricular apagado o fuera de alcance.
    NoReply,
}

fn frame(total_len: u8, inner_len: u8, cmd_type: u8, cmd: u8, rest: &[u8]) -> [u8; LEN] {
    let mut p = [0u8; LEN];
    p[0] = REPORT_ID;
    p[1] = 0x80;
    p[2] = total_len;
    p[5] = b'P';
    p[6] = b'A';
    p[7] = inner_len;
    p[9] = cmd_type;
    p[10] = cmd;
    p[11..11 + rest.len()].copy_from_slice(rest);
    p
}

fn remote(on: bool) -> [u8; LEN] {
    frame(0x07, 0x0E, TYPE_REMOTE, CMD_REMOTE, &[on as u8])
}

fn query(cmd: u8) -> [u8; LEN] {
    frame(0x08, 0x08, TYPE_QUERY, cmd, &[0x00, 0x00])
}

/// Datos de la respuesta (flag 01) a `cmd`, recorriendo las tramas "PI" por su
/// tamaño de cuerpo (algunas no llevan byte de longitud).
fn parse_reply(buf: &[u8], cmd: u8) -> Option<Vec<u8>> {
    find_frame(buf, cmd, FLAG_ACK)
}

const FLAG_EVENT: u8 = 0x02;

const CMD_LINK: u8 = 0x20;

/// Escucha de eventos del auricular (flag 02): `21` batería, `2A` carga y `20`
/// enlace (0 = auricular apagado o fuera de alcance, 1 = vuelve). `current` da el
/// valor inicial para combinar batería y carga.
pub fn event_parser(
    current: Option<&BatteryReading>,
) -> impl FnMut(&[u8]) -> Option<ListenEvent> + Send + 'static {
    let mut level = current.and_then(|r| r.percent);
    let mut charging = current.map(|r| r.is_charging());
    move |buf: &[u8]| {
        if let Some(d) = find_frame(buf, CMD_LINK, FLAG_EVENT) {
            return Some(if d.first() == Some(&1) {
                ListenEvent::Refresh
            } else {
                ListenEvent::Gone
            });
        }
        let mut changed = false;
        if let Some(d) = find_frame(buf, CMD_BATTERY, FLAG_EVENT) {
            level = d.first().map(|&l| l.min(100));
            changed = true;
        }
        if let Some(d) = find_frame(buf, CMD_CHARGING, FLAG_EVENT) {
            charging = d.first().map(|&c| c != 0);
            changed = true;
        }
        if !changed {
            return None;
        }
        let charge = match charging {
            Some(true) => ChargeState::Charging,
            Some(false) => ChargeState::Discharging,
            None => ChargeState::Unknown,
        };
        Some(ListenEvent::Reading(BatteryReading {
            percent: level,
            approximate: false,
            charge,
            voltage_mv: None,
            measured_at: None,
        }))
    }
}

fn find_frame(buf: &[u8], cmd: u8, flag: u8) -> Option<Vec<u8>> {
    let (mut off, end) = if buf.first() == Some(&REPORT_ID) && buf.len() > 1 {
        (2, (2 + buf[1] as usize).min(buf.len()))
    } else {
        (1, buf.len())
    };
    while off + 12 <= end && buf[off] == b'P' && buf[off + 1] == b'I' {
        let size = u16::from_le_bytes([buf[off + 8], buf[off + 9]]) as usize;
        let body = &buf[off + 10..(off + 10 + size).min(end)];
        if body.len() >= 2 && body[0] == cmd && body[1] == flag {
            let data = body.get(3..).unwrap_or(&[]);
            let len = body.get(2).map_or(0, |&l| l as usize).min(data.len());
            return Some(data[..len].to_vec());
        }
        off += 10 + size;
    }
    None
}

fn send(dev: &HidDevice, pkt: &[u8; LEN]) -> Result<(), MxicError> {
    log::trace!("mxic tx {:02x?}", &pkt[..14]);
    dev.write(pkt).map_err(|e| MxicError::Io(e.to_string()))?;
    Ok(())
}

fn read_reply(dev: &HidDevice, cmd: u8) -> Result<Option<Vec<u8>>, MxicError> {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    let mut buf = [0u8; LEN];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        match dev.read_timeout(&mut buf, left.as_millis().max(1) as i32) {
            Ok(0) => {}
            Ok(n) => {
                log::trace!("mxic rx {:02x?}", &buf[..n.min(32)]);
                if let Some(d) = parse_reply(&buf[..n], cmd) {
                    return Ok(Some(d));
                }
            }
            Err(e) => return Err(MxicError::Io(e.to_string())),
        }
    }
}

fn query_bytes(dev: &HidDevice, cmd: u8) -> Result<Option<Vec<u8>>, MxicError> {
    for _ in 0..ATTEMPTS {
        send(dev, &remote(true))?;
        sleep(GAP);
        send(dev, &query(cmd))?;
        if let Some(d) = read_reply(dev, cmd)? {
            return Ok(Some(d));
        }
    }
    Ok(None)
}

fn query_u8(dev: &HidDevice, cmd: u8) -> Result<Option<u8>, MxicError> {
    Ok(query_bytes(dev, cmd)?.and_then(|d| d.first().copied()))
}

const TYPE_AUDIO: u8 = 0x04;
/// "Set Speaker Preset EQ Status". Synapse escribe 0 al arrancar; al cerrarse el
/// registro (lectura 0x1E) vuelve a 1. Es lo único que cambia en el auricular.
pub const CMD_PRESET_EQ_STATUS: u8 = 0x9E;
const CMD_PRESET_EQ_STATUS_GET: u8 = 0x1E;

/// Escritura de un byte como la hace Synapse: remote ×2, valor, devolver control.
/// Devuelve el valor leído después (None si el auricular no contesta).
pub fn write_value(dev: &HidDevice, cmd: u8, value: u8) -> Result<Option<u8>, MxicError> {
    let mut buf = [0u8; LEN];
    while matches!(dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
    send(dev, &remote(true))?;
    sleep(GAP);
    send(dev, &remote(true))?;
    sleep(GAP);
    send(dev, &frame(0x09, 0x08, TYPE_AUDIO, cmd, &[0x00, 0x01, value]))?;
    sleep(GAP);
    let back = if cmd == CMD_PRESET_EQ_STATUS {
        query_u8(dev, CMD_PRESET_EQ_STATUS_GET)
    } else {
        Ok(None)
    };
    let _ = send(dev, &remote(false));
    sleep(GAP);
    back
}

/// Diagnóstico de solo lectura: preset de EQ activo, familia, curva y registro 0x1E.
pub fn read_audio_state(dev: &HidDevice) -> Result<String, MxicError> {
    let mut buf = [0u8; LEN];
    while matches!(dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}
    send(dev, &remote(true))?;
    sleep(GAP);
    let fmt = |r: Result<Option<Vec<u8>>, MxicError>| match r {
        Ok(Some(v)) => format!("{:?}", v.iter().map(|&b| b as i8).collect::<Vec<_>>()),
        Ok(None) => "sin respuesta".into(),
        Err(e) => format!("{e:?}"),
    };
    let preset = match query_u8(dev, 0x13) {
        Ok(Some(s)) => format!(
            "{s:02X} ({})",
            match s {
                0x07 => "GAME",
                0x08 => "MUSIC",
                0x09 => "MOVIE",
                0xFF => "CUSTOM",
                0xFA..=0xFE => "ESPORTS",
                _ => "?",
            }
        ),
        other => fmt(other.map(|o| o.map(|b| vec![b]))),
    };
    let text = format!(
        "preset {preset} · curva {} · 0x1E {} · sidetone {} · nivel sidetone {}",
        fmt(query_bytes(dev, 0x15)),
        fmt(query_bytes(dev, 0x1E)),
        fmt(query_bytes(dev, 0x18)),
        fmt(query_bytes(dev, 0x19)),
    );
    let _ = send(dev, &remote(false));
    sleep(GAP);
    Ok(text)
}

/// Lee batería y carga. Con `keep_eq`, en la misma sesión comprueba el registro
/// de EQ y, si el auricular lo ha devuelto a 1 (Synapse cerrado), lo pone a 0 como
/// hace Synapse al arrancar. Quien llama decide `keep_eq` (ajuste + Synapse cerrado).
pub fn read_battery(dev: &HidDevice, keep_eq: bool) -> Result<BatteryReading, MxicError> {
    // Descartar respuestas viejas (p. ej. de Synapse) que esperan en el búfer.
    let mut buf = [0u8; LEN];
    while matches!(dev.read_timeout(&mut buf, 0), Ok(n) if n > 0) {}

    send(dev, &remote(true))?; // despertador: puede perderse
    sleep(GAP);
    let level = query_u8(dev, CMD_BATTERY);
    let charging = match level {
        Ok(Some(_)) => query_u8(dev, CMD_CHARGING).ok().flatten(),
        _ => None,
    };
    if keep_eq && matches!(level, Ok(Some(_))) {
        if let Ok(Some(status)) = query_u8(dev, CMD_PRESET_EQ_STATUS_GET) {
            if status != 0 {
                log::info!("blackshark: registro de EQ {status} → 0 (Synapse cerrado)");
                let _ = send(dev, &remote(true));
                sleep(GAP);
                let _ = send(dev, &frame(0x09, 0x08, TYPE_AUDIO, CMD_PRESET_EQ_STATUS, &[0x00, 0x01, 0]));
                sleep(GAP);
            }
        }
    }
    // Devolver siempre el control al auricular, aunque algo haya fallado.
    let _ = send(dev, &remote(false));
    sleep(GAP);

    let level = level?.ok_or(MxicError::NoReply)?;
    let charge = match charging {
        Some(0) => ChargeState::Discharging,
        Some(_) => ChargeState::Charging,
        None => ChargeState::Unknown,
    };
    Ok(BatteryReading::percent(level.min(100), charge))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_match_documented_bytes() {
        let q = query(CMD_BATTERY);
        assert_eq!(&q[..13], &[0x02, 0x80, 0x08, 0, 0, 0x50, 0x41, 0x08, 0, 0x03, 0x21, 0, 0]);
        let r = remote(true);
        assert_eq!(&r[..12], &[0x02, 0x80, 0x07, 0, 0, 0x50, 0x41, 0x0E, 0, 0x02, 0xE1, 0x01]);
    }

    #[test]
    fn parses_battery_reply_from_synapse_capture() {
        // getBatteryStatus → 55 % (captura de Synapse 4 publicada por rzr).
        let mut r = vec![
            0x02, 0x0E, 0x50, 0x49, 0x08, 0xD5, 0xCD, 0x1D, 0x00, 0x00, 0x04, 0x00, 0x21, 0x01,
            0x01, 0x37, 0x49, 0x01, 0xC0, 0x22,
        ];
        r.resize(64, 0);
        assert_eq!(parse_reply(&r, CMD_BATTERY), Some(vec![55]));
        assert_eq!(parse_reply(&r, CMD_CHARGING), None);
    }

    #[test]
    fn charging_event_updates_reading() {
        // Evento 2A flag 02 = 1 (capturado al conectar el cable), con nivel previo 71 %.
        let prev = BatteryReading::percent(71, ChargeState::Discharging);
        let mut p = event_parser(Some(&prev));
        let mut ev = vec![
            0x02, 0x0E, 0x50, 0x49, 0x08, 0xD0, 0x79, 0x33, 0x2E, 0x00, 0x04, 0x00, 0x2A, 0x02,
            0x01, 0x01,
        ];
        ev.resize(64, 0);
        let Some(ListenEvent::Reading(r)) = p(&ev) else { panic!("sin lectura") };
        assert_eq!((r.percent, r.charge), (Some(71), ChargeState::Charging));
        // Una respuesta (flag 01) no es un evento.
        let mut ack = ev.clone();
        ack[13] = 0x01;
        assert!(p(&ack).is_none());

        // Enlace caído (capturado por rzr): E3 00 + evento 20 = 0 → Gone.
        let mut drop = vec![
            0x02, 0x1A, 0x50, 0x49, 0x0E, 0xC4, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0xE3, 0x00,
            0x50, 0x49, 0x08, 0xC5, 0x59, 0x28, 0x15, 0x00, 0x04, 0x00, 0x20, 0x02, 0x01, 0x00,
        ];
        drop.resize(64, 0);
        assert_eq!(p(&drop), Some(ListenEvent::Gone));
    }
}
