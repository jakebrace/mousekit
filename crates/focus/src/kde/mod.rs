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
/// control interface. Must match SERVICE in focus.js. A placeholder until
/// the project's app ID is settled.
const BUS_NAME: &str = "io.github.mouseprofiles.FocusListener";
const OBJECT_PATH: &str = "/Focus";
/// The name KWin knows the script by, used to unload a stale copy.
const SCRIPT_NAME: &str = "mouse-profiles-focus";
/// KWin loads scripts from a file, so the embedded script is written out
/// at startup.
const SCRIPT_SOURCE: &str = include_str!("focus.js");

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

    fn cleanup(&self) {
        // Best effort: a script left behind only calls a service that is no
        // longer there, and the next start unloads it anyway.
        if let Ok(conn) = Connection::session() {
            let _ = unload_script(&conn);
        }
    }
}

/// The object the KWin script calls. zbus exposes `focus_changed` as the
/// D-Bus method `FocusChanged`.
struct Listener {
    on_focus: OnFocus,
}

#[interface(name = "io.github.mouseprofiles.Focus")]
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
            Ok(()) => eprintln!("mouse-profiles: KWin restarted; focus script reloaded"),
            Err(error) => eprintln!(
                "mouse-profiles: KWin restarted but the focus script did not reload: {error}"
            ),
        }
    }
    Err("lost the connection to the session bus".into())
}

/// Unloads any stale copy of the script, then loads and starts a fresh one.
fn load_script(conn: &Connection) -> Result<(), String> {
    unload_script(conn).map_err(|e| format!("cannot reach KWin's scripting interface: {e}"))?;

    let runtime_dir =
        std::env::var("XDG_RUNTIME_DIR").map_err(|_| "XDG_RUNTIME_DIR is not set".to_string())?;
    let script_path = format!("{runtime_dir}/{SCRIPT_NAME}.js");
    std::fs::write(&script_path, SCRIPT_SOURCE)
        .map_err(|e| format!("cannot write {script_path}: {e}"))?;

    let reply = conn
        .call_method(
            Some(KWIN),
            SCRIPTING_PATH,
            Some(SCRIPTING_INTERFACE),
            "loadScript",
            &(script_path.as_str(), SCRIPT_NAME),
        )
        .map_err(|e| format!("KWin refused to load the focus script: {e}"))?;
    let script_id: i32 = reply
        .body()
        .deserialize()
        .map_err(|e| format!("unexpected reply from KWin's loadScript: {e}"))?;
    if script_id < 0 {
        return Err(format!(
            "KWin did not load the focus script (id {script_id})"
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
    .map_err(|e| format!("KWin loaded the focus script but could not run it: {e}"))?;
    Ok(())
}

/// Asks KWin to unload our script. Returns whether a copy was loaded.
fn unload_script(conn: &Connection) -> zbus::Result<bool> {
    let reply = conn.call_method(
        Some(KWIN),
        SCRIPTING_PATH,
        Some(SCRIPTING_INTERFACE),
        "unloadScript",
        &(SCRIPT_NAME,),
    )?;
    reply.body().deserialize()
}
