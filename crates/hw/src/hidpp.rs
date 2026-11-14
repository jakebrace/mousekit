//! Talking HID++ 2.0 to a Logitech mouse through its hidraw node.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mouse_profiles_core::{DeviceId, HardwareInfo};
use rustix::event::{PollFd, PollFlags, Timespec, poll};

use crate::Hardware;
use crate::protocol::{self, Reply};

/// How long to wait for the mouse to answer one request. A sleeping
/// wireless mouse does not answer at all.
const REPLY_TIMEOUT: Duration = Duration::from_millis(700);

pub(crate) struct Hidpp {
    node: File,
}

/// The hidraw nodes belonging to a device ID, found through sysfs so nothing
/// depends on hidraw numbering.
fn nodes_for(id: DeviceId) -> Vec<PathBuf> {
    let wanted = format!("{:08X}:{:08X}", id.vendor, id.product);
    let Ok(entries) = fs::read_dir("/sys/class/hidraw") else {
        return Vec::new();
    };
    let mut nodes: Vec<PathBuf> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let uevent = fs::read_to_string(entry.path().join("device/uevent")).ok()?;
            // A line like "HID_ID=0003:0000046D:0000409F" (bus:vendor:product).
            let hid_id = uevent
                .lines()
                .find_map(|line| line.strip_prefix("HID_ID="))?;
            hid_id
                .to_uppercase()
                .ends_with(&wanted)
                .then(|| PathBuf::from("/dev").join(entry.file_name()))
        })
        .collect();
    nodes.sort();
    nodes
}

/// Opens the first node of the device that answers HID++. A mouse on a
/// cable has several hidraw nodes and only one of them speaks HID++.
pub(crate) fn open(id: DeviceId) -> Result<Option<Hidpp>, String> {
    let mut denied = None;
    for path in nodes_for(id) {
        let node = match OpenOptions::new().read(true).write(true).open(&path) {
            Ok(node) => node,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                denied = Some(path);
                continue;
            }
            Err(_) => continue,
        };
        let mut device = Hidpp { node };
        // Asking the root feature about itself is a harmless way to see
        // whether anything on this node answers.
        if device.feature_index(0x0000).is_ok() {
            return Ok(Some(device));
        }
    }
    match denied {
        Some(path) => Err(format!(
            "no permission to open {} (the udev rule needs its hidraw line)",
            path.display()
        )),
        None => Ok(None),
    }
}

impl Hidpp {
    /// Sends one request and waits for its answer.
    fn call(&mut self, feature_index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>, String> {
        let request = protocol::request(feature_index, function, params);
        self.node
            .write_all(&request)
            .map_err(|error| format!("cannot write to the mouse: {error}"))?;

        let deadline = Instant::now() + REPLY_TIMEOUT;
        let mut buffer = [0u8; 64];
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("the mouse did not answer (is it asleep or switched off?)".into());
            }
            // The node also carries ordinary mouse reports, so wait with a
            // timeout and skip whatever is not our reply.
            let timeout = Timespec {
                tv_sec: remaining.as_secs() as i64,
                tv_nsec: remaining.subsec_nanos().into(),
            };
            let mut fds = [PollFd::new(&self.node, PollFlags::IN)];
            let ready = poll(&mut fds, Some(&timeout))
                .map_err(|error| format!("cannot wait for the mouse: {error}"))?;
            if ready == 0 {
                continue;
            }
            let length = self
                .node
                .read(&mut buffer)
                .map_err(|error| format!("cannot read from the mouse: {error}"))?;
            match protocol::classify(&buffer[..length], feature_index, function) {
                Reply::Answer(payload) => return Ok(payload.to_vec()),
                Reply::Error(code) => {
                    return Err(format!(
                        "the mouse refused the request: {}",
                        protocol::error_text(code)
                    ));
                }
                Reply::Unrelated => {}
            }
        }
    }

    /// Where the mouse keeps a feature, or an error if it does not have it.
    fn feature_index(&mut self, feature: u16) -> Result<u8, String> {
        let [high, low] = feature.to_be_bytes();
        let reply = self.call(
            protocol::ROOT_INDEX,
            protocol::ROOT_GET_FEATURE,
            &[high, low],
        )?;
        match reply.first() {
            // Index 0 is the root itself; for any other feature it means
            // "not present".
            Some(0) if feature != 0x0000 => {
                Err(format!("the mouse does not have feature {feature:#06x}"))
            }
            Some(index) => Ok(*index),
            None => Err("the mouse sent an empty reply".into()),
        }
    }

    /// Puts the mouse in host mode, where it obeys DPI and rate commands
    /// instead of its stored onboard profile. A mouse without onboard
    /// profiles is always in that state.
    fn ensure_host_mode(&mut self) -> Result<(), String> {
        let Ok(onboard) = self.feature_index(protocol::FEATURE_ONBOARD_PROFILES) else {
            return Ok(());
        };
        let mode = self.call(onboard, protocol::ONBOARD_GET_MODE, &[])?;
        if mode.first() != Some(&protocol::MODE_HOST) {
            self.call(onboard, protocol::ONBOARD_SET_MODE, &[protocol::MODE_HOST])?;
        }
        Ok(())
    }
}

