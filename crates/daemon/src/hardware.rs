//! DPI and report rate: applying what `config.toml` asks for when a mouse
//! connects, and changing them on request.
//!
//! Talking to the mouse can take hundreds of milliseconds (or time out if it
//! is asleep), so none of it happens while the shared state is locked;
//! otherwise mouse movement would stall behind it.

use std::time::{Duration, Instant};

use mousekit_core::{DeviceId, HardwareInfo};

use crate::{Shared, State, lock, log};

/// How long to wait between attempts to read a mouse that did not answer.
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
/// How many more attempts to make after the first, per connection.
const RETRIES: u32 = 30;

/// What to do to one mouse.
#[derive(Default)]
struct Wanted {
    dpi: Option<u32>,
    polling_hz: Option<u32>,
}

/// Opens the mouse, applies `wanted` where it differs from the current
/// settings, and returns the settings afterwards. `Ok(None)` means no
/// hardware backend can talk to this mouse.
fn apply(ids: &[DeviceId], wanted: &Wanted) -> Result<Option<HardwareInfo>, String> {
    let Some(mut hardware) = mousekit_hw::open(ids)? else {
        return Ok(None);
    };
    let before = hardware.info()?;
    let mut changed = false;
    if let Some(dpi) = wanted.dpi
        && dpi != before.dpi
    {
        if !before.dpi_values.contains(&dpi) {
            return Err(format!("this mouse does not support {dpi} DPI"));
        }
        hardware.set_dpi(dpi)?;
        changed = true;
    }
    if let Some(hz) = wanted.polling_hz
        && hz != before.polling_hz
    {
        if !before.polling_values.contains(&hz) {
            return Err(format!("this mouse does not support {hz} Hz"));
        }
        hardware.set_polling_hz(hz)?;
        changed = true;
    }
    // Read back rather than assume, so what is shown is what the mouse says.
    let after = if changed { hardware.info()? } else { before };
    Ok(Some(after))
}

/// Called when a mouse connects: starts reading its sensor settings in the
/// background, so the first mouse events are not delayed behind it.
pub(crate) fn start_sync(shared: &Shared, device_index: usize) {
    {
        let mut state = lock(shared);
        let supported = state
            .loaded
            .config
            .devices
            .get(device_index)
            .is_some_and(|device| mousekit_hw::is_supported(&device.ids));
        if !supported {
            return;
        }
        if let Some(retry) = state.hardware_retry.get_mut(device_index) {
            *retry = Some((Instant::now() + RETRY_INTERVAL, RETRIES));
        }
    }
    let shared = shared.clone();
    std::thread::spawn(move || sync(&shared, device_index));
}

/// True when the mouse's settings are still unknown and it is time for
/// another attempt. Claims the attempt, so only one caller gets `true`.
pub(crate) fn retry_is_due(state: &mut State, device_index: usize) -> bool {
    if state.hardware.get(device_index).is_none_or(Option::is_some) {
        return false;
    }
    let Some(Some((due, left))) = state.hardware_retry.get_mut(device_index) else {
        return false;
    };
    if *left == 0 || Instant::now() < *due {
        return false;
    }
    *due = Instant::now() + RETRY_INTERVAL;
    *left -= 1;
    true
}

/// Applies the DPI and report rate saved in the config (if any) and
/// remembers the mouse's settings for the GUI. These settings are lost when
/// the mouse powers off, which is why they are applied on every connect.
///
/// If the mouse does not answer (a wireless mouse asleep at login), the
/// settings stay unknown and the reader thread tries again once the mouse
/// is being used.
pub(crate) fn sync(shared: &Shared, device_index: usize) {
    let (ids, wanted, name) = {
        let state = lock(shared);
        let Some(device) = state.loaded.config.devices.get(device_index) else {
            return;
        };
        (
            device.ids.clone(),
            Wanted {
                dpi: device.dpi,
                polling_hz: device.polling_hz,
            },
            device.name.clone(),
        )
    };
    let result = apply(&ids, &wanted);
    let mut state = lock(shared);
    let tries_left = match state.hardware_retry.get(device_index) {
        Some(Some((_, left))) => *left,
        _ => 0,
    };
    let Some(slot) = state.hardware.get_mut(device_index) else {
        return;
    };
    match result {
        Ok(Some(info)) => {
            log(&format!(
                "\"{name}\": {} DPI, {} Hz",
                info.dpi, info.polling_hz
            ));
            *slot = Some(info);
        }
        Ok(None) => {
            *slot = None;
            if tries_left == RETRIES {
                log(&format!(
                    "\"{name}\": DPI and report rate not readable yet (mouse asleep?); will retry when it is used"
                ));
            }
        }
        Err(error) => {
            *slot = None;
            if tries_left == RETRIES || tries_left == 0 {
                log(&format!(
                    "\"{name}\": cannot read or set DPI and report rate: {error}"
                ));
            }
        }
    }
}

/// Changes the DPI and/or report rate of a mouse now, and saves the new
/// values to `config.toml` so they are applied again on every connect.
pub(crate) fn set(
    shared: &Shared,
    device_index: usize,
    dpi: Option<u32>,
    polling_hz: Option<u32>,
) -> Result<(), String> {
    let ids = {
        let state = lock(shared);
        state
            .loaded
            .config
            .devices
            .get(device_index)
            .ok_or_else(|| format!("there is no device number {device_index}"))?
            .ids
            .clone()
    };
    let info = apply(&ids, &Wanted { dpi, polling_hz })?.ok_or(
        "DPI and report rate are not available for this mouse (not connected, or not supported)",
    )?;

    let mut state = lock(shared);
    if let Some(slot) = state.hardware.get_mut(device_index) {
        *slot = Some(info.clone());
    }
    // Save what the mouse reports it is now using.
    state.save_hardware_settings(
        device_index,
        dpi.map(|_| info.dpi),
        polling_hz.map(|_| info.polling_hz),
    )
}
