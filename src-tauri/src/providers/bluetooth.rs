//! Provider Bluetooth (Windows).
//!
//! Dos fuentes, de más barata a más cara:
//!
//! 1. **Propiedad PnP de batería** `{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2`.
//!    Windows la rellena él mismo: con el Battery Service GATT (0x180F) en
//!    dispositivos BLE que gestiona, y con los indicadores HFP en auriculares
//!    clásicos. Es lo que enseña Configuración > Bluetooth. Leerla no genera
//!    tráfico de radio ni compite con nadie.
//! 2. **GATT directo** (WinRT `Windows.Devices.Bluetooth`): Battery Service 0x180F,
//!    característica Battery Level 0x2A19. Solo para BLE conectados con el servicio
//!    pero sin propiedad PnP.
//!
//! El estado de conexión sale de `{83DA6326-97A6-4088-9453-A1923F573B29} 15`.

use crate::model::{now_ms, BatteryReading, ChargeState, Connection, DeviceInfo, DeviceKind};
use crate::provider::{DeviceProvider, PollContext, ProviderError};
use std::collections::HashMap;

/// Los desconectados se siguen mostrando (con su última lectura) durante este tiempo.
const RECENT_MS: u64 = 7 * 24 * 3600 * 1000;

#[derive(Debug, Default, Clone)]
struct BtDevice {
    address: u64,
    name: Option<String>,
    connected: bool,
    battery: Option<u8>,
    battery_time: Option<u64>,
    gatt_battery: bool,
    le: bool,
    cod: Option<u32>,
    last_connected: Option<u64>,
}

pub struct BluetoothProvider {
    devices: HashMap<String, BtDevice>,
}

impl BluetoothProvider {
    pub fn new() -> Self {
        Self {
            devices: HashMap::new(),
        }
    }
}

fn clean_name(name: &str) -> String {
    let mut n = name.trim();
    for suffix in [" Hands-Free AG", " Hands-Free", " Avrcp Transport", " Stereo"] {
        n = n.strip_suffix(suffix).unwrap_or(n);
    }
    n.to_uppercase()
}

fn kind_of(cod: Option<u32>, name: &str) -> DeviceKind {
    if let Some(c) = cod {
        match (c >> 8) & 0x1F {
            0x04 => return DeviceKind::Headset,
            0x05 => {
                let minor = (c >> 2) & 0x3F;
                if minor & 0x10 != 0 {
                    return DeviceKind::Keyboard;
                }
                if minor & 0x20 != 0 {
                    return DeviceKind::Mouse;
                }
                if matches!(minor & 0x0F, 0x01 | 0x02) {
                    return DeviceKind::Gamepad;
                }
            }
            _ => {}
        }
    }
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["keyboard", "teclado", " kb", "keys"]) || n.ends_with("kb") {
        DeviceKind::Keyboard
    } else if has(&["mouse", "ratón", "raton", "mx master", "mx anywhere"]) {
        DeviceKind::Mouse
    } else if has(&["buds", "headset", "headphone", "auricular", "airpods", "wh-", "wf-"]) {
        DeviceKind::Headset
    } else if has(&["controller", "gamepad", "dualsense", "xbox"]) {
        DeviceKind::Gamepad
    } else {
        DeviceKind::Other
    }
}

fn format_address(a: u64) -> String {
    (0..6)
        .rev()
        .map(|i| format!("{:02X}", (a >> (i * 8)) & 0xFF))
        .collect::<Vec<_>>()
        .join(":")
}

