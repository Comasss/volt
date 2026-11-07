//! Resolución del nombre que se muestra de cada dispositivo.
//!
//! Orden de prioridad:
//! 1. Nombre puesto por el usuario (Ajustes, clave = huella).
//! 2. Base de datos de Volt (`devices.json`): exacto o probable.
//! 3. Lo que informa el propio dispositivo (HID++, descriptor USB, Bluetooth).
//! 4. usb.ids (base de datos pública de VID:PID, subconjunto de periféricos).
//! 5. Nombre genérico del provider.

use crate::model::NameSource;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

#[derive(Deserialize)]
struct Db {
    devices: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    fingerprint: String,
    name: String,
    /// "exact" (el dispositivo se identifica sin ambigüedad) o "probable" (huella
    /// compartida por varios modelos, p. ej. un PID genérico de chip).
    #[serde(default = "exact")]
    confidence: String,
}

fn exact() -> String {
    "exact".into()
}

pub struct DbName {
    pub name: &'static str,
    pub probable: bool,
}

fn db() -> &'static HashMap<String, (String, bool)> {
    static DB: OnceLock<HashMap<String, (String, bool)>> = OnceLock::new();
    DB.get_or_init(|| {
        let parsed: Db = serde_json::from_str(include_str!("../devices.json"))
            .expect("devices.json mal formado");
        parsed
            .devices
            .into_iter()
            .map(|e| {
                (
                    e.fingerprint.to_ascii_lowercase(),
                    (e.name, e.confidence == "probable"),
                )
            })
            .collect()
    })
}

pub fn lookup(fingerprint: &str) -> Option<DbName> {
    db().get(&fingerprint.to_ascii_lowercase()).map(|(n, p)| DbName {
        name: n.as_str(),
        probable: *p,
    })
}

struct UsbIds {
    vendors: HashMap<u16, String>,
    products: HashMap<(u16, u16), String>,
}

fn usb_ids() -> &'static UsbIds {
    static IDS: OnceLock<UsbIds> = OnceLock::new();
    IDS.get_or_init(|| {
        let mut ids = UsbIds {
            vendors: HashMap::new(),
            products: HashMap::new(),
        };
        for line in include_str!("../usb-ids.txt").lines() {
            let Some((key, name)) = line.split_once('\t') else { continue };
            match key.split_once(':') {
                Some((v, p)) => {
                    if let (Ok(v), Ok(p)) = (u16::from_str_radix(v, 16), u16::from_str_radix(p, 16)) {
                        ids.products.insert((v, p), name.to_string());
                    }
                }
                None => {
                    if let Ok(v) = u16::from_str_radix(key, 16) {
                        ids.vendors.insert(v, name.to_string());
                    }
                }
            }
        }
        ids
    })
}

/// Nombre corto del fabricante: el de la tabla de Volt o el de usb.ids sin
/// "Inc.", "Ltd"... ("Logitech, Inc." → "Logitech").
fn vendor_short(vid: u16) -> Option<String> {
    if let Some(v) = crate::device_ids::vendor_name(vid) {
        return Some(v.split([' ', '(']).next().unwrap_or(v).to_string());
    }
    let v = usb_ids().vendors.get(&vid)?;
    Some(v.split([',', '(']).next().unwrap_or(v).trim().to_string())
}

/// Nombre de producto según usb.ids, con el fabricante delante si no lo lleva ya.
/// Ej.: 1532:0084 "DeathAdder V2" → "RAZER DEATHADDER V2".
pub fn usb_name(vid: u16, pid: u16) -> Option<String> {
    let product = usb_ids().products.get(&(vid, pid))?;
    // usb.ids usa "Familia [Modelo]": el modelo entre corchetes es lo más útil.
    let product = match (product.find('['), product.rfind(']')) {
        (Some(a), Some(b)) if b > a + 1 => product[a + 1..b].to_string(),
        _ => product.clone(),
    };
    let name = match vendor_short(vid) {
        Some(v) if !product.to_lowercase().contains(&v.to_lowercase()) => format!("{v} {product}"),
        _ => product,
    };
    Some(name.to_uppercase())
}

/// "046D:C54D" → (0x046D, 0xC54D).
pub fn parse_vid_pid(model: &str) -> Option<(u16, u16)> {
    let (v, p) = model.split_once(':')?;
    if v.len() != 4 || p.len() != 4 {
        return None;
    }
    Some((u16::from_str_radix(v, 16).ok()?, u16::from_str_radix(p, 16).ok()?))
}

/// Nombre final y su origen. `device_named` = el provider obtuvo el nombre del
/// propio dispositivo. `vid_pid` permite el respaldo de usb.ids.
pub fn resolve(
    fingerprint: &str,
    provider_name: &str,
    device_named: bool,
    vid_pid: Option<(u16, u16)>,
    user: &BTreeMap<String, String>,
) -> (String, NameSource) {
    if let Some(n) = user.get(fingerprint).filter(|n| !n.trim().is_empty()) {
        return (n.trim().to_uppercase(), NameSource::User);
    }
    if let Some(e) = lookup(fingerprint) {
        let src = if e.probable {
            NameSource::Probable
        } else {
            NameSource::Database
        };
        return (e.name.to_string(), src);
    }
    if device_named {
        return (provider_name.to_string(), NameSource::Device);
    }
    if let Some(n) = vid_pid.and_then(|(v, p)| usb_name(v, p)) {
        return (n, NameSource::UsbIds);
    }
    (provider_name.to_string(), NameSource::Generic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_parses_and_user_wins() {
        assert_eq!(lookup("compx:0300000000BA").map(|e| e.name), Some("REDRAGON UCAL PRO"));
        let mut user = BTreeMap::new();
        let (n, s) = resolve("compx:0300000000ba", "TECLADO 2.4G", false, None, &user);
        assert_eq!((n.as_str(), s), ("REDRAGON UCAL PRO", NameSource::Database));
        user.insert("compx:0300000000ba".into(), "Mi teclado".into());
        let (n, s) = resolve("compx:0300000000ba", "TECLADO 2.4G", false, None, &user);
        assert_eq!((n.as_str(), s), ("MI TECLADO", NameSource::User));
        // Receptor Attack Shark compartido: probable, no exacto.
        let (_, s) = resolve("xenta:fa60", "RATÓN 2.4G", false, Some((0x1D57, 0xFA60)), &BTreeMap::new());
        assert_eq!(s, NameSource::Probable);
    }

    #[test]
    fn usb_ids_fallback() {
        // 046d:c539 "Lightspeed Receiver" está en usb.ids.
        assert_eq!(usb_name(0x046D, 0xC539).as_deref(), Some("LOGITECH LIGHTSPEED RECEIVER"));
        let (n, s) = resolve("x:1", "GENÉRICO", false, Some((0x046D, 0xC539)), &BTreeMap::new());
        assert_eq!((n.as_str(), s), ("LOGITECH LIGHTSPEED RECEIVER", NameSource::UsbIds));
        assert_eq!(parse_vid_pid("046D:C54D"), Some((0x046D, 0xC54D)));
        assert_eq!(parse_vid_pid("48:73:CB:FB:A6:23"), None);
        // Los de Volt ganan a usb.ids.
        assert!(usb_ids().products.len() > 1000);
    }
}
