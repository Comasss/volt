//! Transporte y features básicas de HID++ 2.0.
//!
//! Formato de informe:
//! - Corto (0x10): 7 bytes  `[0x10, dev, feat_idx, fn<<4|swid, p0, p1, p2]`
//! - Largo (0x11): 20 bytes `[0x11, dev, feat_idx, fn<<4|swid, p0..p15]`
//!
//! En Windows cada longitud vive en una colección HID distinta de la misma interfaz
//! (usage page 0xFF00, usage 0x0001 = corto, 0x0002 = largo). Las peticiones 2.0 van
//! por la colección larga, pero los errores que genera el *receptor* (p. ej. "el
//! dispositivo está dormido") llegan como HID++ 1.0 por la corta, así que hay que
//! escuchar las dos.
//!
//! Referencias: Solaar (lib/logitech_receiver), libratbag (src/hidpp20.c).

use crate::model::{BatteryReading, ChargeState, DeviceKind};
use hidapi::HidDevice;
use std::time::{Duration, Instant};

pub const REPORT_SHORT: u8 = 0x10;
pub const REPORT_LONG: u8 = 0x11;
pub const REPORT_VERY_LONG: u8 = 0x12;
const SHORT_LEN: usize = 7;
const LONG_LEN: usize = 20;

/// Índice de dispositivo para algo conectado directamente (cable/BLE) o el propio receptor.
pub const DEVICE_DIRECT: u8 = 0xFF;

// Errores HID++ 1.0 (los genera el receptor).
pub const ERR10_INVALID_SUBID: u8 = 0x01;
pub const ERR10_CONNECT_FAIL: u8 = 0x04;
pub const ERR10_BUSY: u8 = 0x07;
pub const ERR10_UNKNOWN_DEVICE: u8 = 0x08;
pub const ERR10_RESOURCE: u8 = 0x09;

// Errores HID++ 2.0.
pub const ERR20_BUSY: u8 = 0x08;

// Features (IRoot 0x0000 siempre está en el índice 0).
pub const FEAT_DEVICE_NAME: u16 = 0x0005;
pub const FEAT_BATTERY_STATUS: u16 = 0x1000;
pub const FEAT_BATTERY_VOLTAGE: u16 = 0x1001;
pub const FEAT_UNIFIED_BATTERY: u16 = 0x1004;

pub const TIMEOUT: Duration = Duration::from_millis(1200);
pub const PING_TIMEOUT: Duration = Duration::from_millis(700);

#[derive(Debug)]
pub enum HidppError {
    Timeout,
    Hidpp10(u8),
    Hidpp20(u8),
    Io(String),
}

#[derive(Debug)]
pub enum Ping {
    Online { major: u8, minor: u8 },
    /// Emparejado pero inalcanzable (dormido/apagado).
    Offline,
    /// Nada emparejado en ese índice, o no contestó.
    Absent,
}

/// Un par de handles (corto/largo) abiertos sobre la misma interfaz HID++.
pub struct Channel {
    short: Option<HidDevice>,
    long: Option<HidDevice>,
    swid: u8,
}

impl Channel {
    pub fn new(short: Option<HidDevice>, long: Option<HidDevice>) -> Option<Self> {
        if short.is_none() && long.is_none() {
            return None;
        }
        Some(Self {
            short,
            long,
            swid: 0x0A,
        })
    }

    /// El software id (nibble bajo) distingue nuestras respuestas de las de G HUB
    /// u otras apps que hablen con el mismo receptor. 0 está reservado a eventos.
    fn next_swid(&mut self) -> u8 {
        self.swid = if self.swid >= 0x0F { 0x02 } else { self.swid + 1 };
        self.swid
    }

    /// Petición HID++ 2.0 a la feature `index`, función `func`.
    /// Devuelve los 16 bytes de parámetros de la respuesta.
    pub fn request(
        &mut self,
        device: u8,
        index: u8,
        func: u8,
        params: &[u8],
        timeout: Duration,
    ) -> Result<[u8; 16], HidppError> {
        let fn_sw = (func << 4) | self.next_swid();
        self.send(device, index, fn_sw, params)?;
        self.wait_response(device, index, fn_sw, timeout)
    }

