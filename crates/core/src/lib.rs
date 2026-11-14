//! Config types, binding resolution and the remapping engine.
//!
//! Pure logic: no I/O, no D-Bus, no evdev. Everything here takes plain values
//! in and gives plain values out, so `cargo test` covers it without hardware.
//!
//! - [`event`]: the event, key and trigger types everything else speaks in.
//! - [`config`]: `config.toml` (settings and known mice).
//! - [`profile`]: profile files, and resolving them against a mouse.
//! - [`engine`]: the state machine that rewrites events.

pub mod config;
pub mod engine;
pub mod error;
pub mod event;
mod keynames;
pub mod profile;

pub use config::{Config, Control, DeviceConfig, DeviceId, Settings};
pub use engine::Engine;
pub use error::ConfigError;
pub use event::{Event, Key, ScrollAxis, Trigger};
pub use profile::{Action, Binding, Profile, ResolvedProfile, resolve};
