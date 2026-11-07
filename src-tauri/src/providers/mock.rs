//! Dispositivos simulados para desarrollar la interfaz sin hardware.
//! Se activan en Ajustes ("Dispositivos demo") o arrancando con `--mock`.

use crate::model::{BatteryReading, ChargeState, Connection, DeviceInfo, DeviceKind};
use crate::provider::{DeviceProvider, PollContext, ProviderError};

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
}

impl MockProvider {
    pub fn new() -> Self {
        Self { tick: 0 }
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
}
