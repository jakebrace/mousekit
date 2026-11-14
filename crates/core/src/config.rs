//! `config.toml`: global settings and one block per known mouse.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::ConfigError;
use crate::event::Trigger;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    /// Written as repeated `[[device]]` blocks.
    #[serde(default, rename = "device")]
    pub devices: Vec<DeviceConfig>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct Settings {
    pub autostart: bool,
    pub start_in_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            autostart: true,
            start_in_tray: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfig {
    /// Every vendor:product ID this mouse shows up as. A wireless mouse has
    /// one ID over its receiver and another over its cable.
    pub ids: Vec<DeviceId>,
    pub name: String,
    /// Stored for Milestone 6; nothing applies these yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpi: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polling_hz: Option<u32>,
    /// Written as repeated `[[device.control]]` blocks.
    #[serde(default, rename = "control")]
    pub controls: Vec<Control>,
}

impl DeviceConfig {
    /// This one device block as TOML, the form the daemon's `SaveDevice`
    /// call takes.
    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a device always serializes")
    }

    pub fn control(&self, label: &str) -> Option<&Control> {
        self.controls.iter().find(|control| control.label == label)
    }
}

/// A named button or scroll direction on one mouse. Profiles refer to the
/// label, so they survive the underlying event changing.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub label: String,
    pub event: Trigger,
}

/// A USB vendor:product pair, written as "046d:409f".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId {
    pub vendor: u16,
    pub product: u16,
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04x}:{:04x}", self.vendor, self.product)
    }
}

impl FromStr for DeviceId {
    type Err = String;

    fn from_str(text: &str) -> Result<DeviceId, String> {
        let parsed = text.split_once(':').and_then(|(vendor, product)| {
            Some(DeviceId {
                vendor: u16::from_str_radix(vendor, 16).ok()?,
                product: u16::from_str_radix(product, 16).ok()?,
            })
        });
        parsed.ok_or_else(|| {
            format!("device ID \"{text}\" must look like \"046d:409f\" (vendor:product in hex)")
        })
    }
}

impl Serialize for DeviceId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for DeviceId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<DeviceId, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl Config {
    /// Parses and validates the text of a `config.toml`.
    pub fn from_toml(text: &str) -> Result<Config, ConfigError> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    /// The text of a `config.toml` holding this config. Comments in a
    /// hand-written file are not kept.
    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a config always serializes")
    }

    /// The device block that lists this ID, if any.
    pub fn device(&self, id: DeviceId) -> Option<&DeviceConfig> {
        self.devices.iter().find(|device| device.ids.contains(&id))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen_ids = HashSet::new();
        for device in &self.devices {
            if device.ids.is_empty() {
                return Err(invalid(format!(
                    "device \"{}\" has an empty ids list",
                    device.name
                )));
            }
            for id in &device.ids {
                if !seen_ids.insert(*id) {
                    return Err(invalid(format!("device ID {id} is listed more than once")));
                }
            }

            let mut labels = HashSet::new();
            let mut events = HashSet::new();
            for control in &device.controls {
                if control.label.is_empty() {
                    return Err(invalid(format!(
                        "device \"{}\" has a control with an empty label",
                        device.name
                    )));
                }
                if !labels.insert(control.label.as_str()) {
                    return Err(invalid(format!(
                        "device \"{}\" has two controls labelled \"{}\"",
                        device.name, control.label
                    )));
                }
                if !events.insert(control.event) {
                    return Err(invalid(format!(
                        "device \"{}\" has two controls for the event {}",
                        device.name, control.event
                    )));
                }
            }
        }
        Ok(())
    }
}

fn invalid(message: String) -> ConfigError {
    ConfigError::Invalid(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Key, ScrollAxis};

    const EXAMPLE: &str = r#"
        [settings]
        autostart = false

        [[device]]
        ids = ["046d:409f", "046d:c098"]
        name = "Logitech G502 X LS"
        dpi = 1600
        polling_hz = 1000

          [[device.control]]
          label = "M4"
          event = "BTN_SIDE"

          [[device.control]]
          label = "Scroll Up"
          event = "REL_WHEEL:+"
    "#;

    #[test]
    fn parses_the_example() {
        let config = Config::from_toml(EXAMPLE).unwrap();
        assert!(!config.settings.autostart);
        assert!(
            config.settings.start_in_tray,
            "unset settings keep their default"
        );

        let wired = DeviceId {
            vendor: 0x046d,
            product: 0xc098,
        };
        let device = config.device(wired).expect("found by its second ID");
        assert_eq!(device.dpi, Some(1600));
        assert_eq!(
            device.control("M4").unwrap().event,
            Trigger::Key(Key::from_name("BTN_SIDE").unwrap())
        );
        assert_eq!(
            device.control("Scroll Up").unwrap().event,
            Trigger::Scroll {
                axis: ScrollAxis::Vertical,
                positive: true
            }
        );
    }

    #[test]
    fn a_config_survives_being_written_and_read_back() {
        let config = Config::from_toml(EXAMPLE).unwrap();
        assert_eq!(Config::from_toml(&config.to_toml()).unwrap(), config);
        let empty = Config::from_toml("").unwrap();
        assert_eq!(Config::from_toml(&empty.to_toml()).unwrap(), empty);
    }

    #[test]
    fn an_empty_file_is_a_valid_config() {
        let config = Config::from_toml("").unwrap();
        assert!(config.devices.is_empty());
        assert_eq!(config.settings, Settings::default());
    }

    #[test]
    fn rejects_unknown_fields_and_bad_names() {
        assert!(matches!(
            Config::from_toml("[settings]\nautostrat = true"),
            Err(ConfigError::Parse(_))
        ));
        let bad_key = r#"
            [[device]]
            ids = ["046d:409f"]
            name = "x"
            [[device.control]]
            label = "M4"
            event = "BTN_SIDEWAYS"
        "#;
        let error = Config::from_toml(bad_key).unwrap_err().to_string();
        assert!(error.contains("BTN_SIDEWAYS"), "{error}");
    }

    #[test]
    fn rejects_duplicate_labels_events_and_ids() {
        let duplicate_label = r#"
            [[device]]
            ids = ["046d:409f"]
            name = "x"
            [[device.control]]
            label = "M4"
            event = "BTN_SIDE"
            [[device.control]]
            label = "M4"
            event = "BTN_EXTRA"
        "#;
        assert!(matches!(
            Config::from_toml(duplicate_label),
            Err(ConfigError::Invalid(_))
        ));

        let duplicate_event = duplicate_label.replace(
            "label = \"M4\"\n            event = \"BTN_EXTRA\"",
            "label = \"M5\"\n            event = \"BTN_SIDE\"",
        );
        assert!(matches!(
            Config::from_toml(&duplicate_event),
            Err(ConfigError::Invalid(_))
        ));

        let duplicate_id = r#"
            [[device]]
            ids = ["046d:409f"]
            name = "a"
            [[device]]
            ids = ["046d:409f"]
            name = "b"
        "#;
        assert!(matches!(
            Config::from_toml(duplicate_id),
            Err(ConfigError::Invalid(_))
        ));
    }
}
