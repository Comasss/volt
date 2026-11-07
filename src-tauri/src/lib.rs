//! Volt: monitor de batería de periféricos inalámbricos.
//!
//! Arquitectura:
//! - `provider`  trait `DeviceProvider` (un provider por fabricante/protocolo)
//! - `providers` logitech (HID++), razer, compx, xenta, bluetooth (PnP + GATT), mock
//! - `manager`   hilo de sondeo único + escuchas de eventos; publica `volt://state`
//! - `names`     nombres exactos: usuario › base de datos › dispositivo › genérico
//! - `devwatch`  aviso de Windows al enchufar/desenchufar
//! - `tray`, `window`, `notify`, `i18n`, `settings`, `commands`

pub mod commands;
pub mod device_ids;
pub mod devwatch;
pub mod hid;
pub mod i18n;
pub mod manager;
pub mod model;
pub mod names;
pub mod notify;
pub mod provider;
pub mod providers;
pub mod settings;
pub mod synapse;
pub mod tray;
pub mod window;

use manager::Shared;
use std::sync::Arc;
use tauri::{Manager, RunEvent};
use tauri_plugin_autostart::MacosLauncher;

/// Argumento que añade el arranque con Windows: empezar solo en la bandeja.
pub const ARG_HIDDEN: &str = "--hidden";
/// Fuerza los dispositivos simulados sin tocar los ajustes.
pub const ARG_MOCK: &str = "--mock";
/// Muestra una notificación de prueba (útil con la app ya abierta: `volt.exe --test-notification`).
pub const ARG_TEST_NOTIFICATION: &str = "--test-notification";

/// Deja el icono en disco para que Windows lo enseñe en las notificaciones.
fn notification_icon(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let dir = app.path().app_local_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("volt.png");
    let bytes: &[u8] = include_bytes!("../icons/128x128@2x.png");
    if std::fs::read(&path).ok().as_deref() != Some(bytes) {
        std::fs::write(&path, bytes).ok()?;
    }
    Some(path)
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("volt_lib=info"))
        .init();

    let args: Vec<String> = std::env::args().collect();
    let start_hidden = args.iter().any(|a| a == ARG_HIDDEN || a == ARG_TEST_NOTIFICATION);
    let force_mock = args.iter().any(|a| a == ARG_MOCK);
    let test_notification = args.iter().any(|a| a == ARG_TEST_NOTIFICATION);

    let app = tauri::Builder::default()
        // Debe ir primero: una segunda instancia solo enseña la ventana de la primera
        // (dos instancias pelearían por los mismos dispositivos HID).
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if argv.iter().any(|a| a == ARG_TEST_NOTIFICATION) {
                notify::test();
            } else {
                window::show_main_async(app);
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![ARG_HIDDEN]),
        ))
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::refresh_now,
            commands::update_settings,
            commands::rename_device,
            commands::test_notification,
            commands::hid_inventory,
            commands::quit_app,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            let settings = settings::Settings::load(&handle);
            let shared = Arc::new(Shared::new(settings, force_mock));
            app.manage(shared.clone());

            notify::init(notification_icon(&handle).as_deref());
            tray::create(&handle)?;
            manager::spawn(handle.clone(), shared);
            if test_notification {
                notify::test();
            }
            if !start_hidden {
                window::show_main(&handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error al construir la aplicación");

    app.run(|_app, event| {
        // Al cerrar la última ventana Tauri pediría salir: seguimos en la bandeja.
        // `app.exit(0)` (menú "Salir") llega con código y sí sale.
        if let RunEvent::ExitRequested { code: None, api, .. } = event {
            api.prevent_exit();
        }
    });
}
