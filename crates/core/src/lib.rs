//! Config types, binding resolution and the remapping engine.
//!
//! Pure logic: no I/O, no D-Bus, no evdev. Everything here takes plain values
//! in and gives plain values out, so `cargo test` covers it without hardware.
//!
//! - [`event`]: the event, key and trigger types everything else speaks in.
//! - [`config`]: `config.toml` (settings and known mice).
//! - [`profile`]: profile files, and resolving them against a mouse.
//! - [`engine`]: the state machine that rewrites events.
//! - [`focus`]: which profile a focused window calls for.

pub mod config;

/// Where the daemon lives on the session bus. Plain strings shared by the
/// daemon and its clients; nothing in this crate talks to D-Bus.
/// Placeholders until the project's app ID is settled (see docs/PLAN.md).
pub mod dbus {
    pub const BUS_NAME: &str = "io.github.mouseprofiles.Daemon";
    pub const OBJECT_PATH: &str = "/io/github/mouseprofiles/Daemon";
    pub const INTERFACE: &str = "io.github.mouseprofiles.Daemon1";
    /// Environment variable that selects an isolated test instance; its
    /// value is appended to the bus name. Never set in normal use.
    pub const INSTANCE_VARIABLE: &str = "MOUSE_PROFILES_INSTANCE";
}
pub mod engine;
pub mod error;
pub mod event;
pub mod focus;
mod keynames;
pub mod profile;
pub mod snapshot;

pub use config::{Config, Control, DeviceConfig, DeviceId, Settings};
pub use engine::Engine;
pub use error::ConfigError;
pub use event::{Event, Key, ScrollAxis, Trigger};
pub use focus::{Decision, Window, WindowKind, check_profiles, decide};
pub use profile::{Action, Binding, Profile, ResolvedProfile, resolve};
pub use snapshot::{DeviceSnapshot, Snapshot, WindowSummary};
