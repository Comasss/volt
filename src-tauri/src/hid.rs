//! Contexto HID compartido: una única instancia de hidapi y una enumeración por ciclo.

use hidapi::{HidApi, HidDevice, HidResult};
use serde::Serialize;
use std::ffi::{CStr, CString};

/// Una colección de nivel superior (TLC) HID. En Windows cada TLC de una interfaz
/// aparece como un "dispositivo" HID distinto, con su propia ruta (`...&Col01...`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HidEntry {
    #[serde(skip)]
    pub path: CString,
    pub path_str: String,
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    pub interface: i32,
    pub product: String,
    pub manufacturer: String,
    pub serial: String,
    pub bus: String,
}

pub struct HidContext {
    api: HidApi,
    pub devices: Vec<HidEntry>,
}

impl HidContext {
    pub fn new() -> Result<Self, String> {
        let api = HidApi::new().map_err(|e| e.to_string())?;
        let mut ctx = Self {
            api,
            devices: Vec::new(),
        };
        ctx.refresh();
        Ok(ctx)
    }

    /// Re-enumera el bus (~ms). No abre ningún dispositivo.
    pub fn refresh(&mut self) {
        if let Err(e) = self.api.refresh_devices() {
            log::warn!("hidapi refresh_devices: {e}");
        }
        self.devices = self
            .api
            .device_list()
            .map(|d| HidEntry {
                path: d.path().to_owned(),
                path_str: d.path().to_string_lossy().into_owned(),
                vid: d.vendor_id(),
                pid: d.product_id(),
                usage_page: d.usage_page(),
                usage: d.usage(),
                interface: d.interface_number(),
                product: d.product_string().unwrap_or_default().trim().to_string(),
                manufacturer: d.manufacturer_string().unwrap_or_default().trim().to_string(),
                serial: d.serial_number().unwrap_or_default().to_string(),
                bus: format!("{:?}", d.bus_type()),
            })
            .collect();
    }

    pub fn open(&self, path: &CStr) -> HidResult<HidDevice> {
        self.api.open_path(path)
    }
}

/// Clave que identifica la *interfaz física* a la que pertenece una TLC, para poder
/// agrupar las colecciones hermanas (p. ej. HID++ corto `Col01` y largo `Col02`).
///
/// Ruta típica en Windows:
/// `\\?\HID#VID_046D&PID_C54D&MI_02&Col01#8&2a1b3c4d&0&0000#{4d1e55b2-...}`
/// Se quita `&ColNN` del hardware id y el último `&NNNN` del id de instancia.
pub fn interface_key(e: &HidEntry) -> String {
    let p = e.path_str.to_ascii_lowercase();
    let parts: Vec<&str> = p.split('#').collect();
    if parts.len() >= 3 {
        let hw = parts[1]
            .split('&')
            .filter(|s| !s.starts_with("col"))
            .collect::<Vec<_>>()
            .join("&");
        let inst = parts[2];
        let inst = inst.rfind('&').map(|i| &inst[..i]).unwrap_or(inst);
        return format!("{hw}#{inst}");
    }
    format!(
        "{:04x}:{:04x}:{}:{}",
        e.vid, e.pid, e.interface, e.serial
    )
}

/// Hash corto y estable (FNV-1a) para construir ids legibles.
pub fn short_hash(s: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{:06x}", h & 0x00ff_ffff)
}
