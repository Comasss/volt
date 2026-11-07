//! Ajustes persistentes en `%APPDATA%\dev.volt.battery\settings.json`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub poll_interval_secs: u32,
    pub low_threshold: u8,
    pub notifications: bool,
    pub theme: Theme,
    pub demo_devices: bool,
    /// Espejo del registro (HKCU\...\Run); la fuente de verdad es el plugin.
    pub autostart: bool,
    /// BlackShark V2 Pro 2023: con Synapse cerrado, dejar el registro de EQ
    /// del auricular como lo deja Synapse (si no, suena más flojo).
    pub blackshark_keep_eq: bool,
    /// Nombres puestos por el usuario: huella del dispositivo → nombre.
    pub device_names: BTreeMap<String, String>,
    /// "system" (idioma de Windows), "es" o "en".
    pub language: String,
    /// Enseñar también los dispositivos apagados/desconectados (con su último valor).
    pub show_offline: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_interval_secs: 60,
            low_threshold: 15,
            notifications: true,
            theme: Theme::System,
            demo_devices: false,
            autostart: false,
            blackshark_keep_eq: true,
            device_names: BTreeMap::new(),
            language: "system".into(),
            show_offline: false,
        }
    }
}

impl Settings {
    pub fn sanitized(mut self) -> Self {
        self.poll_interval_secs = self.poll_interval_secs.clamp(10, 3600);
        self.low_threshold = self.low_threshold.clamp(1, 95);
        if !matches!(self.language.as_str(), "system" | "es" | "en") {
            self.language = "system".into();
        }
        self.device_names.retain(|k, v| !k.is_empty() && !v.trim().is_empty());
        self
    }

    fn path(app: &AppHandle) -> Option<PathBuf> {
        app.path()
            .app_config_dir()
            .ok()
            .map(|d| d.join("settings.json"))
    }

    pub fn load(app: &AppHandle) -> Self {
        Self::path(app)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default()
            .sanitized()
    }

    pub fn save(&self, app: &AppHandle) -> Result<(), String> {
        let path = Self::path(app).ok_or("sin carpeta de configuración")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }
}
