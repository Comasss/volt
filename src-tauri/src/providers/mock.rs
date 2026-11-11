//! Dispositivos simulados para desarrollar la interfaz sin hardware.
//! Se activan en Ajustes ("Dispositivos demo") o arrancando con `--mock`.

use crate::controls::{choice, BandPreset, Control, DeviceControls, Kind, Section, Trust, Value};
use crate::i18n::tr;
use crate::model::{BatteryReading, ChargeState, Connection, DeviceInfo, DeviceKind};
use crate::provider::{Capabilities, DeviceProvider, PollContext, ProviderError};
use std::collections::HashMap;

struct Spec {
    key: &'static str,
    name: &'static str,
    model: &'static str,
    kind: DeviceKind,
    connection: Connection,
    via: Option<&'static str>,
    start: u8,
    behaviour: Behaviour,
}

#[derive(Clone, Copy)]
enum Behaviour {
    Draining,
    Charging,
    Asleep,
    Locked,
}

const SPECS: &[Spec] = &[
    Spec {
        key: "ms01",
        name: "MS-01 WIRELESS",
        model: "0000:M501",
        kind: DeviceKind::Mouse,
        connection: Connection::Receiver,
        via: Some("DEMO RECEIVER R1"),
        start: 82,
        behaviour: Behaviour::Draining,
    },
    Spec {
        key: "hs02",
        name: "HS-02 HEADSET",
        model: "0000:H502",
        kind: DeviceKind::Headset,
        connection: Connection::Receiver,
        via: Some("DEMO RECEIVER R1"),
        start: 13,
        behaviour: Behaviour::Draining,
    },
    Spec {
        key: "kb75",
        name: "KB-75 MECHANICAL",
        model: "DE:M0:00:00:00:75",
        kind: DeviceKind::Keyboard,
        connection: Connection::Bluetooth,
        via: Some("BLUETOOTH LE"),
        start: 46,
        behaviour: Behaviour::Charging,
    },
    Spec {
        key: "pd01",
        name: "PD-01 CONTROLLER",
        model: "DE:M0:00:00:00:01",
        kind: DeviceKind::Gamepad,
        connection: Connection::Bluetooth,
        via: Some("BLUETOOTH CLÁSICO"),
        start: 64,
        behaviour: Behaviour::Asleep,
    },
    Spec {
        key: "tb01",
        name: "TB-01 TRACKBALL",
        model: "0000:T801",
        kind: DeviceKind::Mouse,
        connection: Connection::Wired,
        via: None,
        start: 0,
        behaviour: Behaviour::Locked,
    },
];

pub struct MockProvider {
    tick: u32,
    /// Valores puestos desde la ficha: (dispositivo, control) → valor.
    values: HashMap<(String, String), Value>,
}

impl MockProvider {
    pub fn new() -> Self {
        Self {
            tick: 0,
            values: HashMap::new(),
        }
    }

    /// Controles de ejemplo de cada tipo, para probar la interfaz sin hardware.
    fn templates(kind: DeviceKind) -> Vec<Control> {
        let level = || Kind::Range { min: 0, max: 4, step: 1, unit: "", presets: vec![] };
        match kind {
            DeviceKind::Mouse => vec![
                Control::new(
                    "dpi",
                    Section::Pointer,
                    "DPI",
                    Kind::Range { min: 100, max: 26000, step: 50, unit: "DPI", presets: vec![400, 800, 1600, 3200] },
                    Some(Value::Int(1600)),
                )
                .volatile(),
                Control::new(
                    "rate",
                    Section::Pointer,
                    tr("TASA DE SONDEO", "REPORT RATE"),
                    choice([(125, "125 HZ".into()), (500, "500 HZ".into()), (1000, "1 KHZ".into())]),
                    Some(Value::Int(1000)),
                ),
            ],
            DeviceKind::Keyboard => vec![
                Control::new(
                    "light.effect",
                    Section::Lighting,
                    tr("EFECTO", "EFFECT"),
                    choice([(1, tr("FIJO", "STATIC").into()), (2, tr("RESPIRACIÓN", "BREATHING").into()), (3, tr("ARCOÍRIS", "RAINBOW").into())]),
                    Some(Value::Int(1)),
                )
                .trust(Trust::Sibling),
                Control::new("light.brightness", Section::Lighting, tr("BRILLO", "BRIGHTNESS"), level(), Some(Value::Int(3)))
                    .trust(Trust::Sibling),
                Control::new("light.color", Section::Lighting, tr("COLOR PERSONALIZADO", "CUSTOM COLOUR"), Kind::Color, Some(Value::rgb(255, 79, 0)))
                    .trust(Trust::Experimental)
                    .hint(tr("Control experimental de ejemplo.", "Sample experimental control.")),
                Control::new(
                    "power.sleep",
                    Section::Power,
                    tr("REPOSO Y APAGADO DE LUCES", "SLEEP AND LIGHTS OFF"),
                    choice([(0, tr("NUNCA", "NEVER").into()), (60, "1 MIN".into()), (300, "5 MIN".into()), (600, "10 MIN".into())]),
                    Some(Value::Int(300)),
                ),
            ],
            DeviceKind::Headset => vec![
                Control::new(
                    "eq.bands",
                    Section::Audio,
                    tr("ECUALIZADOR PERSONALIZADO", "CUSTOM EQUALISER"),
                    Kind::Bands {
                        labels: ["31", "63", "125", "250", "500", "1K", "2K", "4K", "8K", "16K"].map(String::from).to_vec(),
                        min: -6,
                        max: 6,
                        unit: "dB",
                        presets: vec![
                            BandPreset { label: tr("PLANO", "FLAT").into(), values: vec![0; 10] },
                            BandPreset { label: tr("JUEGO", "GAME").into(), values: vec![-3, -3, -4, 0, 5, 5, 4, 1, 0, -1] },
                        ],
                    },
                    Some(Value::List(vec![0; 10])),
                )
                .volatile(),
                Control::new("audio.enhancement", Section::Audio, tr("MEJORA DE AUDIO", "AUDIO ENHANCEMENT"), Kind::Toggle, Some(Value::Bool(true))),
            ],
            _ => Vec::new(),
        }
    }
}

