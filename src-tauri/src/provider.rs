//! El contrato que implementa cada fabricante o protocolo.
//!
//! Los providers son síncronos a propósito: el gestor los llama desde un único hilo
//! de sondeo que pasa casi todo el tiempo dormido en `recv_timeout`, así que no hace
//! falta un runtime async ni hilos extra por provider.

use crate::hid::HidContext;
use crate::controls::{DeviceControls, Value};
use crate::model::{BatteryReading, DeviceInfo};
use std::ffi::CString;
use std::fmt;

/// Recursos compartidos durante un ciclo de sondeo.
pub struct PollContext<'a> {
    /// Enumeración HID hecha una sola vez por ciclo y compartida por todos los
    /// providers HID. `None` si hidapi no se pudo inicializar.
    pub hid: Option<&'a HidContext>,
    /// Ajuste "BlackShark: mantener EQ sin Synapse". Es lo único que se escribe
    /// durante el sondeo; con `false`, `discover` y `battery` solo leen.
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

/// Qué sabe hacer un provider con un dispositivo concreto, sin tocar el hardware
/// (la lista detallada de controles la da `controls`, que sí lo lee).
#[derive(Debug, Clone, Copy, Default)]
pub struct Capabilities {
    pub battery: bool,
    pub dpi: bool,
    pub lighting: bool,
    pub polling_rate: bool,
    /// Tiempo de reposo / apagado de luces.
    pub sleep: bool,
    /// Ecualizador y otros ajustes de audio.
    pub audio: bool,
}

impl Capabilities {
    pub const BATTERY_ONLY: Self = Self {
        battery: true,
        dpi: false,
        lighting: false,
        polling_rate: false,
        sleep: false,
        audio: false,
    };

    /// ¿Hay algo que personalizar? Decide si la ficha enseña el botón.
    pub fn configurable(&self) -> bool {
        self.dpi || self.lighting || self.polling_rate || self.sleep || self.audio
    }
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

    // ---- Personalización: por defecto, nada. ----

    fn capabilities(&self, _device: &DeviceInfo) -> Capabilities {
        Capabilities::BATTERY_ONLY
    }

    /// Lee del dispositivo todo lo que se puede ajustar y su valor actual. Se llama
    /// fuera del ciclo de sondeo (al abrir la ficha o tras una escritura), con los
    /// datos que dejó el último `discover`. Después se llama a `release`.
    fn controls(
        &mut self,
        _ctx: &PollContext,
        _device: &DeviceInfo,
    ) -> Result<DeviceControls, ProviderError> {
        Err(ProviderError::Unsupported("controls".into()))
    }

    /// Escribe un control. No hace falta que devuelva el valor final: el gestor
    /// vuelve a llamar a `controls` y enseña lo que contesta el dispositivo.
    fn set_control(
        &mut self,
        _ctx: &PollContext,
        _device: &DeviceInfo,
        _control: &str,
        _value: &Value,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Unsupported("set_control".into()))
    }

    /// Nombre de la app del fabricante si está abierta ahora (G HUB, Synapse…).
    /// Con ella abierta Volt no reaplica nada por su cuenta: mandaría la app.
    fn vendor_app(&self) -> Option<&'static str> {
        None
    }
}