    fn send(&self, device: u8, b2: u8, b3: u8, params: &[u8]) -> Result<(), HidppError> {
        let io = |e: hidapi::HidError| HidppError::Io(e.to_string());
        if let Some(dev) = &self.long {
            let mut buf = [0u8; LONG_LEN];
            buf[0] = REPORT_LONG;
            buf[1] = device;
            buf[2] = b2;
            buf[3] = b3;
            let n = params.len().min(LONG_LEN - 4);
            buf[4..4 + n].copy_from_slice(&params[..n]);
            log::trace!("hid++ tx {:02x?}", &buf);
            dev.write(&buf).map_err(io)?;
        } else if let Some(dev) = &self.short {
            if params.len() > SHORT_LEN - 4 {
                return Err(HidppError::Io("petición demasiado larga para informe corto".into()));
            }
            let mut buf = [0u8; SHORT_LEN];
            buf[0] = REPORT_SHORT;
            buf[1] = device;
            buf[2] = b2;
            buf[3] = b3;
            buf[4..4 + params.len()].copy_from_slice(params);
            dev.write(&buf).map_err(io)?;
        }
        Ok(())
    }

    /// Lee de ambas colecciones hasta encontrar la respuesta que casa con la
    /// petición. Las lecturas bloquean en el kernel (overlapped I/O + espera),
    /// no hay espera activa. Ignora notificaciones y respuestas ajenas.
    fn wait_response(
        &self,
        device: u8,
        index: u8,
        fn_sw: u8,
        timeout: Duration,
    ) -> Result<[u8; 16], HidppError> {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 64];
        let handles: Vec<&HidDevice> = [self.long.as_ref(), self.short.as_ref()]
            .into_iter()
            .flatten()
            .collect();
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(HidppError::Timeout);
            }
            let slice = (deadline - now).min(Duration::from_millis(25)).as_millis() as i32;
            for (i, dev) in handles.iter().enumerate() {
                // El primer handle espera; el resto solo se consulta sin bloquear.
                let t = if i == 0 { slice.max(1) } else { 0 };
                match dev.read_timeout(&mut buf, t) {
                    Ok(0) => {}
                    Ok(n) => {
                        log::trace!("hid++ rx {:02x?}", &buf[..n]);
                        if let Some(r) = match_response(&buf[..n], device, index, fn_sw) {
                            return r;
                        }
                    }
                    Err(e) => return Err(HidppError::Io(e.to_string())),
                }
            }
        }
    }
}

fn match_response(
    r: &[u8],
    device: u8,
    index: u8,
    fn_sw: u8,
) -> Option<Result<[u8; 16], HidppError>> {
    if r.len() < 4 || !matches!(r[0], REPORT_SHORT | REPORT_LONG | REPORT_VERY_LONG) {
        return None;
    }
    if r[1] != device {
        return None;
    }
    // Error HID++ 1.0: 10 dev 8F <sub_id> <addr> <code>
    if r[2] == 0x8F && r.len() >= 6 && r[3] == index && r[4] == fn_sw {
        return Some(Err(HidppError::Hidpp10(r[5])));
    }
    // Error HID++ 2.0: 11 dev FF <feat_idx> <fn|sw> <code>
    if r[2] == 0xFF && r.len() >= 6 && r[3] == index && r[4] == fn_sw {
        return Some(Err(HidppError::Hidpp20(r[5])));
    }
    if r[2] == index && r[3] == fn_sw {
        let mut out = [0u8; 16];
        let n = (r.len() - 4).min(16);
        out[..n].copy_from_slice(&r[4..4 + n]);
        return Some(Ok(out));
    }
    None
}

/// IRoot.getProtocolVersion (feature 0x0000, función 1) con byte de ping.
pub fn ping(ch: &mut Channel, device: u8) -> Result<Ping, HidppError> {
    const MARK: u8 = 0x5A;
    match ch.request(device, 0x00, 0x1, &[0, 0, MARK], PING_TIMEOUT) {
        Ok(p) => Ok(Ping::Online {
            major: p[0],
            minor: p[1],
        }),
        // Un dispositivo HID++ 1.0 no conoce la función: sigue siendo "online".
        Err(HidppError::Hidpp10(ERR10_INVALID_SUBID)) => Ok(Ping::Online { major: 1, minor: 0 }),
        Err(HidppError::Hidpp10(ERR10_RESOURCE | ERR10_CONNECT_FAIL)) => Ok(Ping::Offline),
        Err(HidppError::Hidpp10(ERR10_UNKNOWN_DEVICE)) | Err(HidppError::Timeout) => {
            Ok(Ping::Absent)
        }
        Err(e) => Err(e),
    }
}