impl DeviceProvider for BluetoothProvider {
    fn id(&self) -> &'static str {
        "bluetooth"
    }

    fn discover(&mut self, _ctx: &PollContext) -> Result<Vec<DeviceInfo>, ProviderError> {
        let now = now_ms();
        let scanned = pnp::scan();
        self.devices.clear();
        let mut out = Vec::new();
        for d in scanned.into_values() {
            // Ordenadores y teléfonos no son periféricos.
            if matches!(d.cod.map(|c| (c >> 8) & 0x1F), Some(0x01 | 0x02)) {
                continue;
            }
            if d.battery.is_none() && !(d.le && d.gatt_battery) {
                continue;
            }
            let recent = [d.last_connected, d.battery_time]
                .into_iter()
                .flatten()
                .max()
                .is_some_and(|t| now.saturating_sub(t) < RECENT_MS);
            if !d.connected && !recent {
                continue;
            }
            let raw_name = d.name.clone().unwrap_or_else(|| "BLUETOOTH".into());
            let id = format!("bluetooth:{:012x}", d.address);
            out.push(DeviceInfo {
                id: id.clone(),
                fingerprint: format!("bt:{:012x}", d.address),
                provider: "bluetooth",
                vendor: "Bluetooth".into(),
                name: clean_name(&raw_name),
                name_from_device: d.name.is_some(),
                model: format_address(d.address),
                kind: kind_of(d.cod, &raw_name),
                connection: Connection::Bluetooth,
                via: Some(if d.le {
                    "BLUETOOTH LE"
                } else {
                    crate::i18n::tr("BLUETOOTH CLÁSICO", "CLASSIC BLUETOOTH")
                }
                .into()),
                slot: None,
            });
            self.devices.insert(id, d);
        }
        Ok(out)
    }

    fn battery(
        &mut self,
        _ctx: &PollContext,
        device: &DeviceInfo,
    ) -> Result<BatteryReading, ProviderError> {
        let d = self
            .devices
            .get(&device.id)
            .ok_or(ProviderError::NoResponse)?;
        let reading = |p: u8, at: Option<u64>| BatteryReading {
            percent: Some(p.min(100)),
            approximate: false,
            // El Battery Service básico no informa de carga.
            charge: ChargeState::Unknown,
            voltage_mv: None,
            measured_at: at,
        };
        if !d.connected {
            return Err(ProviderError::Offline {
                last: d.battery.map(|p| reading(p, d.battery_time)),
            });
        }
        if let Some(p) = d.battery {
            return Ok(reading(p, None));
        }
        if d.le && d.gatt_battery {
            return gatt::read_battery_level(d.address).map(|p| reading(p, None));
        }
        Err(ProviderError::Unsupported("sin Battery Service".into()))
    }
}

