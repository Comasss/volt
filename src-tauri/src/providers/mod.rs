//! Registro de providers.
//!
//! Para añadir un fabricante nuevo (SteelSeries, Corsair, HyperX...):
//! 1. Crea `providers/<fabricante>.rs` con un struct que implemente `DeviceProvider`.
//!    Usa `ctx.hid` (enumeración compartida) y `hid::interface_key` para agrupar
//!    colecciones, igual que `logitech` o `razer`.
//! 2. Añade sus VID/PID a `device_ids.rs` con el `Protocol` correspondiente.
//! 3. Regístralo en `all()`.
//!
//! Para auriculares (SteelSeries Arctis, Corsair Void, HyperX Cloud...) la mejor
//! referencia de protocolos es HeadsetControl (`src/devices/*.c`): casi todos son
//! un output report en una colección vendor (0xFF00+) y una respuesta de entrada.

pub mod bluetooth;
pub mod compx;
pub mod compx_cfg;
pub mod logitech;
pub mod mock;
pub mod razer;
pub mod razer_mxic;
pub mod xenta;

use crate::provider::DeviceProvider;

/// Providers de hardware real. El orden es el orden de sondeo.
pub fn all() -> Vec<Box<dyn DeviceProvider>> {
    vec![
        Box::new(logitech::LogitechProvider::new()),
        Box::new(razer::RazerProvider::new()),
        Box::new(compx::CompxProvider::new()),
        Box::new(bluetooth::BluetoothProvider::new()),
        // Último: espera pasiva de hasta 5 s a que el ratón mande su informe.
        Box::new(xenta::XentaProvider::new()),
        // Box::new(steelseries::SteelSeriesProvider::new()),
        // Box::new(corsair::CorsairProvider::new()),
        // Box::new(hyperx::HyperXProvider::new()),
    ]
}