/// IRoot.getFeature: índice de una feature en este dispositivo (None si no la tiene).
pub fn feature_index(ch: &mut Channel, device: u8, feature: u16) -> Result<Option<u8>, HidppError> {
    let p = ch.request(
        device,
        0x00,
        0x0,
        &[(feature >> 8) as u8, feature as u8],
        TIMEOUT,
    )?;
    Ok(if p[0] == 0 { None } else { Some(p[0]) })
}

/// Feature 0x0005 (DeviceNameType): nombre comercial del dispositivo.
pub fn device_name(ch: &mut Channel, device: u8, idx: u8) -> Result<String, HidppError> {
    let len = ch.request(device, idx, 0x0, &[], TIMEOUT)?[0] as usize;
    let mut bytes = Vec::with_capacity(len);
    while bytes.len() < len {
        let chunk = ch.request(device, idx, 0x1, &[bytes.len() as u8], TIMEOUT)?;
        let take = (len - bytes.len()).min(chunk.len());
        if take == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..take]);
    }
    let name = String::from_utf8_lossy(&bytes)
        .trim_matches(char::from(0))
        .trim()
        .to_string();
    Ok(name)
}

/// Feature 0x0005, función 2: tipo de dispositivo.
pub fn device_kind(ch: &mut Channel, device: u8, idx: u8) -> Result<DeviceKind, HidppError> {
    let t = ch.request(device, idx, 0x2, &[], TIMEOUT)?[0];
    Ok(match t {
        0 | 2 => DeviceKind::Keyboard,
        3 | 4 | 5 => DeviceKind::Mouse,
        8 | 15 => DeviceKind::Headset,
        11 | 12 => DeviceKind::Gamepad,
        _ => DeviceKind::Other,
    })
}

#[derive(Debug, Clone, Copy)]
pub enum BatteryFeature {
    Unified,
    Status,
    Voltage,
}

impl BatteryFeature {
    /// Por orden de preferencia.
    pub const ALL: [(BatteryFeature, u16); 3] = [
        (BatteryFeature::Unified, FEAT_UNIFIED_BATTERY),
        (BatteryFeature::Status, FEAT_BATTERY_STATUS),
        (BatteryFeature::Voltage, FEAT_BATTERY_VOLTAGE),
    ];
}

pub fn read_battery(
    ch: &mut Channel,
    device: u8,
    feature: BatteryFeature,
    idx: u8,
) -> Result<BatteryReading, HidppError> {
    match feature {
        BatteryFeature::Unified => read_unified(ch, device, idx),
        BatteryFeature::Status => read_status(ch, device, idx),
        BatteryFeature::Voltage => read_voltage(ch, device, idx),
    }
}

/// 0x1004 Unified Battery.
/// getCapabilities → [niveles soportados, flags (bit1 = porcentaje exacto)]
/// getStatus       → [porcentaje, nivel (bitmask), estado de carga, alimentación externa]
fn read_unified(ch: &mut Channel, device: u8, idx: u8) -> Result<BatteryReading, HidppError> {
    let caps = ch.request(device, idx, 0x0, &[], TIMEOUT)?;
    let has_soc = caps[1] & 0x02 != 0;
    let st = ch.request(device, idx, 0x1, &[], TIMEOUT)?;
    let (soc, level, charging) = (st[0], st[1], st[2]);

    let (percent, approximate) = if has_soc || soc > 0 {
        (Some(soc.min(100)), false)
    } else {
        let approx = match level {
            l if l & 0x08 != 0 => 90,
            l if l & 0x04 != 0 => 50,
            l if l & 0x02 != 0 => 20,
            l if l & 0x01 != 0 => 5,
            _ => 0,
        };
        (Some(approx), true)
    };
    let charge = match charging {
        0 => ChargeState::Discharging,
        1 | 2 => ChargeState::Charging,
        3 => ChargeState::Full,
        _ => ChargeState::Unknown,
    };
    Ok(BatteryReading {
        percent,
        approximate,
        charge,
        voltage_mv: None,
        measured_at: None,
    })
}

