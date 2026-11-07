//! Tabla de fabricantes y dispositivos conocidos (VID/PID).
//!
//! Para añadir un dispositivo basta con una línea en `DEVICES`. Usa
//! `cargo run --bin hid-list` para ver los VID/PID de lo que tienes conectado.

use crate::model::DeviceKind;

/// Protocolo con el que se habla con el dispositivo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// HID++ 1.0/2.0 (Logitech).
    LogitechHidpp,
    /// Informe de 90 bytes de openrazer (ratones/teclados Razer).
    Razer,
    /// Receptores Compx 2.4G (Redragon, AULA...): informe 0x13, comando 0x4A.
    Compx,
    /// Receptores 1D57 (Attack Shark X6/X7…): informe de entrada 0x03, código 0x4010.
    Xenta,
    // Huecos preparados (sin provider todavía):
    SteelSeries,
    Corsair,
    HyperX,
    /// Conocido, pero sin protocolo de batería implementado.
    Unknown,
}

pub struct Vendor {
    pub vid: u16,
    pub name: &'static str,
}

pub const VENDORS: &[Vendor] = &[
    Vendor { vid: 0x046D, name: "Logitech" },
    Vendor { vid: 0x1532, name: "Razer" },
    Vendor { vid: 0x1038, name: "SteelSeries" },
    Vendor { vid: 0x1B1C, name: "Corsair" },
    Vendor { vid: 0x0951, name: "HyperX (Kingston)" },
    Vendor { vid: 0x03F0, name: "HyperX (HP)" },
    Vendor { vid: 0x1E7D, name: "ROCCAT / Turtle Beach" },
    Vendor { vid: 0x0B05, name: "ASUS" },
    Vendor { vid: 0x054C, name: "Sony" },
    Vendor { vid: 0x045E, name: "Microsoft" },
    Vendor { vid: 0x3554, name: "Compx (VXE / ATK / MCHOSE)" },
    Vendor { vid: 0x3434, name: "Keychron" },
    Vendor { vid: 0x0FD9, name: "Elgato" },
    Vendor { vid: 0x1462, name: "MSI" },
    Vendor { vid: 0x2516, name: "Cooler Master" },
    Vendor { vid: 0x258A, name: "SINO WEALTH" },
    Vendor { vid: 0x1D57, name: "Xenta (Attack Shark…)" },
];

pub struct KnownDevice {
    pub vid: u16,
    pub pid: u16,
    pub name: &'static str,
    pub kind: DeviceKind,
    pub protocol: Protocol,
    /// 0 = dispositivo final. >0 = receptor con N ranuras de emparejamiento.
    pub receiver_slots: u8,
}

const fn receiver(vid: u16, pid: u16, name: &'static str, slots: u8) -> KnownDevice {
    KnownDevice {
        vid,
        pid,
        name,
        kind: DeviceKind::Other,
        protocol: Protocol::LogitechHidpp,
        receiver_slots: slots,
    }
}

const fn device(
    vid: u16,
    pid: u16,
    name: &'static str,
    kind: DeviceKind,
    protocol: Protocol,
) -> KnownDevice {
    KnownDevice {
        vid,
        pid,
        name,
        kind,
        protocol,
        receiver_slots: 0,
    }
}

pub const DEVICES: &[KnownDevice] = &[
    // --- Logitech: receptores (fuente: Solaar, base_usb.py) ---
    receiver(0x046D, 0xC52B, "UNIFYING RECEIVER", 6),
    receiver(0x046D, 0xC532, "UNIFYING RECEIVER", 6),
    receiver(0x046D, 0xC548, "BOLT RECEIVER", 6),
    receiver(0x046D, 0xC52F, "NANO RECEIVER", 1),
    receiver(0x046D, 0xC534, "NANO RECEIVER", 2),
    receiver(0x046D, 0xC539, "LIGHTSPEED RECEIVER", 2),
    receiver(0x046D, 0xC53A, "LIGHTSPEED RECEIVER", 2),
    receiver(0x046D, 0xC53F, "LIGHTSPEED RECEIVER", 2),
    receiver(0x046D, 0xC541, "LIGHTSPEED RECEIVER", 2),
    receiver(0x046D, 0xC545, "LIGHTSPEED RECEIVER", 2),
    receiver(0x046D, 0xC547, "LIGHTSPEED RECEIVER", 2),
    receiver(0x046D, 0xC54D, "LIGHTSPEED RECEIVER", 2),
    // --- Logitech: modo cable de ratones LIGHTSPEED (HID++ en índice 0xFF) ---
    device(0x046D, 0xC08D, "G502 LIGHTSPEED", DeviceKind::Mouse, Protocol::LogitechHidpp),
    device(0x046D, 0xC088, "G PRO WIRELESS", DeviceKind::Mouse, Protocol::LogitechHidpp),
    device(0x046D, 0xC094, "PRO X SUPERLIGHT", DeviceKind::Mouse, Protocol::LogitechHidpp),
    // --- Razer: el provider sondea cualquier 1532; aquí solo nombres bonitos ---
    device(0x1532, 0x0555, "BLACKSHARK V2 PRO", DeviceKind::Headset, Protocol::Unknown),
    device(0x1532, 0x0528, "BLACKSHARK V2 PRO", DeviceKind::Headset, Protocol::Unknown),
    // --- Compx: receptores 2.4G compartidos por varias marcas de teclados ---
    device(0x3554, 0xFA09, "RECEPTOR COMPX COMPARTIDO (REDRAGON, AULA, MCHOSE…)", DeviceKind::Keyboard, Protocol::Compx),
    // --- 1D57 "Xenta": PIDs genéricos del chip (Attack Shark y otras marcas). El
    //     modelo exacto no se puede deducir del PID: nombre genérico. ---
    device(0x1D57, 0xFA60, "RECEPTOR 2.4G ATTACK SHARK (X6, X7…)", DeviceKind::Mouse, Protocol::Xenta),
    device(0x1D57, 0xFA61, "ATTACK SHARK X6 (CABLE)", DeviceKind::Mouse, Protocol::Xenta),
    // --- Huecos para próximos providers (PIDs según HeadsetControl) ---
    device(0x1038, 0x12AD, "ARCTIS 7", DeviceKind::Headset, Protocol::SteelSeries),
    device(0x1B1C, 0x0A14, "VOID PRO WIRELESS", DeviceKind::Headset, Protocol::Corsair),
    device(0x0951, 0x1718, "CLOUD II WIRELESS", DeviceKind::Headset, Protocol::HyperX),
];

pub fn vendor_name(vid: u16) -> Option<&'static str> {
    VENDORS.iter().find(|v| v.vid == vid).map(|v| v.name)
}

pub fn lookup(vid: u16, pid: u16) -> Option<&'static KnownDevice> {
    DEVICES.iter().find(|d| d.vid == vid && d.pid == pid)
}
