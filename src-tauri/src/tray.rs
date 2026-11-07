//! Icono de bandeja: tooltip con las baterías, menú y un icono dinámico que
//! dibuja la batería más baja como indicador segmentado.

use crate::i18n::{tr, Lang};
use crate::manager::{Command, Shared};
use crate::model::{DeviceSnapshot, DeviceStatus};
use crate::window;
use std::sync::{Arc, Mutex};
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

const TRAY_ID: &str = "volt";
const SIZE: u32 = 32;
/// Último icono dibujado (segmentos, aviso, barra oscura) para no redibujar en vano.
static LAST_ICON: Mutex<Option<(u8, bool, bool)>> = Mutex::new(None);
/// Contenido del último menú construido: solo se reconstruye si cambia algo visible.
static LAST_MENU: Mutex<Option<String>> = Mutex::new(None);

/// Texto de un dispositivo en el menú: nombre y, tras un tabulador (Windows lo
/// alinea a la derecha, como un atajo), su batería.
fn device_label(d: &DeviceSnapshot) -> String {
    let b = d.battery.as_ref();
    let pct = b.and_then(|b| b.percent);
    let charging = b.is_some_and(|b| b.is_charging());
    let approx = b.is_some_and(|b| b.approximate);
    let value = match (&d.status, pct) {
        (DeviceStatus::Online, Some(p)) => format!(
            "{}{}{p} %",
            if charging { "⚡ " } else { "" },
            if approx { "≈" } else { "" }
        ),
        (DeviceStatus::Online, None) if charging => "⚡".into(),
        (DeviceStatus::Online, None) => "—".into(),
        (DeviceStatus::Offline, _) => tr("apagado", "off").into(),
        (DeviceStatus::Unavailable(_), _) => tr("n/d", "n/a").into(),
    };
    let warn = if d.low { "⚠ " } else { "" };
    // "&" es el marcador de tecla de acceso en los menús de Windows.
    format!("{warn}{}\t{value}", d.info.name.replace('&', "&&"))
}