// ---------------------------------------------------------------------------
// Enumeración PnP con CfgMgr32 (síncrona, sin WinRT, ~ms).
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod pnp {
    use super::BtDevice;
    use std::collections::HashMap;
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_PropertyW, CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW,
        CM_Locate_DevNodeW, CM_GETIDLIST_FILTER_ENUMERATOR, CM_GETIDLIST_FILTER_PRESENT,
        CM_LOCATE_DEVNODE_NORMAL, CR_BUFFER_SMALL, CR_SUCCESS,
    };
    use windows::Win32::Devices::Properties::DEVPROPTYPE;
    use windows::Win32::Foundation::DEVPROPKEY;

    const fn key(fmtid: u128, pid: u32) -> DEVPROPKEY {
        DEVPROPKEY {
            fmtid: GUID::from_u128(fmtid),
            pid,
        }
    }

    const BATTERY: DEVPROPKEY = key(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5, 2);
    const BATTERY_TIME: DEVPROPKEY = key(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5, 7);
    const CONNECTED: DEVPROPKEY = key(0x83da6326_97a6_4088_9453_a1923f573b29, 15);
    const FRIENDLY_NAME: DEVPROPKEY = key(0xa45c254e_df1c_4efd_8020_67d146a850e0, 14);
    const NAME: DEVPROPKEY = key(0xb725f130_47ef_101a_a5f1_02608c9eebac, 10);
    const BT_ADDRESS: DEVPROPKEY = key(0x2bd67d8b_8beb_48d5_87e0_6cda3428040a, 1);
    const BT_COD: DEVPROPKEY = key(0x2bd67d8b_8beb_48d5_87e0_6cda3428040a, 10);
    const BT_LAST_CONNECTED: DEVPROPKEY = key(0x2bd67d8b_8beb_48d5_87e0_6cda3428040a, 11);

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn device_ids(enumerator: &str) -> Vec<String> {
        let filter = wide(enumerator);
        let flags = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;
        let mut len = 0u32;
        unsafe {
            if CM_Get_Device_ID_List_SizeW(&mut len, PCWSTR(filter.as_ptr()), flags) != CR_SUCCESS
                || len == 0
            {
                return Vec::new();
            }
            let mut buf = vec![0u16; len as usize];
            if CM_Get_Device_ID_ListW(PCWSTR(filter.as_ptr()), &mut buf, flags) != CR_SUCCESS {
                return Vec::new();
            }
            buf.split(|&c| c == 0)
                .filter(|s| !s.is_empty())
                .map(String::from_utf16_lossy)
                .collect()
        }
    }

    fn locate(id: &str) -> Option<u32> {
        let w = wide(id);
        let mut inst = 0u32;
        let r = unsafe { CM_Locate_DevNodeW(&mut inst, PCWSTR(w.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) };
        (r == CR_SUCCESS).then_some(inst)
    }

    fn prop(inst: u32, key: &DEVPROPKEY) -> Option<Vec<u8>> {
        let mut ty = DEVPROPTYPE::default();
        let mut size = 0u32;
        unsafe {
            let r = CM_Get_DevNode_PropertyW(inst, key, &mut ty, None, &mut size, 0);
            if r != CR_BUFFER_SMALL || size == 0 {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            let r = CM_Get_DevNode_PropertyW(inst, key, &mut ty, Some(buf.as_mut_ptr()), &mut size, 0);
            if r != CR_SUCCESS {
                return None;
            }
            buf.truncate(size as usize);
            Some(buf)
        }
    }

    fn prop_string(inst: u32, key: &DEVPROPKEY) -> Option<String> {
        let b = prop(inst, key)?;
        let w: Vec<u16> = b
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        let s = String::from_utf16_lossy(&w).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    fn prop_u32(inst: u32, key: &DEVPROPKEY) -> Option<u32> {
        let b = prop(inst, key)?;
        (b.len() >= 4).then(|| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// FILETIME (100 ns desde 1601) → unix ms.
    fn prop_time(inst: u32, key: &DEVPROPKEY) -> Option<u64> {
        let b = prop(inst, key)?;
        if b.len() < 8 {
            return None;
        }
        let ft = u64::from_le_bytes(b[..8].try_into().ok()?);
        (ft / 10_000).checked_sub(11_644_473_600_000)
    }

    /// Último bloque de exactamente 12 dígitos hex del id de instancia.
    fn address_from_id(id: &str) -> Option<u64> {
        id.split(|c: char| !c.is_ascii_hexdigit())
            .filter(|t| t.len() == 12)
            .last()
            .and_then(|t| u64::from_str_radix(t, 16).ok())
    }

    pub(super) fn scan() -> HashMap<u64, BtDevice> {
        let mut map: HashMap<u64, BtDevice> = HashMap::new();
        for enumerator in ["BTHENUM", "BTHLE", "BTHLEDEVICE"] {
            for id in device_ids(enumerator) {
                let up = id.to_ascii_uppercase();
                let Some(inst) = locate(&id) else { continue };
                let address = prop_string(inst, &BT_ADDRESS)
                    .and_then(|s| u64::from_str_radix(&s, 16).ok())
                    .or_else(|| address_from_id(&up));
                let Some(address) = address.filter(|&a| a != 0) else {
                    continue;
                };
                let d = map.entry(address).or_insert_with(|| BtDevice {
                    address,
                    ..Default::default()
                });
                let is_root = up.starts_with("BTHENUM\\DEV_") || up.starts_with("BTHLE\\DEV_");
                if up.starts_with("BTHLE") {
                    d.le = true;
                }
                if up.starts_with("BTHLEDEVICE\\{0000180F-") {
                    d.gatt_battery = true;
                }
                if prop(inst, &CONNECTED).is_some_and(|b| b.first().is_some_and(|&v| v != 0)) {
                    d.connected = true;
                }
                if let Some(level) = prop(inst, &BATTERY).and_then(|b| b.first().copied()) {
                    let t = prop_time(inst, &BATTERY_TIME);
                    if d.battery.is_none() || t > d.battery_time {
                        d.battery = Some(level);
                        d.battery_time = t;
                    }
                }
                if is_root {
                    if let Some(n) =
                        prop_string(inst, &FRIENDLY_NAME).or_else(|| prop_string(inst, &NAME))
                    {
                        d.name = Some(n);
                    }
                    d.cod = prop_u32(inst, &BT_COD).or(d.cod);
                    d.last_connected = prop_time(inst, &BT_LAST_CONNECTED).max(d.last_connected);
                } else if d.name.is_none() && up.starts_with("BTHENUM\\{") {
                    // Nodo de perfil (p. ej. "... Hands-Free AG"): nombre de reserva.
                    d.name = prop_string(inst, &FRIENDLY_NAME);
                }
            }
        }
        map
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn address_from_instance_ids() {
            let hfp = r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_VID&00010094_PID&0004\8&393C1EFF&0&4873CBFBA623_C00000000";
            assert_eq!(super::address_from_id(hfp), Some(0x4873CBFBA623));
            let gatt = r"BTHLEDEVICE\{0000180F-0000-1000-8000-00805F9B34FB}_DEV_VID&023554_PID&FA07_REV&6701_CF3FE2BCB1F3\9&30404C23&0&000F";
            assert_eq!(super::address_from_id(gatt), Some(0xCF3FE2BCB1F3));
        }
    }
}

// ---------------------------------------------------------------------------
// Lectura GATT directa con WinRT.
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod gatt {
    use crate::provider::ProviderError;
    use std::sync::mpsc;
    use std::time::Duration;
    use windows::Devices::Bluetooth::GenericAttributeProfile::{
        GattCharacteristicUuids, GattCommunicationStatus, GattServiceUuids,
    };
    use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothLEDevice};
    use windows::Storage::Streams::DataReader;
    use windows_future::{AsyncOperationCompletedHandler, IAsyncOperation};

    const TIMEOUT: Duration = Duration::from_secs(6);

    fn win(e: windows::core::Error) -> ProviderError {
        ProviderError::Io(e.message().to_string())
    }

    /// Espera una operación WinRT con límite de tiempo, sin bloquear de más:
    /// el handler de finalización despierta el canal.
    fn wait<T: windows::core::RuntimeType + 'static>(
        op: IAsyncOperation<T>,
    ) -> Result<T, ProviderError> {
        let (tx, rx) = mpsc::channel::<()>();
        op.SetCompleted(&AsyncOperationCompletedHandler::new(move |_, _| {
            let _ = tx.send(());
            Ok(())
        }))
        .map_err(win)?;
        match rx.recv_timeout(TIMEOUT) {
            Ok(()) => op.GetResults().map_err(win),
            Err(_) => {
                let _ = op.Cancel();
                Err(ProviderError::NoResponse)
            }
        }
    }

    fn check(status: GattCommunicationStatus) -> Result<(), ProviderError> {
        if status == GattCommunicationStatus::Success {
            Ok(())
        } else if status == GattCommunicationStatus::AccessDenied {
            Err(ProviderError::Busy("Windows deniega el acceso al servicio GATT".into()))
        } else if status == GattCommunicationStatus::Unreachable {
            Err(ProviderError::Offline { last: None })
        } else {
            Err(ProviderError::Protocol(format!("GATT {}", status.0)))
        }
    }

    pub(super) fn read_battery_level(address: u64) -> Result<u8, ProviderError> {
        let dev = wait(BluetoothLEDevice::FromBluetoothAddressAsync(address).map_err(win)?)?;
        let services = wait(
            dev.GetGattServicesForUuidWithCacheModeAsync(
                GattServiceUuids::Battery().map_err(win)?,
                BluetoothCacheMode::Cached,
            )
            .map_err(win)?,
        )?;
        check(services.Status().map_err(win)?)?;
        let list = services.Services().map_err(win)?;
        if list.Size().map_err(win)? == 0 {
            return Err(ProviderError::Unsupported("sin Battery Service".into()));
        }
        let service = list.GetAt(0).map_err(win)?;
        let chars = wait(
            service
                .GetCharacteristicsForUuidWithCacheModeAsync(
                    GattCharacteristicUuids::BatteryLevel().map_err(win)?,
                    BluetoothCacheMode::Cached,
                )
                .map_err(win)?,
        )?;
        check(chars.Status().map_err(win)?)?;
        let chars = chars.Characteristics().map_err(win)?;
        if chars.Size().map_err(win)? == 0 {
            return Err(ProviderError::Unsupported("sin característica 0x2A19".into()));
        }
        let value = wait(
            chars
                .GetAt(0)
                .map_err(win)?
                .ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)
                .map_err(win)?,
        )?;
        check(value.Status().map_err(win)?)?;
        let reader = DataReader::FromBuffer(&value.Value().map_err(win)?).map_err(win)?;
        let level = reader.ReadByte().map_err(win)?;
        let _ = service.Close();
        let _ = dev.Close();
        Ok(level.min(100))
    }
}

#[cfg(not(windows))]
mod pnp {
    pub(super) fn scan() -> std::collections::HashMap<u64, super::BtDevice> {
        Default::default()
    }
}

#[cfg(not(windows))]
mod gatt {
    pub(super) fn read_battery_level(_a: u64) -> Result<u8, crate::provider::ProviderError> {
        Err(crate::provider::ProviderError::Unsupported("solo Windows".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_from_class_of_device() {
        // Redmi Buds (audio) y DualSense (periférico/gamepad), valores reales.
        assert_eq!(kind_of(Some(2360324), ""), DeviceKind::Headset);
        assert_eq!(kind_of(Some(9480), ""), DeviceKind::Gamepad);
        assert_eq!(kind_of(None, "BT5.0 KB"), DeviceKind::Keyboard);
    }

    #[test]
    fn cleans_profile_suffix() {
        assert_eq!(clean_name("Redmi Buds 5 Pro Hands-Free AG"), "REDMI BUDS 5 PRO");
        assert_eq!(format_address(0x4873CBFBA623), "48:73:CB:FB:A6:23");
    }
}
