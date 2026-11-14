//! The daemon's D-Bus interface on the session bus, and the client side the
//! CLI uses to call it. The GUI will use the same interface.
//!
//! Owning the bus name is also the "is a daemon already running" check.

use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::interface;

use crate::actions::{self, CAPTURE_TIMEOUT};
use crate::{Shared, instance, lock};

use mouse_profiles_core::dbus::{BUS_NAME, INTERFACE, OBJECT_PATH};

/// The bus name in use: the normal one, or a separate one for a test
/// instance so it never collides with a daemon that is really in use.
fn bus_name() -> String {
    match instance() {
        Some(name) => format!("{BUS_NAME}.{name}"),
        None => BUS_NAME.to_string(),
    }
}

/// Connects to the session bus and takes the daemon's name. Fails if
/// another daemon already has it.
pub(crate) fn claim_name() -> Result<Connection, String> {
    let connection = Connection::session()
        .map_err(|error| format!("cannot connect to the session bus: {error}"))?;
    // DoNotQueue alone: take the name only if nobody has it, never replace
    // the current owner and never wait in line behind it. (The connection
    // builder's own `name()` would let a second daemon take the name over.)
    let already_running = || "another mouse-profiles daemon is already running".to_string();
    match connection.request_name_with_flags(bus_name(), RequestNameFlags::DoNotQueue.into()) {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => Ok(connection),
        Ok(_) | Err(zbus::Error::NameTaken) => Err(already_running()),
        Err(error) => Err(format!("cannot take the daemon's bus name: {error}")),
    }
}

/// Starts answering calls on the connection.
pub(crate) fn serve(connection: &Connection, shared: Shared) -> Result<(), String> {
    connection
        .object_server()
        .at(OBJECT_PATH, Api { shared })
        .map(|_| ())
        .map_err(|error| format!("cannot publish the D-Bus interface: {error}"))
}

struct Api {
    shared: Shared,
}

/// zbus turns each method name into CamelCase on the bus: `set_override`
/// is called as `SetOverride`.
#[interface(name = "io.github.mouseprofiles.Daemon1")]
impl Api {
    /// A human-readable summary of the daemon's state.
    fn status(&self) -> String {
        lock(&self.shared).status_text()
    }

    /// Each of these returns whether remapping is paused afterwards.
    fn pause(&self) -> bool {
        self.set_paused(|_| true)
    }

    fn resume(&self) -> bool {
        self.set_paused(|_| false)
    }

    fn toggle(&self) -> bool {
        self.set_paused(|paused| !paused)
    }

    /// Re-reads the config files. Returns a summary including warnings.
    fn reload(&self) -> fdo::Result<String> {
        lock(&self.shared).reload().map_err(|error| {
            fdo::Error::Failed(format!("reload failed, keeping the old config:\n{error}"))
        })
    }

    /// Forces a profile by name; "auto" returns to automatic switching.
    fn set_override(&self, profile: &str) -> fdo::Result<String> {
        let mut state = lock(&self.shared);
        state.set_override(profile).map_err(fdo::Error::Failed)?;
        Ok(format!(
            "profile: {}",
            state.loaded.profile_name(state.active_profile)
        ))
    }

    /// Starts a capture session: until it ends, remapping is suspended and
    /// every button pressed on any grabbed mouse is recorded. Left click
    /// keeps working and is not recorded. Collect results with
    /// `PollCapture`; end it with `CancelCapture`, or leave it and it ends
    /// after 30 seconds without a press.
    fn start_capture(&self) {
        lock(&self.shared).start_capture();
    }

    /// Returns whether the session is still open, and the event names
    /// captured since the last poll (each button is reported once per
    /// session).
    fn poll_capture(&self) -> (bool, Vec<String>) {
        let (open, captured) = lock(&self.shared).poll_capture();
        (
            open,
            captured.iter().map(|trigger| trigger.to_string()).collect(),
        )
    }

    fn cancel_capture(&self) {
        lock(&self.shared).capture = None;
    }

    fn quit(&self) {
        actions::quit(&self.shared);
    }

    // --- For the settings window. Structured values travel as TOML text,
    // --- using the same types as the config files.

    /// The daemon's whole state: a `Snapshot` as TOML.
    fn get_snapshot(&self) -> String {
        lock(&self.shared).snapshot().to_toml()
    }

    /// Creates or replaces a profile. `original` is its current name, or
    /// empty for a new profile; `profile` is the profile file's text.
    fn save_profile(&self, original: &str, profile: &str) -> fdo::Result<()> {
        lock(&self.shared)
            .save_profile(original, profile)
            .map_err(fdo::Error::Failed)
    }

    fn delete_profile(&self, name: &str) -> fdo::Result<()> {
        lock(&self.shared)
            .delete_profile(name)
            .map_err(fdo::Error::Failed)
    }

