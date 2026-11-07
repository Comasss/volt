//! Ciclo de vida de la ventana.
//!
//! Cerrar la ventana la **destruye** (no la oculta): así WebView2 libera sus
//! procesos y la app queda solo en la bandeja con unos pocos MB. Abrirla de nuevo
//! la recrea desde la configuración de `tauri.conf.json` (`"create": false`).

use crate::manager::Shared;
use crate::settings::Theme;
use std::sync::Arc;
use tauri::window::Color;
use tauri::{AppHandle, Manager, WebviewWindow, WebviewWindowBuilder};

pub const MAIN: &str = "main";

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let Some(cfg) = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN)
        .cloned()
    else {
        log::error!("falta la ventana '{MAIN}' en tauri.conf.json");
        return;
    };
    let dark = match app.state::<Arc<Shared>>().settings().theme {
        Theme::Dark => true,
        Theme::Light => false,
        Theme::System => apps_use_dark_theme(),
    };
    let built = WebviewWindowBuilder::from_config(app, &cfg).and_then(|b| {
        // Fondo correcto desde el primer frame: sin destello blanco en modo oscuro.
        let bg = if dark {
            Color(0x0E, 0x0E, 0x0E, 0xFF)
        } else {
            Color(0xF8, 0xF8, 0xF8, 0xFF)
        };
        b.background_color(bg).build()
    });
    match built {
        Ok(w) => {
            square_corners(&w);
            let _ = w.set_focus();
        }
        Err(e) => log::error!("no se pudo crear la ventana: {e}"),
    }
}

/// Para handlers de bandeja/menú/segunda instancia: crear un webview dentro del
/// propio callback del bucle de eventos puede bloquearse en Windows, así que se
/// hace desde otro hilo (Tauri lo despacha al hilo principal).
pub fn show_main_async(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || show_main(&app));
}

/// Windows 11 redondea las ventanas sin marco; la estética pide esquinas rectas.
#[cfg(windows)]
fn square_corners(w: &WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };
    if let Ok(h) = w.hwnd() {
        let hwnd = HWND(h.0 as _);
        let pref = DWMWCP_DONOTROUND;
        unsafe {
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const _,
                std::mem::size_of_val(&pref) as u32,
            );
        }
    }
}

#[cfg(not(windows))]
fn square_corners(_w: &WebviewWindow) {}

#[cfg(windows)]
pub fn apps_use_dark_theme() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r == ERROR_SUCCESS && value == 0
}

#[cfg(not(windows))]
pub fn apps_use_dark_theme() -> bool {
    false
}
