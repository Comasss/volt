//! Comandos invocables desde el frontend. Ninguno sondea hardware por sí mismo:
//! o devuelven el último estado o piden algo al hilo del gestor.

use crate::manager::{Command, HidRow, Shared, StatePayload};
use crate::settings::Settings;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tauri::{AppHandle, State};
use tauri_plugin_autostart::ManagerExt;

/// Estado actual (lo que ya se leyó). Lo pide la ventana al abrirse.
#[tauri::command]
pub fn get_state(app: AppHandle, shared: State<'_, Arc<Shared>>) -> StatePayload {
    let mut state = shared.snapshot();
    // El registro manda: alguien pudo desactivar el arranque desde el Administrador de tareas.
    state.settings.autostart = app.autolaunch().is_enabled().unwrap_or(false);
    state
}

#[tauri::command]
pub fn refresh_now(shared: State<'_, Arc<Shared>>) {
    shared.send(Command::Refresh);
}

#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    shared: State<'_, Arc<Shared>>,
    settings: Settings,
) -> Result<Settings, String> {
    let mut settings = settings.sanitized();

    let autolaunch = app.autolaunch();
    let enabled = autolaunch.is_enabled().unwrap_or(false);
    if settings.autostart != enabled {
        let r = if settings.autostart {
            autolaunch.enable()
        } else {
            autolaunch.disable()
        };
        if let Err(e) = r {
            settings.autostart = enabled;
            log::warn!("autostart: {e}");
        }
    }

    settings.save(&app)?;
    shared.set_settings(settings.clone());
    shared.send(Command::Reconfigure);
    Ok(settings)
}

/// Listado HID crudo (vista de diagnóstico). Async para no bloquear el hilo principal
/// mientras el gestor re-enumera.
#[tauri::command]
pub async fn hid_inventory(shared: State<'_, Arc<Shared>>) -> Result<Vec<HidRow>, String> {
    let (tx, rx) = mpsc::channel();
    if !shared.send(Command::HidInventory(tx)) {
        return Err("el gestor no está activo".into());
    }
    rx.recv_timeout(Duration::from_secs(10))
        .map_err(|_| "el gestor está ocupado; inténtalo de nuevo".to_string())
}

/// Nombre puesto por el usuario para un dispositivo (por huella). `None` o vacío
/// vuelve al nombre automático.
#[tauri::command]
pub fn rename_device(
    app: AppHandle,
    shared: State<'_, Arc<Shared>>,
    fingerprint: String,
    name: Option<String>,
) -> Result<Settings, String> {
    let mut settings = shared.settings();
    match name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
        Some(n) => {
            settings.device_names.insert(fingerprint, n.chars().take(40).collect());
        }
        None => {
            settings.device_names.remove(&fingerprint);
        }
    }
    settings.save(&app)?;
    shared.set_settings(settings.clone());
    shared.send(Command::Reconfigure);
    Ok(settings)
}

#[tauri::command]
pub fn test_notification() {
    crate::notify::test();
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
