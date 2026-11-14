//! The vocabulary of the engine: raw events, key codes, and triggers.
//!
//! These mirror the kernel's input-event layout so the `input` crate can
//! convert to and from evdev with no lookups, but nothing here touches a
//! device.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::keynames::{ALIASES, CANONICAL};

// Event types (the kernel's EV_* numbers).
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_MSC: u16 = 0x04;

/// Code of the EV_SYN event that ends a frame.
pub const SYN_REPORT: u16 = 0;

// Relative axis codes for the two scroll wheels.
pub const REL_HWHEEL: u16 = 0x06;
pub const REL_WHEEL: u16 = 0x08;
pub const REL_WHEEL_HI_RES: u16 = 0x0b;
pub const REL_HWHEEL_HI_RES: u16 = 0x0c;

/// One input event, exactly as the kernel reports it minus the timestamp.
/// For keys and buttons, `value` is 1 for press, 0 for release, 2 for repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

impl Event {
    pub const SYN: Event = Event {
        kind: EV_SYN,
        code: SYN_REPORT,
        value: 0,
    };

    pub fn key(key: Key, value: i32) -> Event {
        Event {
            kind: EV_KEY,
            code: key.0,
            value,
        }
    }

    pub fn rel(code: u16, value: i32) -> Event {
        Event {
            kind: EV_REL,
            code,
            value,
        }
    }

    pub fn is_syn_report(&self) -> bool {
        self.kind == EV_SYN && self.code == SYN_REPORT
    }
}

/// A keyboard key or mouse button code (the kernel's KEY_* and BTN_* share
/// one number space).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(pub u16);

impl Key {
    pub const BTN_LEFT: Key = Key(0x110);

    /// One past the highest key code the kernel defines (KEY_MAX is 0x2ff).
    const CODE_LIMIT: u16 = 0x300;

    /// Looks up a kernel name such as "KEY_F13" or "BTN_SIDE".
    ///
    /// Some codes have no kernel name. Gaming mice use several: their extra
    /// buttons continue past BTN_TASK (0x117) into 0x118, 0x119 and so on.
    /// Those are written by number, as "BTN_0x119" or "KEY_0x119", which is
    /// also how they are printed.
    pub fn from_name(name: &str) -> Option<Key> {
        let named = CANONICAL
            .iter()
            .chain(ALIASES)
            .find(|(_, candidate)| *candidate == name)
            .map(|(code, _)| Key(*code));
        named.or_else(|| {
            let hex = name
                .strip_prefix("BTN_0x")
                .or_else(|| name.strip_prefix("KEY_0x"))?;
            let code = u16::from_str_radix(hex, 16).ok()?;
            (code < Key::CODE_LIMIT).then_some(Key(code))
        })
    }

    /// Every key and button code the kernel has a name for.
    pub fn all() -> impl Iterator<Item = Key> {
        CANONICAL.iter().map(|(code, _)| Key(*code))
    }

    /// The kernel name for this code, if it has one.
    pub fn name(self) -> Option<&'static str> {
        CANONICAL
            .binary_search_by_key(&self.0, |(code, _)| *code)
            .ok()
            .map(|index| CANONICAL[index].1)
    }

    /// True for BTN_* codes (mouse, joystick and similar buttons), false for
    /// keyboard keys.
    pub fn is_button(self) -> bool {
        matches!(self.0, 0x100..0x160 | 0x220..0x230 | 0x2c0..0x2e8)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None if self.is_button() => write!(f, "BTN_{:#x}", self.0),
            None => write!(f, "KEY_{:#x}", self.0),
        }
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl FromStr for Key {
    type Err = String;

    fn from_str(text: &str) -> Result<Key, String> {
        Key::from_name(text).ok_or_else(|| format!("unknown key or button name \"{text}\""))
    }
}

impl Serialize for Key {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Key, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScrollAxis {
    /// The normal wheel (REL_WHEEL).
    Vertical,
    /// Wheel tilt or a second wheel (REL_HWHEEL).
    Horizontal,
}

/// Something on the mouse a binding can attach to: a button, or one
/// direction of a scroll wheel.
///
/// Written in config as "BTN_SIDE", "REL_WHEEL:+" or "REL_HWHEEL:-".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    Key(Key),
    Scroll { axis: ScrollAxis, positive: bool },
}

impl Trigger {
    /// The trigger an event fires, if any: a button or key going down, or a
    /// whole tick of a scroll wheel. Releases, repeats, pointer motion and
    /// the high-resolution scroll stream fire nothing.
    pub fn from_event(event: &Event) -> Option<Trigger> {
        match (event.kind, event.code) {
            (EV_KEY, code) if event.value == 1 => Some(Trigger::Key(Key(code))),
            (EV_REL, REL_WHEEL | REL_HWHEEL) if event.value != 0 => Some(Trigger::Scroll {
                axis: if event.code == REL_WHEEL {
                    ScrollAxis::Vertical
                } else {
                    ScrollAxis::Horizontal
                },
                positive: event.value > 0,
            }),
            _ => None,
        }
    }
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trigger::Key(key) => write!(f, "{key}"),
            Trigger::Scroll { axis, positive } => {
                let axis = match axis {
                    ScrollAxis::Vertical => "REL_WHEEL",
                    ScrollAxis::Horizontal => "REL_HWHEEL",
                };
                let sign = if *positive { '+' } else { '-' };
                write!(f, "{axis}:{sign}")
            }
        }
    }
}

