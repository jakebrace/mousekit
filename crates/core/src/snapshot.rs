//! The daemon's whole user-visible state in one value, which is what the
//! GUI displays. It travels over D-Bus as TOML text, so both ends use these
//! same types and the D-Bus interface stays small.

use serde::{Deserialize, Serialize};

use crate::config::{DeviceConfig, DeviceId};
use crate::error::ConfigError;
use crate::macros::Macro;
use crate::profile::Profile;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Snapshot {
    pub paused: bool,
    /// Name of the profile the engines are using right now.
    pub active_profile: String,
    /// Name of the profile forced by hand; empty when switching is automatic.
    pub override_profile: String,
    /// "kde", or why focus switching is off.
    pub focus_status: String,
    pub default_profile: Profile,
    /// App-specific profiles.
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub macros: Vec<Macro>,
    #[serde(default)]
    pub devices: Vec<DeviceSnapshot>,
    /// Mice that are plugged in but not in the config yet, for the "add a
    /// mouse" page.
    #[serde(default)]
    pub available_mice: Vec<AvailableMouse>,
    /// Application windows open right now, for the "add from running
    /// windows" picker.
    #[serde(default)]
    pub windows: Vec<WindowSummary>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct DeviceSnapshot {
    pub connected: bool,
    pub config: DeviceConfig,
    /// The mouse's own sensor settings, when it is connected and a hardware
    /// backend can talk to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware: Option<HardwareInfo>,
}

/// Settings that live in the mouse itself rather than in the remapper.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct HardwareInfo {
    /// Current sensor resolution.
    pub dpi: u32,
    /// Every resolution the sensor accepts, ascending.
    pub dpi_values: Vec<u32>,
    /// Current report rate in reports per second.
    pub polling_hz: u32,
    /// Every report rate the mouse accepts, ascending.
    pub polling_values: Vec<u32>,
}

impl HardwareInfo {
    /// The accepted resolution closest to `wanted`.
    pub fn nearest_dpi(&self, wanted: u32) -> Option<u32> {
        self.dpi_values
            .iter()
            .copied()
            .min_by_key(|value| value.abs_diff(wanted))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct AvailableMouse {
    pub id: DeviceId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct WindowSummary {
    pub class: String,
    pub title: String,
    /// Program name, or empty when unknown.
    pub exe: String,
}

impl Snapshot {
    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a snapshot always serializes")
    }

    pub fn from_toml(text: &str) -> Result<Snapshot, ConfigError> {
        Ok(toml::from_str(text)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn a_snapshot_survives_the_trip_over_the_wire() {
        let config = Config::from_toml(include_str!("../../../docs/examples/config.toml")).unwrap();
        let default =
            Profile::from_toml(include_str!("../../../docs/examples/profiles/default.toml"))
                .unwrap();
        let deadlock = Profile::from_toml(include_str!(
            "../../../docs/examples/profiles/deadlock.toml"
        ))
        .unwrap();
        let snapshot = Snapshot {
            paused: false,
            active_profile: "Deadlock".into(),
            override_profile: String::new(),
            focus_status: "kde".into(),
            default_profile: default,
            profiles: vec![deadlock],
            macros: vec![
                Macro::from_toml(
                    "name = \"combo\"\n[[step]]\ntype = \"key_down\"\nkey = \"KEY_B\"\n[[step]]\ntype = \"delay\"\nms = 25\n",
                )
                .unwrap(),
            ],
            devices: config
                .devices
                .into_iter()
                .map(|config| DeviceSnapshot {
                    connected: true,
                    config,
                    hardware: Some(HardwareInfo {
                        dpi: 1600,
                        dpi_values: vec![800, 1600, 3200],
                        polling_hz: 1000,
                        polling_values: vec![125, 250, 500, 1000],
                    }),
                })
                .collect(),
            available_mice: vec![AvailableMouse {
                id: "1234:abcd".parse().unwrap(),
                name: "Another Mouse".into(),
            }],
            windows: vec![WindowSummary {
                class: "steam_app_1422450".into(),
                title: "Deadlock \"quoted\" title\twith tab".into(),
                exe: String::new(),
            }],
        };
        assert_eq!(Snapshot::from_toml(&snapshot.to_toml()).unwrap(), snapshot);

        let hardware = snapshot.devices[0].hardware.as_ref().unwrap();
        assert_eq!(hardware.nearest_dpi(1500), Some(1600));
        assert_eq!(hardware.nearest_dpi(99_999), Some(3200));

        let empty = Snapshot {
            profiles: Vec::new(),
            macros: Vec::new(),
            devices: Vec::new(),
            available_mice: Vec::new(),
            windows: Vec::new(),
            ..snapshot
        };
        assert_eq!(Snapshot::from_toml(&empty.to_toml()).unwrap(), empty);
    }
}