impl Hardware for Hidpp {
    fn info(&mut self) -> Result<HardwareInfo, String> {
        let dpi_feature = self.feature_index(protocol::FEATURE_ADJUSTABLE_DPI)?;
        // All requests are for sensor 0; mice with two sensors are rare.
        let current = self.call(dpi_feature, protocol::DPI_GET, &[0])?;
        if current.len() < 5 {
            return Err("the mouse sent a short DPI reply".into());
        }
        let mut dpi = u32::from(u16::from_be_bytes([current[1], current[2]]));
        if dpi == 0 {
            // Zero means "the default", which follows in the next two bytes.
            dpi = u32::from(u16::from_be_bytes([current[3], current[4]]));
        }

        // The list may span several replies; each starts with the sensor
        // index, which is not part of the list.
        let mut list_bytes = Vec::new();
        let mut dpi_values = None;
        for part in 0..16u8 {
            let reply = self.call(dpi_feature, protocol::DPI_GET_LIST, &[0, 0, part])?;
            list_bytes.extend_from_slice(reply.get(1..).unwrap_or(&[]));
            dpi_values = protocol::parse_dpi_list(&list_bytes);
            if dpi_values.is_some() {
                break;
            }
        }
        let dpi_values = dpi_values.ok_or("the mouse's DPI list never ended")?;

        let (polling_hz, polling_values) = match self.feature_index(protocol::FEATURE_REPORT_RATE) {
            Ok(rate_feature) => {
                let list = self.call(rate_feature, protocol::RATE_GET_LIST, &[])?;
                let current = self.call(rate_feature, protocol::RATE_GET, &[])?;
                let interval = u32::from(current.first().copied().unwrap_or(0)).max(1);
                (
                    1000 / interval,
                    protocol::parse_rate_list(list.first().copied().unwrap_or(0)),
                )
            }
            // No adjustable rate: report nothing to choose from.
            Err(_) => (0, Vec::new()),
        };

        Ok(HardwareInfo {
            dpi,
            dpi_values,
            polling_hz,
            polling_values,
        })
    }

    fn set_dpi(&mut self, dpi: u32) -> Result<(), String> {
        let value = u16::try_from(dpi).map_err(|_| format!("{dpi} DPI is out of range"))?;
        let feature = self.feature_index(protocol::FEATURE_ADJUSTABLE_DPI)?;
        self.ensure_host_mode()?;
        let [high, low] = value.to_be_bytes();
        self.call(feature, protocol::DPI_SET, &[0, high, low])?;
        Ok(())
    }

    fn set_polling_hz(&mut self, hz: u32) -> Result<(), String> {
        let interval = protocol::interval_ms(hz)
            .ok_or_else(|| format!("{hz} Hz is not a rate the protocol can express"))?;
        let feature = self.feature_index(protocol::FEATURE_REPORT_RATE)?;
        self.ensure_host_mode()?;
        self.call(feature, protocol::RATE_SET, &[interval])?;
        Ok(())
    }
}
