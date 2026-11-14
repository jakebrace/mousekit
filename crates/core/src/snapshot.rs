//! The daemon's whole user-visible state in one value, which is what the
//! GUI displays. It travels over D-Bus as TOML text, so both ends use these
//! same types and the D-Bus interface stays small.

use serde::{Deserialize, Serialize};

use crate::config::DeviceConfig;
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
    /// Application windows open right now, for the "add from running
    /// windows" picker.
    #[serde(default)]
    pub windows: Vec<WindowSummary>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct DeviceSnapshot {
    pub connected: bool,
    pub config: DeviceConfig,
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
                })
                .collect(),
            windows: vec![WindowSummary {
                class: "steam_app_1422450".into(),
                title: "Deadlock \"quoted\" title\twith tab".into(),
                exe: String::new(),
            }],
        };
        assert_eq!(Snapshot::from_toml(&snapshot.to_toml()).unwrap(), snapshot);

        let empty = Snapshot {
            profiles: Vec::new(),
            macros: Vec::new(),
            devices: Vec::new(),
            windows: Vec::new(),
            ..snapshot
        };
        assert_eq!(Snapshot::from_toml(&empty.to_toml()).unwrap(), empty);
    }
}
