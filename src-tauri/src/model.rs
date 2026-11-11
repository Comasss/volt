//! Tipos de datos compartidos entre providers, gestor y frontend.

use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Mouse,
    Keyboard,
    Headset,
    Gamepad,
    Other,
}

/// Cómo llega el dispositivo al PC. El orden define el orden en la UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    /// Receptor USB inalámbrico (LIGHTSPEED, Unifying, Bolt, HyperSpeed...).
    Receiver,
    /// Cable USB directo.
    Wired,
    Bluetooth,
    /// Dispositivos simulados.
    Virtual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargeState {
    Discharging,
    Charging,
    Full,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatteryReading {
    /// 0..=100. `None` si el dispositivo solo informa de "cargando" sin nivel.
    pub percent: Option<u8>,
    /// El porcentaje es una estimación (niveles discretos o calculado por voltaje).
    pub approximate: bool,
    pub charge: ChargeState,
    pub voltage_mv: Option<u16>,
    /// Momento real de la medida (unix ms) cuando no es "ahora", p. ej. un valor
    /// que Windows tenía cacheado de la última conexión Bluetooth.
    #[serde(skip)]
    pub measured_at: Option<u64>,
}

impl BatteryReading {
    pub fn percent(percent: u8, charge: ChargeState) -> Self {
        Self {
            percent: Some(percent.min(100)),
            approximate: false,
            charge,
            voltage_mv: None,
            measured_at: None,
        }
    }

    pub fn is_charging(&self) -> bool {
        matches!(self.charge, ChargeState::Charging | ChargeState::Full)
    }
}

/// Lo que un provider sabe de un dispositivo sin leer todavía la batería.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    /// Identificador estable durante la sesión: `<provider>:<clave única>`.
    pub id: String,
    /// Huella del *modelo* (no de la conexión): lo que el dispositivo dice de sí
    /// mismo (ID de modelo, nombre HID++, dirección BT...). Clave de la base de
    /// datos de nombres y de los nombres que pone el usuario.
    pub fingerprint: String,
    pub provider: &'static str,
    pub vendor: String,
    pub name: String,
    /// `name` viene del propio dispositivo (no es genérico ni de una tabla).
    #[serde(skip)]
    pub name_from_device: bool,
    /// Código técnico corto (VID:PID, dirección BT...). Se muestra como "modelo".
    pub model: String,
    pub kind: DeviceKind,
    pub connection: Connection,
    /// Nodo padre en el árbol de la UI (p. ej. el receptor). `None` = cuelga del grupo.
    pub via: Option<String>,
    /// Slot en el receptor, si aplica.
    pub slot: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum DeviceStatus {
    Online,
    /// Emparejado pero apagado, dormido o fuera de alcance.
    Offline,
    /// Presente pero no se pudo leer (app del fabricante, protocolo, timeout...).
    Unavailable(String),
}

/// Estado completo de un dispositivo tal y como lo ve el frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSnapshot {
    #[serde(flatten)]
    pub info: DeviceInfo,
    pub status: DeviceStatus,
    /// Última lectura válida (puede ser antigua si el estado no es `Online`).
    pub battery: Option<BatteryReading>,
    /// Unix ms de la última lectura válida.
    pub last_read: Option<u64>,
    /// Por debajo del umbral y sin cargar.
    pub low: bool,
    /// Hay ajustes que personalizar (DPI, luces, reposo, audio…).
    pub configurable: bool,
    /// De dónde sale el nombre mostrado.
    pub name_source: NameSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NameSource {
    /// El propio dispositivo lo informa (HID++, descriptor USB, Bluetooth).
    Device,
    /// Base de datos de Volt (`devices.json`), identificación exacta.
    Database,
    /// Base de datos de Volt, pero la huella la comparten varios modelos.
    Probable,
    /// Base de datos pública usb.ids (por VID:PID).
    UsbIds,
    /// Puesto por el usuario.
    User,
    /// Genérico: el dispositivo no se identifica.
    Generic,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
