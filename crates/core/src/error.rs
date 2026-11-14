//! The one error type for anything wrong with a config or profile file.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The text is not valid TOML, or has the wrong shape (unknown field,
    /// wrong type, unknown key name). The message includes line and column.
    Parse(String),
    /// The TOML is well-formed but breaks one of our rules.
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Parse(message) | ConfigError::Invalid(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<toml::de::Error> for ConfigError {
    fn from(error: toml::de::Error) -> Self {
        ConfigError::Parse(error.to_string())
    }
}