impl DeviceProvider for MockProvider {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn discover(&mut self, _ctx: &PollContext) -> Result<Vec<DeviceInfo>, ProviderError> {
        self.tick = self.tick.wrapping_add(1);
        Ok(SPECS
            .iter()
            .map(|s| DeviceInfo {
                id: format!("mock:{}", s.key),
                fingerprint: format!("mock:{}", s.key),
                provider: "mock",
                vendor: "Demo".into(),
                name: s.name.into(),
                name_from_device: true,
                model: s.model.into(),
                kind: s.kind,
                connection: s.connection,
                via: s.via.map(Into::into),
                slot: s.via.map(|_| 1),
            })
            .collect())
    }

    fn battery(
        &mut self,
        _ctx: &PollContext,
        device: &DeviceInfo,
    ) -> Result<BatteryReading, ProviderError> {
        let spec = SPECS
            .iter()
            .find(|s| device.id.ends_with(s.key))
            .ok_or(ProviderError::NoResponse)?;
        let t = self.tick as i32;
        match spec.behaviour {
            Behaviour::Draining => {
                let p = (spec.start as i32 - t / 3).clamp(1, 100) as u8;
                Ok(BatteryReading::percent(p, ChargeState::Discharging))
            }
            Behaviour::Charging => {
                let p = (spec.start as i32 + t * 2).min(100) as u8;
                let state = if p >= 100 {
                    ChargeState::Full
                } else {
                    ChargeState::Charging
                };
                Ok(BatteryReading::percent(p, state))
            }
            Behaviour::Asleep => Err(ProviderError::Offline {
                last: Some(BatteryReading {
                    measured_at: Some(crate::model::now_ms() - 47 * 60 * 1000),
                    ..BatteryReading::percent(spec.start, ChargeState::Discharging)
                }),
            }),
            Behaviour::Locked => Err(ProviderError::Busy(
                "bloqueado por la app del fabricante".into(),
            )),
        }
    }

    fn capabilities(&self, device: &DeviceInfo) -> Capabilities {
        Capabilities {
            dpi: device.kind == DeviceKind::Mouse,
            lighting: device.kind == DeviceKind::Keyboard,
            audio: device.kind == DeviceKind::Headset,
            ..Capabilities::BATTERY_ONLY
        }
    }

    fn controls(&mut self, _ctx: &PollContext, device: &DeviceInfo) -> Result<DeviceControls, ProviderError> {
        let mut controls = Self::templates(device.kind);
        for c in &mut controls {
            if let Some(v) = self.values.get(&(device.id.clone(), c.id.to_string())) {
                c.value = Some(v.clone());
            }
        }
        Ok(DeviceControls {
            controls,
            source: tr(
                "Dispositivo simulado: los cambios solo se guardan en memoria.",
                "Simulated device: changes are only kept in memory.",
            )
            .into(),
            notes: Vec::new(),
        })
    }

    fn set_control(
        &mut self,
        _ctx: &PollContext,
        device: &DeviceInfo,
        control: &str,
        value: &Value,
    ) -> Result<(), ProviderError> {
        if !Self::templates(device.kind).iter().any(|c| c.id == control) {
            return Err(ProviderError::Unsupported(control.into()));
        }
        self.values.insert((device.id.clone(), control.to_string()), value.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_remember_what_was_set() {
        let ctx = PollContext { hid: None, razer_keep_eq: false, forced: true };
        let mut p = MockProvider::new();
        let devices = p.discover(&ctx).unwrap();
        let mouse = devices.iter().find(|d| d.id == "mock:ms01").unwrap();
        assert!(p.capabilities(mouse).configurable());
        assert!(!p.capabilities(devices.iter().find(|d| d.kind == DeviceKind::Gamepad).unwrap()).configurable());

        p.set_control(&ctx, mouse, "dpi", &Value::Int(3200)).unwrap();
        let c = p.controls(&ctx, mouse).unwrap();
        assert_eq!(c.controls.iter().find(|c| c.id == "dpi").unwrap().value, Some(Value::Int(3200)));
        assert!(p.set_control(&ctx, mouse, "light.effect", &Value::Int(1)).is_err());
    }
}
