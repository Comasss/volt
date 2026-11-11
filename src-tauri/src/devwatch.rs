//! Aviso de Windows cuando aparece o desaparece una interfaz HID (cable enchufado,
//! receptor conectado, dispositivo Bluetooth que conecta...).
//!
//! `CM_Register_Notification` llama a un callback desde un hilo del sistema: no hay
//! sondeo ni ventana oculta. El callback solo manda un mensaje al gestor, que espera
//! un momento a que el dispositivo termine de enumerarse y lee.

#[cfg(windows)]
use std::sync::OnceLock;

#[cfg(windows)]
type OnChange = Box<dyn Fn() + Send + Sync>;
#[cfg(windows)]
static ON_CHANGE: OnceLock<OnChange> = OnceLock::new();

#[cfg(windows)]
pub fn start(on_change: impl Fn() + Send + Sync + 'static) {
    use windows::core::GUID;
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Register_Notification, CM_NOTIFY_ACTION, CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
        CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER,
        CM_NOTIFY_FILTER_0, CM_NOTIFY_FILTER_0_0, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
        CR_SUCCESS, HCMNOTIFICATION,
    };

    // GUID_DEVINTERFACE_HID
    const HID: GUID = GUID::from_u128(0x4d1e55b2_f16f_11cf_88cb_001111000030);

    unsafe extern "system" fn callback(
        _h: HCMNOTIFICATION,
        _ctx: *const core::ffi::c_void,
        action: CM_NOTIFY_ACTION,
        _data: *const CM_NOTIFY_EVENT_DATA,
        _size: u32,
    ) -> u32 {
        if action == CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL
            || action == CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL
        {
            if let Some(f) = ON_CHANGE.get() {
                f();
            }
        }
        0
    }

    if ON_CHANGE.set(Box::new(on_change)).is_err() {
        return;
    }
    let filter = CM_NOTIFY_FILTER {
        cbSize: std::mem::size_of::<CM_NOTIFY_FILTER>() as u32,
        Flags: 0,
        FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
        Reserved: 0,
        u: CM_NOTIFY_FILTER_0 {
            DeviceInterface: CM_NOTIFY_FILTER_0_0 { ClassGuid: HID },
        },
    };
    let mut handle = HCMNOTIFICATION::default();
    let r = unsafe { CM_Register_Notification(&filter, None, Some(callback), &mut handle) };
    if r != CR_SUCCESS {
        log::warn!("devwatch: CM_Register_Notification falló ({r:?})");
    }
    // El registro dura lo que el proceso: no se libera.
}

#[cfg(not(windows))]
pub fn start(_on_change: impl Fn() + Send + Sync + 'static) {}
