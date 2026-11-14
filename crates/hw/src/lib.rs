//! Settings that live in the mouse's own firmware: sensor resolution (DPI)
//! and report rate. These are not input events, so the remapper cannot
//! change them; they are set by vendor-specific commands.
//!
//! One backend exists so far: Logitech's HID++ 2.0 protocol, spoken
//! directly over the mouse's hidraw node. Other vendors, or ratbagd, would
//! be further implementations of [`Hardware`].

mod hidpp;
mod protocol;

use mouse_profiles_core::{DeviceId, HardwareInfo};

pub trait Hardware {
    /// Reads the current settings and what the mouse accepts.
    fn info(&mut self) -> Result<HardwareInfo, String>;
    /// Sets the sensor resolution. `dpi` must be one of the accepted values.
    fn set_dpi(&mut self, dpi: u32) -> Result<(), String>;
    /// Sets the report rate. `hz` must be one of the accepted values.
    fn set_polling_hz(&mut self, hz: u32) -> Result<(), String>;
}

const LOGITECH: u16 = 0x046d;

/// Whether any backend could ever talk to a mouse with one of these IDs.
/// When this is false there is no point in trying [`open`] again later.
pub fn is_supported(ids: &[DeviceId]) -> bool {
    ids.iter().any(|id| id.vendor == LOGITECH)
}

/// Opens a hardware backend for the mouse with one of these IDs.
///
/// `Ok(None)` means there is nothing to talk to: the mouse is not connected,
/// or no backend supports it. An error means it should have worked (a
/// supported, connected mouse) but did not, typically a permission problem.
pub fn open(ids: &[DeviceId]) -> Result<Option<Box<dyn Hardware>>, String> {
    for id in ids.iter().filter(|id| id.vendor == LOGITECH) {
        if let Some(device) = hidpp::open(*id)? {
            return Ok(Some(Box::new(device)));
        }
    }
    Ok(None)
}
