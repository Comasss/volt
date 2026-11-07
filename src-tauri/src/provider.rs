//! El contrato que implementa cada fabricante o protocolo.
//!
//! Los providers son síncronos a propósito: el gestor los llama desde un único hilo
//! de sondeo que pasa casi todo el tiempo dormido en `recv_timeout`, así que no hace
//! falta un runtime async ni hilos extra por provider.

use crate::hid::HidContext;
use crate::model::{BatteryReading, DeviceInfo};
use std::ffi::CString;
use std::fmt;

/// Recursos compartidos durante un ciclo de sondeo.
pub struct PollContext<'a> {
    /// Enumeración HID hecha una sola vez por ciclo y compartida por todos los
    /// providers HID. `None` si hidapi no se pudo inicializar.
    pub hid: Option<&'a HidContext>,
    /// Ajuste "BlackShark: mantener EQ sin Synapse". Es lo único que escribe en
    /// un dispositivo; con `false` todos los providers solo leen.
    pub razer_keep_eq: bool,
    /// El sondeo lo ha pedido el usuario (botón Escanear) o un cambio de
    /// dispositivos: los providers que espacian sus consultas deben leer ya.
    pub forced: bool,
}

#[derive(Debug, Clone)]
pub enum ProviderError {
    /// Emparejado pero no alcanzable (dormido, apagado, fuera de rango).
    /// Puede traer la última lectura conocida por el sistema.
    Offline { last: Option<BatteryReading> },
    /// Otro proceso tiene el dispositivo o respondió "ocupado".
    Busy(String),
    /// No contestó dentro del tiempo límite.
    NoResponse,
    /// El dispositivo no expone batería por este protocolo.
    Unsupported(String),
    Protocol(String),
    Io(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use crate::i18n::tr;
        match self {
            ProviderError::Offline { .. } => {
                write!(f, "{}", tr("Desconectado o en reposo", "Disconnected or asleep"))
            }
            ProviderError::Busy(s) => write!(f, "{}: {s}", tr("Ocupado", "Busy")),
            ProviderError::NoResponse => write!(f, "{}", tr("No responde", "Not responding")),
            ProviderError::Unsupported(s) => write!(f, "{}: {s}", tr("Sin soporte", "Unsupported")),
            ProviderError::Protocol(s) => write!(f, "{}: {s}", tr("Error de protocolo", "Protocol error")),
            ProviderError::Io(s) => write!(f, "I/O: {s}"),
        }
    }
}

/// Lo que un dispositivo puede contar por sí solo.
#[derive(Debug, Clone, PartialEq)]
pub enum ListenEvent {
    Reading(BatteryReading),
    /// Se ha apagado o perdido el enlace: se quita de la lista al momento.
    Gone,
    /// Ha vuelto (p. ej. enlace recuperado): conviene leerlo ya.
    Refresh,
}

/// Escucha en segundo plano de un dispositivo que **empuja** su estado (batería,
/// carga, enlace) sin que se le pregunte. El gestor abre `path` en un hilo que
/// queda bloqueado en la lectura (0 % de CPU) y pasa cada informe a `parser`.
pub struct ListenSpec {
    pub path: CString,
    /// Informe de entrada → evento (`None` si no interesa). Puede guardar estado
    /// entre informes (p. ej. nivel y carga llegan en eventos distintos).
    pub parser: Box<dyn FnMut(&[u8]) -> Option<ListenEvent> + Send>,
    /// Para dispositivos que informan periódicamente: si pasa este tiempo sin
    /// noticias, se considera apagado (`Gone`).
    pub silence: Option<std::time::Duration>,
}

/// Qué sabe hacer un provider con un dispositivo concreto.
#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    pub battery: bool,
    pub dpi: bool,
    pub lighting: bool,
    pub polling_rate: bool,
}

impl Capabilities {
    pub const BATTERY_ONLY: Self = Self {
        battery: true,
        dpi: false,
        lighting: false,
        polling_rate: false,
    };
}

/// Parámetros de iluminación para el futuro `set_lighting`.
#[derive(Debug, Clone, Copy)]
pub enum Lighting {
    Off,
    Static { r: u8, g: u8, b: u8 },
}

pub trait DeviceProvider {
    /// Identificador corto y estable ("logitech", "razer", "bluetooth"...).
    fn id(&self) -> &'static str;

    /// Enumera los dispositivos que este provider puede atender ahora mismo.
    /// Debe ser barato: se llama una vez por ciclo.
    fn discover(&mut self, ctx: &PollContext) -> Result<Vec<DeviceInfo>, ProviderError>;

    /// Lee batería y estado de carga de un dispositivo devuelto por `discover`.
    fn battery(
        &mut self,
        ctx: &PollContext,
        device: &DeviceInfo,
    ) -> Result<BatteryReading, ProviderError>;

    /// Se llama al terminar cada ciclo. Aquí se cierran los handles para que nada
    /// quede abierto mientras la app duerme.
    fn release(&mut self) {}

    /// Para el diagnóstico: ¿este provider se encarga de esta interfaz HID?
    fn claims_hid(&self, _vid: u16, _pid: u16, _usage_page: u16, _usage: u16) -> bool {
        false
    }

    /// ¿Un `Offline` de este provider es fiable? Si no (lecturas que a veces se
    /// pierden), el dispositivo conserva su estado y solo se da por apagado tras
    /// varios sondeos seguidos sin respuesta.
    fn offline_is_reliable(&self) -> bool {
        true
    }

    /// Si el dispositivo informa por sí solo de cambios (p. ej. al conectar el
    /// cable), qué escuchar. `current` es la última lectura conocida.
    fn listener(&self, _device: &DeviceInfo, _current: Option<&BatteryReading>) -> Option<ListenSpec> {
        None
    }

    // ---- Preparado para crecer: por defecto no soportado. ----

    fn capabilities(&self, _device: &DeviceInfo) -> Capabilities {
        Capabilities::BATTERY_ONLY
    }

    fn set_dpi(
        &mut self,
        _ctx: &PollContext,
        _device: &DeviceInfo,
        _dpi: u32,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported("set_dpi".into()))
    }

    fn set_lighting(
        &mut self,
        _ctx: &PollContext,
        _device: &DeviceInfo,
        _lighting: Lighting,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported("set_lighting".into()))
    }

    fn set_polling_rate(
        &mut self,
        _ctx: &PollContext,
        _device: &DeviceInfo,
        _hz: u16,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported("set_polling_rate".into()))
    }
}
