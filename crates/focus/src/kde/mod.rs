//! KDE Plasma 6 backend.
//!
//! KWin can run small JavaScript scripts. We hand it one (`focus.js`) that
//! calls a D-Bus method on us every time the active window changes, so there
//! is no polling: KWin pushes each change as it happens.

use std::time::Duration;

use zbus::blocking::{Connection, connection, fdo};
use zbus::interface;

use crate::{FocusBackend, FocusEvent, OnFocus, Window, WindowKind, exe_name};

/// This backend's own name on the session bus, separate from the daemon's
/// control interface. Must match SERVICE in focus.js.
const BUS_NAME: &str = "io.github.jakebrace.mousekit.Focus";
const OBJECT_PATH: &str = "/Focus";
/// The name KWin knows the script by, used to unload a stale copy.
const SCRIPT_NAME: &str = "mousekit-focus";
/// KWin loads scripts from a file, so the embedded script is written out
/// at startup.
const SCRIPT_SOURCE: &str = include_str!("focus.js");
/// The one-off script that brings a window to the front.
const ACTIVATE_NAME: &str = "mousekit-activate";
const ACTIVATE_SOURCE: &str = include_str!("activate.js");

const KWIN: &str = "org.kde.KWin";
const SCRIPTING_PATH: &str = "/Scripting";
const SCRIPTING_INTERFACE: &str = "org.kde.kwin.Scripting";

pub(crate) struct Kde;

impl FocusBackend for Kde {
    fn name(&self) -> &'static str {
        "kde"
    }

    fn run(&self, on_focus: OnFocus) -> Result<(), String> {
        run(on_focus)
    }

    fn activate(&self, class: &str) -> Result<(), String> {
        let conn =
            Connection::session().map_err(|e| format!("cannot connect to the session bus: {e}"))?;
        // Rust's quoting of a plain string is also valid JavaScript.
        let source = format!("const CLASS = {class:?};\n{ACTIVATE_SOURCE}");
        // KWin runs the script shortly after being asked, so it is left
        // loaded; the next call (or cleanup) unloads it.
        run_script(&conn, ACTIVATE_NAME, &source)
    }

    fn cleanup(&self) {
        // Best effort: a script left behind only calls a service that is no
        // longer there, and the next start unloads it anyway.
        if let Ok(conn) = Connection::session() {
            let _ = unload_script(&conn, SCRIPT_NAME);
            let _ = unload_script(&conn, ACTIVATE_NAME);
        }
    }
}

/// The object the KWin script calls. zbus exposes `focus_changed` as the
/// D-Bus method `FocusChanged`.
struct Listener {
    on_focus: OnFocus,
}

#[interface(name = "io.github.jakebrace.mousekit.Focus1")]
impl Listener {
    fn focus_changed(&self, class: &str, title: &str, pid: &str, kind: &str) {
        let kind = match kind {
            "app" => WindowKind::App,
            "lock" => WindowKind::LockScreen,
            _ => WindowKind::Shell,
        };
        (self.on_focus)(FocusEvent::Focused(Window {
            class: class.to_string(),
            title: title.to_string(),
            exe: exe_of(pid),
            kind,
        }));
    }

    /// One window per line, as "class<TAB>pid<TAB>title".
    fn windows_changed(&self, list: &str) {
        let windows = list
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(3, '\t');
                let class = fields.next()?;
                let pid = fields.next()?;
                let title = fields.next().unwrap_or("");
                Some(Window {
                    class: class.to_string(),
                    title: title.to_string(),
                    exe: exe_of(pid),
                    kind: WindowKind::App,
                })
            })
            .collect();
        (self.on_focus)(FocusEvent::Windows(windows));
    }
}

fn exe_of(pid: &str) -> Option<String> {
    pid.parse().ok().filter(|pid| *pid != 0).and_then(exe_name)
}

fn run(on_focus: OnFocus) -> Result<(), String> {
    // Claim the bus name and start answering before loading the script, so
    // its immediate report of the current window has somewhere to land.
    let conn = connection::Builder::session()
        .and_then(|builder| builder.name(BUS_NAME))
        .and_then(|builder| builder.serve_at(OBJECT_PATH, Listener { on_focus }))
        .and_then(|builder| builder.build())
        .map_err(|e| format!("cannot set up the focus listener on D-Bus: {e}"))?;

    load_script(&conn)?;

    // A script loaded over D-Bus dies with KWin. Watch for KWin coming back
    // (after a crash or `kwin_wayland --replace`) and load it again.
    let dbus =
        fdo::DBusProxy::new(&conn).map_err(|e| format!("cannot watch the session bus: {e}"))?;
    let owner_changes = dbus
        .receive_name_owner_changed_with_args(&[(0, KWIN)])
        .map_err(|e| format!("cannot watch for KWin restarts: {e}"))?;
    for signal in owner_changes {
        let kwin_is_back = signal.args().is_ok_and(|args| args.new_owner().is_some());
        if !kwin_is_back {
            continue;
        }
        // KWin owns its bus name slightly before scripting is ready.
        let mut result = Err("not tried".to_string());
        for _ in 0..10 {
            std::thread::sleep(Duration::from_millis(500));
            result = load_script(&conn);
            if result.is_ok() {
                break;
            }
        }
        match result {
            Ok(()) => eprintln!("mousekit: KWin restarted; focus script reloaded"),
            Err(error) => {
                eprintln!("mousekit: KWin restarted but the focus script did not reload: {error}")
            }
        }
    }
    Err("lost the connection to the session bus".into())
}

/// Loads and starts the focus script.
fn load_script(conn: &Connection) -> Result<(), String> {
    run_script(conn, SCRIPT_NAME, SCRIPT_SOURCE)
}

/// Unloads any stale copy of the script called `name`, then loads and
/// starts a fresh one from `source`.
fn run_script(conn: &Connection, name: &str, source: &str) -> Result<(), String> {
    unload_script(conn, name)
        .map_err(|e| format!("cannot reach KWin's scripting interface: {e}"))?;

    let runtime_dir =
        std::env::var("XDG_RUNTIME_DIR").map_err(|_| "XDG_RUNTIME_DIR is not set".to_string())?;
    let script_path = format!("{runtime_dir}/{name}.js");
    std::fs::write(&script_path, source).map_err(|e| format!("cannot write {script_path}: {e}"))?;

    let reply = conn
        .call_method(
            Some(KWIN),
            SCRIPTING_PATH,
            Some(SCRIPTING_INTERFACE),
            "loadScript",
            &(script_path.as_str(), name),
        )
        .map_err(|e| format!("KWin refused to load the script {name}: {e}"))?;
    let script_id: i32 = reply
        .body()
        .deserialize()
        .map_err(|e| format!("unexpected reply from KWin's loadScript: {e}"))?;
    if script_id < 0 {
        return Err(format!(
            "KWin did not load the script {name} (id {script_id})"
        ));
    }

    // Loading only registers the script; this starts it.
    conn.call_method(
        Some(KWIN),
        format!("{SCRIPTING_PATH}/Script{script_id}"),
        Some("org.kde.kwin.Script"),
        "run",
        &(),
    )
    .map_err(|e| format!("KWin loaded the script {name} but could not run it: {e}"))?;
    Ok(())
}

/// Asks KWin to unload one of our scripts. Returns whether a copy was loaded.
fn unload_script(conn: &Connection, name: &str) -> zbus::Result<bool> {
    let reply = conn.call_method(
        Some(KWIN),
        SCRIPTING_PATH,
        Some(SCRIPTING_INTERFACE),
        "unloadScript",
        &(name,),
    )?;
    reply.body().deserialize()
}
