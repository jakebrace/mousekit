//! Milestone 0, spike (b): load a KWin script that reports every focus change
//! to this program over D-Bus, and print each one. Throwaway code.
//!
//! Usage: spike-kwin-focus
//!
//! Press Enter in the terminal to stop. That unloads the script from KWin.

use std::future::pending;
use std::time::{SystemTime, UNIX_EPOCH};

use zbus::{Connection, connection, interface};

/// Our name on the session bus. Must match SERVICE in focus.js.
const SERVICE: &str = "io.github.mouseprofiles.SpikeFocus";
/// The name KWin knows the script by, used to unload a stale copy.
const SCRIPT_NAME: &str = "mouse-profiles-spike-focus";
/// The script is compiled into the binary and written out at startup,
/// because KWin loads scripts from a file path.
const SCRIPT_SOURCE: &str = include_str!("focus.js");

/// The object KWin's script calls. zbus turns `focus_changed` into the D-Bus
/// method name `FocusChanged`.
struct FocusListener;

#[interface(name = "io.github.mouseprofiles.SpikeFocus")]
impl FocusListener {
    fn focus_changed(
        &self,
        sent_ms: &str,
        class: &str,
        desktop_file: &str,
        caption: &str,
        pid: &str,
        kind: &str,
    ) {
        // The script stamped the message with KWin's clock; ours is the same
        // system clock, so the difference is the script-to-here delay.
        let delay = match sent_ms.parse::<u128>() {
            Ok(sent) => format!("{:>3} ms", now_ms().saturating_sub(sent)),
            Err(_) => "  ? ms".to_string(),
        };
        if kind == "none" {
            println!("[{delay}] (no window focused)");
        } else {
            println!(
                "[{delay}] class={class:?}  kind={kind}  pid={pid}  desktop_file={desktop_file:?}  title={caption:?}"
            );
        }
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn main() {
    // A second thread waits for Enter, unloads the script and exits. It uses
    // its own D-Bus connection because the main thread is busy serving.
    std::thread::spawn(|| {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(n) if n > 0 => {
                let result = zbus::block_on(async {
                    let conn = Connection::session().await?;
                    unload_script(&conn).await
                });
                match result {
                    Ok(_) => println!("Script unloaded. Bye."),
                    Err(e) => eprintln!("warning: could not unload the KWin script: {e}"),
                }
                std::process::exit(0);
            }
            // stdin closed: nothing to wait for, keep running until killed.
            _ => {}
        }
    });

    if let Err(e) = zbus::block_on(run()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    // Claim our bus name and start answering FocusChanged calls first, so the
    // script's immediate report of the current window has somewhere to land.
    let conn = connection::Builder::session()
        .name(SERVICE)
        .serve_at("/Focus", FocusListener)
        .build()
        .await
        .map_err(|e| format!("cannot set up the D-Bus service (is another copy running?): {e}"))?;

    // A copy left behind by a previous run would block loading under the same name.
    let was_loaded = unload_script(&conn)
        .await
        .map_err(|e| format!("cannot reach KWin's scripting interface: {e}"))?;
    if was_loaded {
        println!("Unloaded a stale copy of the script from a previous run.");
    }

    let runtime_dir =
        std::env::var("XDG_RUNTIME_DIR").map_err(|_| "XDG_RUNTIME_DIR is not set".to_string())?;
    let script_path = format!("{runtime_dir}/{SCRIPT_NAME}.js");
    std::fs::write(&script_path, SCRIPT_SOURCE)
        .map_err(|e| format!("cannot write {script_path}: {e}"))?;

    let reply = conn
        .call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "loadScript",
            &(script_path.as_str(), SCRIPT_NAME),
        )
        .await
        .map_err(|e| format!("KWin refused to load the script: {e}"))?;
    let script_id: i32 = reply
        .body()
        .deserialize()
        .map_err(|e| format!("unexpected reply from loadScript: {e}"))?;
    if script_id < 0 {
        return Err(format!(
            "KWin returned script id {script_id}, meaning it did not load"
        ));
    }

    // Loading only registers the script; this starts it.
    conn.call_method(
        Some("org.kde.KWin"),
        format!("/Scripting/Script{script_id}"),
        Some("org.kde.kwin.Script"),
        "run",
        &(),
    )
    .await
    .map_err(|e| format!("KWin loaded the script but could not run it: {e}"))?;

    println!("KWin script loaded (id {script_id}). Focus changes print below.");
    println!("The number in brackets is the delay from KWin's script to this program.");
    println!("Press Enter to stop.\n");

    // Nothing more to do on this thread except keep the connection served.
    pending::<()>().await;
    Ok(())
}

/// Asks KWin to unload our script. Returns whether a copy was loaded.
async fn unload_script(conn: &Connection) -> zbus::Result<bool> {
    let reply = conn
        .call_method(
            Some("org.kde.KWin"),
            "/Scripting",
            Some("org.kde.kwin.Scripting"),
            "unloadScript",
            &(SCRIPT_NAME,),
        )
        .await?;
    reply.body().deserialize()
}
