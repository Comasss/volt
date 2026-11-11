//! Ajustes de ratones Logitech por HID++ 2.0: DPI, tasa de sondeo y modo de
//! memoria interna.
//!
//! Features (el dispositivo anuncia cuáles tiene; solo se ofrecen esas):
//!
//! ```text
//! 0x2201 Adjustable DPI            fn1 lista · fn2 leer · fn3 escribir
//! 0x2202 Extended Adjustable DPI   fn1 capacidades · fn2 rangos · fn5 leer · fn6 escribir
//!                                  (PRO X SUPERLIGHT 2, G502 X…: DPI en X/Y y LOD)
//! 0x8060 Report Rate               fn0 lista (bit n = n+1 ms) · fn1 leer · fn2 escribir
//! 0x8061 Extended Report Rate      fn1 lista · fn2 leer · fn3 escribir (hasta 8 kHz)
//! 0x8100 Onboard Profiles          fn1 escribir modo · fn2 leer modo (1 interna, 2 software)
//! ```
//!
//! Referencia: Solaar (`lib/logitech_receiver/settings_templates.py`, clases
//! `AdjustableDpi`, `ExtendedAdjustableDpi`, `ReportRate`, `ExtendedReportRate`,
//! `OnboardProfiles`), que lo usa con estos ratones en Linux. No se ha probado
//! desde Volt con hardware: por eso tras cada escritura se relee el valor.

use super::hidpp::{self, Channel, HidppError, TIMEOUT};

pub const FEAT_ADJUSTABLE_DPI: u16 = 0x2201;
pub const FEAT_EXT_DPI: u16 = 0x2202;
pub const FEAT_REPORT_RATE: u16 = 0x8060;
pub const FEAT_EXT_REPORT_RATE: u16 = 0x8061;
pub const FEAT_ONBOARD_PROFILES: u16 = 0x8100;

pub const MODE_ONBOARD: u8 = 1;
pub const MODE_HOST: u8 = 2;

/// Índices de las features de ajustes en un dispositivo (se resuelven una vez).
#[derive(Debug, Clone, Copy, Default)]
pub struct Features {
    pub dpi: Option<u8>,
    pub ext_dpi: Option<u8>,
    pub rate: Option<u8>,
    pub ext_rate: Option<u8>,
    pub onboard: Option<u8>,
}

impl Features {
    pub fn resolve(ch: &mut Channel, device: u8) -> Result<Self, HidppError> {
        Ok(Self {
            dpi: hidpp::feature_index(ch, device, FEAT_ADJUSTABLE_DPI)?,
            ext_dpi: hidpp::feature_index(ch, device, FEAT_EXT_DPI)?,
            rate: hidpp::feature_index(ch, device, FEAT_REPORT_RATE)?,
            ext_rate: hidpp::feature_index(ch, device, FEAT_EXT_REPORT_RATE)?,
            onboard: hidpp::feature_index(ch, device, FEAT_ONBOARD_PROFILES)?,
        })
    }

    pub fn any(&self) -> bool {
        self.has_dpi() || self.has_rate() || self.onboard.is_some()
    }

    pub fn has_dpi(&self) -> bool {
        self.dpi.is_some() || self.ext_dpi.is_some()
    }

    pub fn has_rate(&self) -> bool {
        self.rate.is_some() || self.ext_rate.is_some()
    }
}

/// Tramo de valores de DPI válidos: de `from` a `to` de `step` en `step`
/// (`step` 0 = un único valor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seg {
    pub from: u16,
    pub to: u16,
    pub step: u16,
}

/// Decodifica la lista de DPI: valores u16 big-endian terminados en 0. Un valor
/// con los tres bits altos a 1 (`0xE000 | paso`) significa «desde el anterior
/// hasta el siguiente, de `paso` en `paso`».
pub fn parse_dpi_list(bytes: &[u8]) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let val = u16::from_be_bytes([bytes[i], bytes[i + 1]]);
        if val == 0 {
            break;
        }
        if val >> 13 == 0b111 {
            let step = val & 0x1FFF;
            let last = bytes.get(i + 2..i + 4).map(|b| u16::from_be_bytes([b[0], b[1]]));
            match (out.last_mut(), last) {
                (Some(seg), Some(last)) if step > 0 && last > seg.to => {
                    if seg.step == 0 {
                        // El valor anterior era el inicio de este tramo.
                        seg.step = step;
                        seg.to = last;
                    } else {
                        // Tramo encadenado con otro paso: empieza donde acabó el anterior.
                        let from = seg.to;
                        out.push(Seg { from, to: last, step });
                    }
                }
                // Lista mal formada: nos quedamos con lo decodificado hasta aquí.
                _ => break,
            }
            i += 4;
        } else {
            out.push(Seg { from: val, to: val, step: 0 });
            i += 2;
        }
    }
    out
}

pub fn dpi_min(segs: &[Seg]) -> Option<u16> {
    segs.iter().map(|s| s.from).min()
}

