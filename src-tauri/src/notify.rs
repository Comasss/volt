//! Notificaciones de Windows (toast) con la identidad de Volt.
//!
//! Windows asocia cada toast a un AppUserModelID (AUMID). Una app sin instalar no
//! tiene ninguno registrado, y por eso las notificaciones salían como "PowerShell".
//! Aquí se registra el AUMID en `HKCU\Software\Classes\AppUserModelId\<id>` con
//! nombre e icono (el mismo mecanismo que usa Windows App SDK para apps sin
//! empaquetar; no requiere administrador ni acceso directo) y se envían los toasts
//! con WinRT a ese AUMID.

use crate::i18n::tr;

pub const AUMID: &str = "dev.volt.battery";

/// Registra Volt como origen de notificaciones. `icon` = PNG en disco.
#[cfg(windows)]
pub fn init(icon: Option<&std::path::Path>) {
    use windows::core::HSTRING;
    use windows::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
    use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

    let path = HSTRING::from(format!("Software\\Classes\\AppUserModelId\\{AUMID}"));
    // RegSetKeyValueW crea la clave si no existe.
    let set = |name: &str, value: &str| {
        let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &path,
                &HSTRING::from(name),
                REG_SZ.0,
                Some(wide.as_ptr() as *const _),
                (wide.len() * 2) as u32,
            )
        }
    };
    let r = set("DisplayName", "Volt");
    if r.is_err() {
        log::warn!("notify: no se pudo registrar el AUMID ({r:?})");
    }
    if let Some(icon) = icon {
        let _ = set("IconUri", &icon.to_string_lossy());
    }
    let _ = set("IconBackgroundColor", "FF111111");
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(AUMID));
    }
}

#[cfg(not(windows))]
pub fn init(_icon: Option<&std::path::Path>) {}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(windows)]
pub fn show(title: &str, body: &str) {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    let xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_escape(title),
        xml_escape(body)
    );
    let result = (|| -> windows::core::Result<()> {
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&doc)?;
        ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))?.Show(&toast)
    })();
    if let Err(e) = result {
        log::warn!("notify: no se pudo mostrar la notificación: {e}");
    }
}

#[cfg(not(windows))]
pub fn show(title: &str, body: &str) {
    log::info!("notificación: {title} — {body}");
}

pub fn low_battery(name: &str, percent: u8) {
    show(
        &format!("{} · {name}", tr("Batería baja", "Low battery")),
        &match crate::i18n::lang() {
            crate::i18n::Lang::Es => format!("Queda un {percent} %. Conéctalo a la corriente."),
            crate::i18n::Lang::En => format!("{percent}% left. Plug it in to charge."),
        },
    );
}

pub fn test() {
    show(
        "Volt",
        tr(
            "Las notificaciones de Volt funcionan.",
            "Volt notifications are working.",
        ),
    );
}
