//! Macros: a stored list of key presses, releases and pauses, kept one per
//! file in `macros/` and referred to by name from any profile.

use serde::{Deserialize, Serialize};

use crate::error::ConfigError;
use crate::event::Key;

/// The longest single pause a macro may contain.
pub const MAX_DELAY_MS: u32 = 60_000;
/// The most steps one macro may have.
pub const MAX_STEPS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Macro {
    pub name: String,
    /// Written as repeated `[[step]]` blocks.
    #[serde(default, rename = "step")]
    pub steps: Vec<Step>,
}

/// One step, e.g. `{ type = "key_down", key = "KEY_B" }` or
/// `{ type = "delay", ms = 50 }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    KeyDown {
        key: Key,
    },
    KeyUp {
        key: Key,
    },
    /// Wait this many milliseconds before the next step.
    Delay {
        ms: u32,
    },
}

impl Macro {
    pub fn from_toml(text: &str) -> Result<Macro, ConfigError> {
        let parsed: Macro = toml::from_str(text)?;
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a macro always serializes")
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.name.trim().is_empty() {
            return Err(ConfigError::Invalid("a macro needs a name".into()));
        }
        if self.steps.len() > MAX_STEPS {
            return Err(ConfigError::Invalid(format!(
                "macro \"{}\" has {} steps; the limit is {MAX_STEPS}",
                self.name,
                self.steps.len()
            )));
        }
        for step in &self.steps {
            if let Step::Delay { ms } = step
                && *ms > MAX_DELAY_MS
            {
                return Err(ConfigError::Invalid(format!(
                    "macro \"{}\" has a {ms} ms delay; the limit is {MAX_DELAY_MS} ms",
                    self.name
                )));
            }
        }
        Ok(())
    }

    /// How long one pass takes: the sum of its delays, in milliseconds.
    pub fn duration_ms(&self) -> u64 {
        self.steps
            .iter()
            .map(|step| match step {
                Step::Delay { ms } => u64::from(*ms),
                _ => 0,
            })
            .sum()
    }
}

/// No two macros may share a name.
pub fn check_macros(macros: &[Macro]) -> Result<(), ConfigError> {
    for (index, one) in macros.iter().enumerate() {
        if macros[..index]
            .iter()
            .any(|other| other.name.eq_ignore_ascii_case(&one.name))
        {
            return Err(ConfigError::Invalid(format!(
                "two macros are named \"{}\"",
                one.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
        name = "buy-combo"

        [[step]]
        type = "key_down"
        key = "KEY_B"

        [[step]]
        type = "delay"
        ms = 40

        [[step]]
        type = "key_up"
        key = "KEY_B"
    "#;

    #[test]
    fn parses_and_round_trips() {
        let parsed = Macro::from_toml(EXAMPLE).unwrap();
        let b = Key::from_name("KEY_B").unwrap();
        assert_eq!(
            parsed.steps,
            [
                Step::KeyDown { key: b },
                Step::Delay { ms: 40 },
                Step::KeyUp { key: b }
            ]
        );
        assert_eq!(parsed.duration_ms(), 40);
        assert_eq!(Macro::from_toml(&parsed.to_toml()).unwrap(), parsed);

        let empty = Macro::from_toml("name = \"empty\"").unwrap();
        assert_eq!(Macro::from_toml(&empty.to_toml()).unwrap(), empty);
    }

    #[test]
    fn rejects_bad_macros() {
        assert!(Macro::from_toml("name = \" \"").is_err());
        let long_delay = "name = \"x\"\n[[step]]\ntype = \"delay\"\nms = 60001\n";
        assert!(matches!(
            Macro::from_toml(long_delay),
            Err(ConfigError::Invalid(_))
        ));
        let unknown_step = "name = \"x\"\n[[step]]\ntype = \"click\"\n";
        assert!(matches!(
            Macro::from_toml(unknown_step),
            Err(ConfigError::Parse(_))
        ));

        let a = Macro::from_toml("name = \"Combo\"").unwrap();
        let b = Macro::from_toml("name = \"combo\"").unwrap();
        assert!(check_macros(&[a.clone(), b]).is_err());
        assert!(check_macros(&[a]).is_ok());
    }
}
