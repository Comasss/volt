//! Controles configurables de un dispositivo (DPI, iluminación, reposo, audio…).
//!
//! Cada provider describe lo que sabe ajustar como una lista de `Control`: un
//! identificador, un tipo (rango, lista, interruptor, color, bandas de EQ) y el
//! valor que el dispositivo tiene **ahora**. La interfaz los pinta de forma
//! genérica, así que añadir un ajuste a un provider no toca el frontend.
//!
//! Regla de honestidad: cada control lleva su nivel de confianza (`Trust`). Nada de
//! lo que hay aquí se ha podido probar con el hardware delante durante el
//! desarrollo, así que tras cada escritura se vuelve a leer el dispositivo y lo que
//! se enseña es lo que este contesta, no lo que se le pidió.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Apartado de la ficha en el que aparece el control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Pointer,
    Lighting,
    Power,
    Audio,
}

/// De dónde sale el protocolo de un control y cuánto se puede confiar en él.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    /// Protocolo documentado y verificado por terceros: o es un estándar que el
    /// propio dispositivo anuncia (features HID++), o se verificó en este modelo.
    Reference,
    /// Verificado por terceros en otro modelo de la misma familia de firmware.
    Sibling,
    /// Deducido, sin verificar en ningún hardware. Solo se puede escribir con el
    /// ajuste «controles experimentales» activado.
    Experimental,
}

/// Valor de un control. Sin etiqueta: en JSON es un booleano, un número, un texto
/// (`"#rrggbb"`) o una lista de números (bandas de EQ).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Text(String),
    List(Vec<i64>),
}

impl Value {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Int(v) => Some(*v != 0),
            _ => None,
        }
    }

    /// `#rrggbb` → (r, g, b).
    pub fn as_rgb(&self) -> Option<(u8, u8, u8)> {
        let Value::Text(s) = self else { return None };
        let h = s.strip_prefix('#')?;
        if h.len() != 6 || !h.is_ascii() {
            return None;
        }
        let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some((p(0)?, p(2)?, p(4)?))
    }

    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Value::Text(format!("#{r:02x}{g:02x}{b:02x}"))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChoiceOption {
    pub value: i64,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BandPreset {
    pub label: String,
    pub values: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    /// Número entre `min` y `max`. `presets` son atajos (p. ej. 400/800/1600 DPI).
    Range {
        min: i64,
        max: i64,
        step: i64,
        unit: &'static str,
        presets: Vec<i64>,
    },
    Choice {
        options: Vec<ChoiceOption>,
    },
    Toggle,
    /// Color `#rrggbb`.
    Color,
    /// Ecualizador: una barra por banda.
    Bands {
        labels: Vec<String>,
        min: i64,
        max: i64,
        unit: &'static str,
        presets: Vec<BandPreset>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Control {
    /// Estable dentro del provider ("dpi", "light.effect"…).
    pub id: &'static str,
    pub section: Section,
    pub label: String,
    pub hint: Option<String>,
    #[serde(flatten)]
    pub kind: Kind,
    /// Lo que dice el dispositivo. `None` si el ajuste se puede escribir pero no leer.
    pub value: Option<Value>,
    pub trust: Trust,
    /// El dispositivo lo olvida al apagarse: Volt lo guarda y lo vuelve a aplicar
    /// al reconectar (si el ajuste «reaplicar» está activo).
    pub volatile: bool,
}

impl Control {
    pub fn new(id: &'static str, section: Section, label: &str, kind: Kind, value: Option<Value>) -> Self {
        Self {
            id,
            section,
            label: label.into(),
            hint: None,
            kind,
            value,
            trust: Trust::Reference,
            volatile: false,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn trust(mut self, trust: Trust) -> Self {
        self.trust = trust;
        self
    }

    pub fn volatile(mut self) -> Self {
        self.volatile = true;
        self
    }
}

/// Todo lo configurable de un dispositivo, leído en este momento.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceControls {
    pub controls: Vec<Control>,
    /// De dónde sale el protocolo (proyecto de referencia, qué se verificó y dónde).
    pub source: String,
    /// Avisos del momento: app del fabricante abierta, modo del dispositivo…
    pub notes: Vec<String>,
}

/// Valores guardados de los controles volátiles: huella → (control → valor).
pub type SavedControls = BTreeMap<String, BTreeMap<String, Value>>;

pub fn choice(options: impl IntoIterator<Item = (i64, String)>) -> Kind {
    Kind::Choice {
        options: options
            .into_iter()
            .map(|(value, label)| ChoiceOption { value, label })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_roundtrips_as_plain_json() {
        let v: Value = serde_json::from_str("true").unwrap();
        assert_eq!(v, Value::Bool(true));
        let v: Value = serde_json::from_str("1600").unwrap();
        assert_eq!(v.as_int(), Some(1600));
        let v: Value = serde_json::from_str("[1,-2,3]").unwrap();
        assert_eq!(v, Value::List(vec![1, -2, 3]));
        let v: Value = serde_json::from_str("\"#ff8000\"").unwrap();
        assert_eq!(v.as_rgb(), Some((255, 128, 0)));
        assert_eq!(serde_json::to_string(&Value::rgb(1, 2, 255)).unwrap(), "\"#0102ff\"");
        assert_eq!(Value::Text("#12".into()).as_rgb(), None);
        assert_eq!(Value::Text("#ñññ".into()).as_rgb(), None);
    }

    #[test]
    fn control_serializes_flat() {
        let c = Control::new(
            "dpi",
            Section::Pointer,
            "DPI",
            Kind::Range { min: 100, max: 32000, step: 50, unit: "DPI", presets: vec![800] },
            Some(Value::Int(1600)),
        )
        .volatile();
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["type"], "range");
        assert_eq!(j["max"], 32000);
        assert_eq!(j["value"], 1600);
        assert_eq!(j["trust"], "reference");
        assert_eq!(j["volatile"], true);
    }
}
