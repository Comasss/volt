//! Idioma de los textos del backend (bandeja, notificaciones, errores, nombres
//! genéricos). Por defecto, el idioma de la interfaz de Windows.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Es,
    En,
}

static LANG: AtomicU8 = AtomicU8::new(0); // 0 = sin resolver

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::Es => "es",
            Lang::En => "en",
        }
    }
}

/// Idioma de la interfaz de Windows (español → Es; cualquier otro → En).
#[cfg(windows)]
pub fn system_lang() -> Lang {
    // LANG_SPANISH = 0x0A en los 10 bits bajos del LANGID.
    let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    if id & 0x3FF == 0x0A {
        Lang::Es
    } else {
        Lang::En
    }
}

#[cfg(not(windows))]
pub fn system_lang() -> Lang {
    match std::env::var("LANG") {
        Ok(l) if l.starts_with("es") => Lang::Es,
        _ => Lang::En,
    }
}

/// Aplica el ajuste ("system", "es", "en").
pub fn set(setting: &str) -> Lang {
    let lang = match setting {
        "es" => Lang::Es,
        "en" => Lang::En,
        _ => system_lang(),
    };
    LANG.store(if lang == Lang::Es { 1 } else { 2 }, Ordering::Relaxed);
    lang
}

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::Es,
        2 => Lang::En,
        _ => set("system"),
    }
}

/// Elige la versión del texto según el idioma activo.
pub fn tr(es: &'static str, en: &'static str) -> &'static str {
    match lang() {
        Lang::Es => es,
        Lang::En => en,
    }
}