impl FromStr for Trigger {
    type Err = String;

    fn from_str(text: &str) -> Result<Trigger, String> {
        if let Some((axis, sign)) = text.split_once(':') {
            let axis = match axis {
                "REL_WHEEL" => ScrollAxis::Vertical,
                "REL_HWHEEL" => ScrollAxis::Horizontal,
                _ => return Err(format!("unknown scroll axis in \"{text}\"")),
            };
            let positive = match sign {
                "+" => true,
                "-" => false,
                _ => return Err(format!("scroll direction in \"{text}\" must be + or -")),
            };
            Ok(Trigger::Scroll { axis, positive })
        } else {
            text.parse().map(Trigger::Key)
        }
    }
}

impl Serialize for Trigger {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Trigger {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Trigger, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_round_trip() {
        assert_eq!(Key::from_name("BTN_SIDE"), Some(Key(0x113)));
        assert_eq!(Key(0x113).name(), Some("BTN_SIDE"));
        assert_eq!(Key::from_name("KEY_F13"), Some(Key(183)));
        assert_eq!(Key::from_name("KEY_NOPE"), None);
    }

    /// Found on a G502 X: a thumb button reports code 0x119, which the
    /// kernel has no name for.
    #[test]
    fn codes_without_a_kernel_name_round_trip_by_number() {
        let unnamed = Key(0x119);
        assert_eq!(unnamed.name(), None);
        assert_eq!(unnamed.to_string(), "BTN_0x119");
        assert_eq!(Key::from_name("BTN_0x119"), Some(unnamed));
        assert_eq!(Key::from_name("KEY_0x119"), Some(unnamed));
        assert_eq!(
            Trigger::Key(unnamed).to_string().parse(),
            Ok(Trigger::Key(unnamed))
        );
        assert_eq!(Key(0x2f0).to_string(), "KEY_0x2f0");
        assert_eq!(Key::from_name("KEY_0x2f0"), Some(Key(0x2f0)));
        // Out of range or malformed numbers are still rejected.
        assert_eq!(Key::from_name("KEY_0x300"), None);
        assert_eq!(Key::from_name("BTN_0xzz"), None);
    }

    /// Every code must print as something that parses back to itself, or a
    /// captured button could not be saved.
    #[test]
    fn every_possible_code_round_trips() {
        for code in 0..Key::CODE_LIMIT {
            let key = Key(code);
            assert_eq!(
                Key::from_name(&key.to_string()),
                Some(key),
                "code {code:#x}"
            );
        }
    }

    #[test]
    fn alias_parses_but_canonical_name_prints() {
        assert_eq!(Key::from_name("BTN_MOUSE"), Some(Key::BTN_LEFT));
        assert_eq!(Key::BTN_LEFT.to_string(), "BTN_LEFT");
    }

    #[test]
    fn canonical_table_is_sorted_for_binary_search() {
        assert!(CANONICAL.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn triggers_parse_and_print() {
        let up: Trigger = "REL_WHEEL:+".parse().unwrap();
        assert_eq!(
            up,
            Trigger::Scroll {
                axis: ScrollAxis::Vertical,
                positive: true
            }
        );
        assert_eq!(up.to_string(), "REL_WHEEL:+");
        assert_eq!("BTN_SIDE".parse(), Ok(Trigger::Key(Key(0x113))));
        assert!("REL_X:+".parse::<Trigger>().is_err());
        assert!("REL_WHEEL:up".parse::<Trigger>().is_err());
    }

    #[test]
    fn only_presses_and_whole_scroll_ticks_are_triggers() {
        let side = Key::from_name("BTN_SIDE").unwrap();
        assert_eq!(
            Trigger::from_event(&Event::key(side, 1)),
            Some(Trigger::Key(side))
        );
        assert_eq!(Trigger::from_event(&Event::key(side, 0)), None);
        assert_eq!(Trigger::from_event(&Event::key(side, 2)), None);
        assert_eq!(
            Trigger::from_event(&Event::rel(REL_WHEEL, -1)),
            Some(Trigger::Scroll {
                axis: ScrollAxis::Vertical,
                positive: false
            })
        );
        assert_eq!(
            Trigger::from_event(&Event::rel(REL_WHEEL_HI_RES, 120)),
            None
        );
        assert_eq!(Trigger::from_event(&Event::rel(0, 5)), None);
    }

    #[test]
    fn buttons_and_keys_are_told_apart() {
        assert!(Key::BTN_LEFT.is_button());
        assert!(!Key::from_name("KEY_F13").unwrap().is_button());
    }
}