    /// Creates or replaces a macro. `original` is its current name, or
    /// empty for a new one; `text` is the macro file's text.
    fn save_macro(&self, original: &str, text: &str) -> fdo::Result<()> {
        lock(&self.shared)
            .save_macro(original, text)
            .map_err(fdo::Error::Failed)
    }

    fn delete_macro(&self, name: &str) -> fdo::Result<()> {
        lock(&self.shared)
            .delete_macro(name)
            .map_err(fdo::Error::Failed)
    }

    /// Replaces one device block; `device` is the block's TOML text.
    fn save_device(&self, index: u32, device: &str) -> fdo::Result<()> {
        lock(&self.shared)
            .save_device(index as usize, device)
            .map_err(fdo::Error::Failed)
    }

    /// Sets a mouse's sensor resolution now and saves it for next time.
    /// Blocks while the mouse is asked, usually well under a second.
    fn set_dpi(&self, index: u32, dpi: u32) -> fdo::Result<()> {
        crate::hardware::set(&self.shared, index as usize, Some(dpi), None)
            .map_err(fdo::Error::Failed)
    }

    /// Sets a mouse's report rate in Hz now and saves it for next time.
    fn set_polling_hz(&self, index: u32, hz: u32) -> fdo::Result<()> {
        crate::hardware::set(&self.shared, index as usize, None, Some(hz))
            .map_err(fdo::Error::Failed)
    }

    /// Renames a control and every binding that refers to it.
    fn rename_control(&self, index: u32, old: &str, new: &str) -> fdo::Result<()> {
        lock(&self.shared)
            .rename_control(index as usize, old, new)
            .map_err(fdo::Error::Failed)
    }
}

impl Api {
    fn set_paused(&self, decide: impl Fn(bool) -> bool) -> bool {
        let mut state = lock(&self.shared);
        let paused = decide(state.paused);
        state.set_paused(paused);
        state.paused
    }
}

/// The commands `mouse-profiles <command>` accepts besides `daemon`.
pub const COMMANDS: &[&str] = &[
    "status", "pause", "resume", "toggle", "reload", "quit", "capture", "profile",
];

/// Client side: runs one CLI command against the running daemon and returns
/// the text to print.
pub fn run_command(command: &str, argument: Option<&str>) -> Result<String, String> {
    let connection = Connection::session()
        .map_err(|error| format!("cannot connect to the session bus: {error}"))?;
    let paused_text =
        |paused: bool| format!("remapping: {}", if paused { "paused" } else { "active" });
    match (command, argument) {
        ("status", None) => call(&connection, "Status", &()),
        ("pause", None) => call(&connection, "Pause", &()).map(paused_text),
        ("resume", None) => call(&connection, "Resume", &()).map(paused_text),
        ("toggle", None) => call(&connection, "Toggle", &()).map(paused_text),
        ("reload", None) => call(&connection, "Reload", &()),
        ("quit", None) => call::<()>(&connection, "Quit", &()).map(|()| "stopping".to_string()),
        ("profile", Some(name)) => call(&connection, "SetOverride", &(name,)),
        ("profile", None) => {
            Err("say which profile, or \"auto\": mouse-profiles profile <name>".into())
        }
        ("capture", None) => capture(&connection),
        _ => Err(format!("\"{command}\" does not take that argument")),
    }
}

/// `mouse-profiles capture`: prints the first button pressed, then stops.
fn capture(connection: &Connection) -> Result<String, String> {
    call::<()>(connection, "StartCapture", &())?;
    eprintln!("Press a mouse button other than left click, or scroll the wheel...");
    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    let mut result = Err(
        "nothing was pressed in time (is the mouse connected? check `mouse-profiles status`)"
            .to_string(),
    );
    while Instant::now() < deadline {
        let (open, captured): (bool, Vec<String>) = call(connection, "PollCapture", &())?;
        if let Some(first) = captured.into_iter().next() {
            result = Ok(first);
            break;
        }
        if !open {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let _ = call::<()>(connection, "CancelCapture", &());
    result
}

fn call<R>(
    connection: &Connection,
    method: &str,
    body: &(impl zbus::export::serde::Serialize + zbus::zvariant::DynamicType),
) -> Result<R, String>
where
    R: for<'a> zbus::zvariant::DynamicDeserialize<'a>,
{
    let reply = connection
        .call_method(Some(bus_name()), OBJECT_PATH, Some(INTERFACE), method, body)
        .map_err(|error| match error {
            zbus::Error::MethodError(name, message, _) => {
                if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" {
                    "the daemon is not running (start it with: mouse-profiles daemon)".to_string()
                } else {
                    message.unwrap_or_else(|| name.to_string())
                }
            }
            other => format!("cannot reach the daemon: {other}"),
        })?;
    reply
        .body()
        .deserialize()
        .map_err(|error| format!("unexpected reply from the daemon: {error}"))
}