fn build_menu(app: &AppHandle, devices: &[DeviceSnapshot]) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;
    let header = match (crate::i18n::lang(), devices.len()) {
        (Lang::Es, 1) => "Volt · 1 dispositivo".to_string(),
        (Lang::Es, n) => format!("Volt · {n} dispositivos"),
        (Lang::En, 1) => "Volt · 1 device".to_string(),
        (Lang::En, n) => format!("Volt · {n} devices"),
    };
    menu.append(&MenuItem::with_id(app, "header", header, false, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    if devices.is_empty() {
        let empty = tr("Sin dispositivos", "No devices");
        menu.append(&MenuItem::with_id(app, "none", empty, false, None::<&str>)?)?;
    }
    for d in devices {
        let id = format!("dev:{}", d.info.id);
        menu.append(&MenuItem::with_id(app, id, device_label(d), true, None::<&str>)?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "open", tr("Abrir Volt", "Open Volt"), true, None::<&str>)?)?;
    menu.append(&MenuItem::with_id(app, "refresh", tr("Actualizar ahora", "Refresh now"), true, None::<&str>)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(app, "quit", tr("Salir", "Quit"), true, None::<&str>)?)?;
    Ok(menu)
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app, &[])?;
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::new_owned(render(None, false, taskbar_is_dark()), SIZE, SIZE))
        .tooltip("Volt")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "refresh" => {
                app.state::<Arc<Shared>>().send(Command::Refresh);
            }
            "quit" => app.exit(0),
            // "Abrir" o cualquier dispositivo: enseña la ventana.
            id if id == "open" || id.starts_with("dev:") => window::show_main_async(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                window::show_main_async(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

pub fn update(app: &AppHandle, devices: &[DeviceSnapshot]) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let _ = tray.set_tooltip(Some(tooltip(devices)));

    // Menú con cada dispositivo: se reconstruye solo si cambia lo que muestra.
    let key = format!(
        "{}|{}",
        crate::i18n::lang().code(),
        devices.iter().map(|d| format!("{}={}", d.info.id, device_label(d))).collect::<Vec<_>>().join("|")
    );
    let mut last_menu = LAST_MENU.lock().unwrap();
    if last_menu.as_deref() != Some(key.as_str()) {
        match build_menu(app, devices) {
            Ok(menu) => {
                let _ = tray.set_menu(Some(menu));
                *last_menu = Some(key);
            }
            Err(e) => log::warn!("bandeja: no se pudo construir el menú: {e}"),
        }
    }

    let online = devices
        .iter()
        .filter(|d| d.status == DeviceStatus::Online)
        .filter_map(|d| d.battery.as_ref()?.percent.map(|p| (p, d.low)));
    let lowest = online.clone().map(|(p, _)| p).min();
    let any_low = online.clone().any(|(_, low)| low);
    let dark = taskbar_is_dark();
    let key = (segments(lowest), any_low, dark);

    let mut last = LAST_ICON.lock().unwrap();
    if last.as_ref() != Some(&key) {
        let _ = tray.set_icon(Some(Image::new_owned(render(lowest, any_low, dark), SIZE, SIZE)));
        *last = Some(key);
    }
}
fn tooltip(devices: &[DeviceSnapshot]) -> String {
    // Windows corta el tooltip en 127 caracteres UTF-16.
    const MAX: usize = 127;
    let mut s = String::from("Volt");
    if devices.is_empty() {
        s.push_str(tr("\nSin dispositivos", "\nNo devices"));
    }
    for d in devices {
        let pct = d.battery.as_ref().and_then(|b| b.percent);
        let charging = d.battery.as_ref().is_some_and(|b| b.is_charging());
        let value = match (&d.status, pct) {
            (DeviceStatus::Online, Some(p)) => format!("{p}%{}", if charging { " ⚡" } else { "" }),
            (DeviceStatus::Online, None) => "—".into(),
            (DeviceStatus::Offline, _) => "off".into(),
            (DeviceStatus::Unavailable(_), _) => tr("n/d", "n/a").into(),
        };
        let name: String = d.info.name.chars().take(22).collect();
        let line = format!("\n{name}  {value}");
        if s.encode_utf16().count() + line.encode_utf16().count() > MAX - 2 {
            s.push_str("\n…");
            break;
        }
        s.push_str(&line);
    }
    s
}

fn segments(level: Option<u8>) -> u8 {
    level.map(|p| (p as u16).div_ceil(25).min(4) as u8).unwrap_or(0)
}

/// Batería de 4 segmentos en 32×32 RGBA. Trazo de 2 px para que al reducirse a
/// 16×16 (escala 100 %) quede en 1 px nítido.
fn render(level: Option<u8>, low: bool, dark_taskbar: bool) -> Vec<u8> {
    let s = SIZE as usize;
    let fg: [u8; 4] = if dark_taskbar {
        [0xF2, 0xF2, 0xF2, 0xFF]
    } else {
        [0x11, 0x11, 0x11, 0xFF]
    };
    let warn: [u8; 4] = [0xFF, 0x4F, 0x00, 0xFF];
    let mut px = vec![0u8; s * s * 4];
    let mut rect = |x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 4]| {
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * s + x) * 4;
                px[i..i + 4].copy_from_slice(&c);
            }
        }
    };
    // Cuerpo
    rect(1, 8, 28, 10, fg);
    rect(1, 22, 28, 24, fg);
    rect(1, 8, 3, 24, fg);
    rect(26, 8, 28, 24, fg);
    // Polo
    rect(28, 12, 31, 20, fg);
    match level {
        Some(_) => {
            let n = segments(level) as usize;
            for i in 0..n {
                let x0 = 5 + i * 5;
                rect(x0, 12, x0 + 4, 20, if low { warn } else { fg });
            }
        }
        // Sin datos: un guion.
        None => rect(10, 15, 19, 17, fg),
    }
    px
}

#[cfg(windows)]
fn taskbar_is_dark() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let mut value: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    r == ERROR_SUCCESS && value == 0
}

#[cfg(not(windows))]
fn taskbar_is_dark() -> bool {
    false
}