pub fn dpi_max(segs: &[Seg]) -> Option<u16> {
    segs.iter().map(|s| s.to).max()
}

/// Paso más fino de la lista (para el control de la interfaz).
pub fn dpi_step(segs: &[Seg]) -> u16 {
    segs.iter().map(|s| s.step).filter(|&s| s > 0).min().unwrap_or(0)
}

/// El valor válido más cercano a `want`.
pub fn dpi_snap(segs: &[Seg], want: u32) -> Option<u16> {
    segs.iter()
        .map(|s| {
            let w = want.clamp(s.from as u32, s.to as u32);
            if s.step == 0 {
                return s.from;
            }
            let k = (w - s.from as u32 + s.step as u32 / 2) / s.step as u32;
            (s.from as u32 + k * s.step as u32).min(s.to as u32) as u16
        })
        .min_by_key(|&v| (v as i64 - want as i64).abs())
}

#[derive(Debug, Clone)]
pub struct Dpi {
    pub segs: Vec<Seg>,
    pub current: u16,
    /// Solo 0x2202: DPI vertical independiente (Volt lo iguala al horizontal).
    y: bool,
    /// Solo 0x2202: distancia de despegue actual, que se conserva al escribir.
    lod: Option<u8>,
}

/// Lee la lista de valores válidos y el DPI actual del sensor 0.
pub fn read_dpi(ch: &mut Channel, device: u8, f: &Features) -> Result<Option<Dpi>, HidppError> {
    if let Some(idx) = f.ext_dpi {
        let caps = ch.request(device, idx, 0x1, &[0], TIMEOUT)?;
        let (y, has_lod) = (caps[2] & 0x01 != 0, caps[2] & 0x02 != 0);
        // Los rangos llegan troceados: 13 bytes por respuesta tras 3 de cabecera.
        let mut bytes = Vec::new();
        for page in 0..8u8 {
            let r = ch.request(device, idx, 0x2, &[0, 0, page], TIMEOUT)?;
            bytes.extend_from_slice(&r[3..]);
            if bytes.ends_with(&[0, 0]) {
                break;
            }
        }
        let p = ch.request(device, idx, 0x5, &[0], TIMEOUT)?;
        let pick = |cur: [u8; 2], def: [u8; 2]| match u16::from_be_bytes(cur) {
            0 => u16::from_be_bytes(def),
            v => v,
        };
        return Ok(Some(Dpi {
            segs: parse_dpi_list(&bytes),
            current: pick([p[1], p[2]], [p[3], p[4]]),
            y,
            lod: has_lod.then_some(p[9]),
        }));
    }
    if let Some(idx) = f.dpi {
        let list = ch.request(device, idx, 0x1, &[0], TIMEOUT)?;
        let p = ch.request(device, idx, 0x2, &[0], TIMEOUT)?;
        let current = match u16::from_be_bytes([p[1], p[2]]) {
            0 => u16::from_be_bytes([p[3], p[4]]), // 0 = usa el valor por defecto
            v => v,
        };
        return Ok(Some(Dpi {
            segs: parse_dpi_list(&list[1..]),
            current,
            y: false,
            lod: None,
        }));
    }
    Ok(None)
}

pub fn write_dpi(ch: &mut Channel, device: u8, f: &Features, dpi: &Dpi, value: u16) -> Result<(), HidppError> {
    let [hi, lo] = value.to_be_bytes();
    if let Some(idx) = f.ext_dpi {
        let (yh, yl) = if dpi.y { (hi, lo) } else { (0, 0) };
        ch.request(device, idx, 0x6, &[0, hi, lo, yh, yl, dpi.lod.unwrap_or(0)], TIMEOUT)?;
    } else if let Some(idx) = f.dpi {
        ch.request(device, idx, 0x3, &[0, hi, lo], TIMEOUT)?;
    }
    Ok(())
}

/// Tasas del 0x8061 por índice, en Hz (8 ms … 125 µs).
const EXT_RATES_HZ: [u32; 7] = [125, 250, 500, 1000, 2000, 4000, 8000];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rate {
    /// Tasas que admite el dispositivo, en Hz, de menor a mayor.
    pub options: Vec<u32>,
    pub current: Option<u32>,
}

pub fn rates_from_ms_flags(flags: u8) -> Vec<u32> {
    let mut v: Vec<u32> = (0..8).filter(|i| flags >> i & 1 != 0).map(|i| 1000 / (i + 1)).collect();
    v.sort_unstable();
    v
}

pub fn rates_from_ext_flags(flags: u16) -> Vec<u32> {
    (0..EXT_RATES_HZ.len()).filter(|i| flags >> i & 1 != 0).map(|i| EXT_RATES_HZ[i]).collect()
}

