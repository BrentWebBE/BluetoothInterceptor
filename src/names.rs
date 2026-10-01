//! Friendly names for a handful of well-known GATT UUIDs.
//!
//! This is purely cosmetic: it makes the live log readable ("Battery Level"
//! instead of a bare 128-bit UUID). It is intentionally a small, curated subset
//! of the Bluetooth SIG assigned numbers — enough to make common devices legible
//! without shipping the whole registry.

use bluer::Uuid;

/// The Bluetooth SIG base UUID `00000000-0000-1000-8000-00805F9B34FB` as a
/// `u128`. A SIG 16-bit UUID `0000XXXX-...` is this value with the 16-bit id
/// placed in bits 96..112.
const SIG_BASE: u128 = 0x0000_0000_0000_1000_8000_0080_5f9b_34fb;

/// Mask covering the 16-bit id field (bits 96..112).
const ID_MASK: u128 = 0xffff << 96;

/// The SIG 16-bit id for a UUID, if it is a SIG base UUID. Public for callers
/// that need to recognise reserved services (e.g. 0x1800/0x1801).
pub fn sig_id(uuid: &Uuid) -> Option<u16> {
    short_id(uuid)
}

/// If `uuid` is a SIG 16-bit UUID, return the short id, else `None`.
fn short_id(uuid: &Uuid) -> Option<u16> {
    let v = uuid.as_u128();
    // Everything outside the id field must match the SIG base exactly.
    if v & !ID_MASK == SIG_BASE {
        Some(((v >> 96) & 0xffff) as u16)
    } else {
        None
    }
}

/// Human-readable label for a UUID, or `None` if we don't recognise it.
pub fn label(uuid: &Uuid) -> Option<&'static str> {
    let id = short_id(uuid)?;
    Some(match id {
        // Services
        0x1800 => "Generic Access",
        0x1801 => "Generic Attribute",
        0x180a => "Device Information",
        0x180f => "Battery Service",
        0x180d => "Heart Rate",
        0x1809 => "Health Thermometer",
        0x1812 => "Human Interface Device",
        0x1816 => "Cycling Speed and Cadence",
        0x181a => "Environmental Sensing",
        0xfe59 => "Nordic DFU",
        // Characteristics
        0x2a00 => "Device Name",
        0x2a01 => "Appearance",
        0x2a04 => "Peripheral Preferred Connection Parameters",
        0x2a05 => "Service Changed",
        0x2a19 => "Battery Level",
        0x2a29 => "Manufacturer Name String",
        0x2a24 => "Model Number String",
        0x2a25 => "Serial Number String",
        0x2a27 => "Hardware Revision String",
        0x2a26 => "Firmware Revision String",
        0x2a28 => "Software Revision String",
        0x2a37 => "Heart Rate Measurement",
        0x2a38 => "Body Sensor Location",
        0x2a6e => "Temperature",
        0x2a6f => "Humidity",
        // Descriptors
        0x2901 => "Characteristic User Description",
        0x2902 => "Client Characteristic Configuration",
        0x2904 => "Characteristic Presentation Format",
        _ => return None,
    })
}

/// A label suitable for logs: the friendly name if known, otherwise the UUID.
pub fn describe(uuid: &Uuid) -> String {
    match label(uuid) {
        Some(name) => format!("{name} [{uuid}]"),
        None => uuid.to_string(),
    }
}