/// 0x1000 Battery Status: [porcentaje, siguiente nivel, estado]
fn read_status(ch: &mut Channel, device: u8, idx: u8) -> Result<BatteryReading, HidppError> {
    let p = ch.request(device, idx, 0x0, &[], TIMEOUT)?;
    let charge = match p[2] {
        0 => ChargeState::Discharging,
        1 | 2 | 4 => ChargeState::Charging,
        3 => ChargeState::Full,
        _ => ChargeState::Unknown,
    };
    let percent = if p[0] == 0 && charge != ChargeState::Discharging {
        None
    } else {
        Some(p[0].min(100))
    };
    Ok(BatteryReading {
        percent,
        approximate: false,
        charge,
        voltage_mv: None,
        measured_at: None,
    })
}

/// 0x1001 Battery Voltage: [mV hi, mV lo, flags]. El porcentaje se estima con la
/// curva de descarga típica de una celda Li-Po de 1S.
fn read_voltage(ch: &mut Channel, device: u8, idx: u8) -> Result<BatteryReading, HidppError> {
    let p = ch.request(device, idx, 0x0, &[], TIMEOUT)?;
    let mv = u16::from_be_bytes([p[0], p[1]]);
    let flags = p[2];
    let charge = if flags & 0x80 != 0 {
        match flags & 0x07 {
            0 => ChargeState::Charging,
            1 => ChargeState::Full,
            2 => ChargeState::Discharging,
            _ => ChargeState::Unknown,
        }
    } else {
        ChargeState::Discharging
    };
    Ok(BatteryReading {
        percent: Some(lipo_percent(mv)),
        approximate: true,
        charge,
        voltage_mv: Some(mv),
        measured_at: None,
    })
}

/// Interpolación lineal sobre una curva Li-Po 1S aproximada (mV, %).
fn lipo_percent(mv: u16) -> u8 {
    const CURVE: [(u16, u8); 12] = [
        (4180, 100),
        (4060, 90),
        (3980, 80),
        (3920, 70),
        (3860, 60),
        (3810, 50),
        (3780, 40),
        (3750, 30),
        (3720, 20),
        (3670, 10),
        (3600, 3),
        (3500, 0),
    ];
    if mv >= CURVE[0].0 {
        return 100;
    }
    for w in CURVE.windows(2) {
        let (hi_mv, hi_p) = w[0];
        let (lo_mv, lo_p) = w[1];
        if mv >= lo_mv {
            let span = (hi_mv - lo_mv) as f32;
            let t = (mv - lo_mv) as f32 / span;
            return (lo_p as f32 + t * (hi_p - lo_p) as f32).round() as u8;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lipo_curve_is_monotonic_and_bounded() {
        assert_eq!(lipo_percent(4300), 100);
        assert_eq!(lipo_percent(3400), 0);
        assert_eq!(lipo_percent(3810), 50);
        let mut last = 0;
        for mv in (3400..4300).step_by(5) {
            let p = lipo_percent(mv);
            assert!(p >= last, "{mv} mV → {p} < {last}");
            last = p;
        }
    }

    #[test]
    fn matches_success_and_errors() {
        // Respuesta larga a feat 0x00 fn 1 swid 0xB.
        let ok = [0x11, 0x01, 0x00, 0x1B, 4, 2, 0x5A, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let r = match_response(&ok, 0x01, 0x00, 0x1B).unwrap().unwrap();
        assert_eq!((r[0], r[1], r[2]), (4, 2, 0x5A));

        // Error 1.0 del receptor: dispositivo dormido.
        let off = [0x10, 0x01, 0x8F, 0x00, 0x1B, ERR10_RESOURCE, 0x00];
        assert!(matches!(
            match_response(&off, 0x01, 0x00, 0x1B),
            Some(Err(HidppError::Hidpp10(ERR10_RESOURCE)))
        ));

        // Notificación ajena (swid 0) → se ignora.
        let notif = [0x11, 0x01, 0x04, 0x00, 50, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(match_response(&notif, 0x01, 0x04, 0x1B).is_none());
    }
}