pub fn read_rate(ch: &mut Channel, device: u8, f: &Features) -> Result<Option<Rate>, HidppError> {
    if let Some(idx) = f.ext_rate {
        let list = ch.request(device, idx, 0x1, &[], TIMEOUT)?;
        let cur = ch.request(device, idx, 0x2, &[], TIMEOUT)?[0] as usize;
        return Ok(Some(Rate {
            options: rates_from_ext_flags(u16::from_be_bytes([list[0], list[1]])),
            current: EXT_RATES_HZ.get(cur).copied(),
        }));
    }
    if let Some(idx) = f.rate {
        let flags = ch.request(device, idx, 0x0, &[], TIMEOUT)?[0];
        let ms = ch.request(device, idx, 0x1, &[], TIMEOUT)?[0] as u32;
        return Ok(Some(Rate {
            options: rates_from_ms_flags(flags),
            current: (1..=8).contains(&ms).then(|| 1000 / ms),
        }));
    }
    Ok(None)
}

pub fn write_rate(ch: &mut Channel, device: u8, f: &Features, hz: u32) -> Result<(), HidppError> {
    if let Some(idx) = f.ext_rate {
        if let Some(i) = EXT_RATES_HZ.iter().position(|&r| r == hz) {
            ch.request(device, idx, 0x3, &[i as u8], TIMEOUT)?;
        }
    } else if let Some(idx) = f.rate {
        ch.request(device, idx, 0x2, &[(1000 / hz.max(1)) as u8], TIMEOUT)?;
    }
    Ok(())
}

/// Modo actual: `MODE_ONBOARD` (manda el perfil guardado en el ratón) o
/// `MODE_HOST` (manda el software).
pub fn read_mode(ch: &mut Channel, device: u8, f: &Features) -> Result<Option<u8>, HidppError> {
    match f.onboard {
        Some(idx) => Ok(Some(ch.request(device, idx, 0x2, &[], TIMEOUT)?[0])),
        None => Ok(None),
    }
}

pub fn write_mode(ch: &mut Channel, device: u8, f: &Features, mode: u8) -> Result<(), HidppError> {
    if let Some(idx) = f.onboard {
        ch.request(device, idx, 0x1, &[mode], TIMEOUT)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_range_and_discrete_lists() {
        // 100..=25600 de 50 en 50 (formato típico del sensor HERO).
        let hero = [0x00, 0x64, 0xE0, 0x32, 0x64, 0x00, 0x00, 0x00];
        let segs = parse_dpi_list(&hero);
        assert_eq!(segs, vec![Seg { from: 100, to: 25600, step: 50 }]);
        assert_eq!((dpi_min(&segs), dpi_max(&segs), dpi_step(&segs)), (Some(100), Some(25600), 50));

        // Lista de valores sueltos.
        let fixed = [0x01, 0x90, 0x03, 0x20, 0x06, 0x40, 0x00, 0x00];
        let segs = parse_dpi_list(&fixed);
        assert_eq!(segs.len(), 3);
        assert_eq!((dpi_min(&segs), dpi_max(&segs), dpi_step(&segs)), (Some(400), Some(1600), 0));
        assert_eq!(dpi_snap(&segs, 900), Some(800));
        assert_eq!(dpi_snap(&segs, 5000), Some(1600));
    }

    #[test]
    fn parses_chained_ranges_with_different_steps() {
        // 100..=200 de 1 en 1, luego hasta 32000 de 50 en 50.
        let b = [0x00, 0x64, 0xE0, 0x01, 0x00, 0xC8, 0xE0, 0x32, 0x7D, 0x00, 0x00, 0x00];
        let segs = parse_dpi_list(&b);
        assert_eq!(
            segs,
            vec![Seg { from: 100, to: 200, step: 1 }, Seg { from: 200, to: 32000, step: 50 }]
        );
        assert_eq!(dpi_snap(&segs, 151), Some(151));
        assert_eq!(dpi_snap(&segs, 1620), Some(1600));
        assert_eq!(dpi_snap(&segs, 1630), Some(1650));
        assert_eq!(dpi_snap(&segs, 99_999), Some(32000));
        assert_eq!(dpi_snap(&segs, 0), Some(100));
    }

    #[test]
    fn malformed_lists_do_not_panic() {
        assert!(parse_dpi_list(&[]).is_empty());
        assert!(parse_dpi_list(&[0xE0, 0x32, 0x64, 0x00]).is_empty()); // paso sin valor previo
        assert_eq!(parse_dpi_list(&[0x00, 0x64, 0xE0, 0x32]).len(), 1); // paso sin final
        assert_eq!(parse_dpi_list(&[0x00, 0x64, 0xE0, 0x32, 0x00, 0x32]).len(), 1); // final menor
        assert_eq!(dpi_snap(&[], 800), None);
    }

    #[test]
    fn report_rate_flags() {
        // 0x8060: bits 0, 1, 3 y 7 → 1, 2, 4 y 8 ms.
        assert_eq!(rates_from_ms_flags(0b1000_1011), vec![125, 250, 500, 1000]);
        // 0x8061: índices 3..=6 → 1, 2, 4 y 8 kHz.
        assert_eq!(rates_from_ext_flags(0b0111_1000), vec![1000, 2000, 4000, 8000]);
    }
}
