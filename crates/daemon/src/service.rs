//! The daemon's D-Bus interface on the session bus, and the client side the
//! CLI uses to call it. The GUI will use the same interface.
//!
//! Owning the bus name is also the "is a daemon already running" check.

use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::interface;

use crate::actions::{self, CAPTURE_TIMEOUT};
use crate::{Capture, Shared, instance, lock};

// Placeholders until the project's app ID is settled (see docs/PLAN.md).
const BUS_NAME: &str = "io.github.mouseprofiles.Daemon";
const OBJECT_PATH: &str = "/io/github/mouseprofiles/Daemon";
const INTERFACE: &str = "io.github.mouseprofiles.Daemon1";

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

    /// Starts waiting for the next button press or scroll tick on any
    /// grabbed mouse. Poll for the result with `PollCapture`.
    fn start_capture(&self) {
        lock(&self.shared).start_capture();
    }

    /// Returns ("waiting", ""), ("done", "<event name>") once, or
    /// ("idle", "") if no capture is running (never started, timed out, or
    /// already collected).
    fn poll_capture(&self) -> (String, String) {
        let mut state = lock(&self.shared);
        if state.capturing() {
            return ("waiting".into(), String::new());
        }
        match std::mem::replace(&mut state.capture, Capture::Idle) {
            Capture::Done(trigger) => ("done".into(), trigger.to_string()),
            _ => ("idle".into(), String::new()),
        }
    }

    fn cancel_capture(&self) {
        lock(&self.shared).capture = Capture::Idle;
    }

    fn quit(&self) {
        actions::quit(&self.shared);
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

fn capture(connection: &Connection) -> Result<String, String> {
    call::<()>(connection, "StartCapture", &())?;
    eprintln!("Press a mouse button or scroll the wheel...");
    let deadline = Instant::now() + CAPTURE_TIMEOUT;
    while Instant::now() < deadline {
        let (state, value): (String, String) = call(connection, "PollCapture", &())?;
        match state.as_str() {
            "done" => return Ok(value),
            "waiting" => std::thread::sleep(Duration::from_millis(30)),
            _ => break,
        }
    }
    let _ = call::<()>(connection, "CancelCapture", &());
    Err(
        "nothing was pressed in time (is the mouse connected? check `mouse-profiles status`)"
            .into(),
    )
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
