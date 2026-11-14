//! A tiny local control channel: one text command per connection over a Unix
//! socket in the user's runtime directory. It gives `mouse-profiles pause`
//! something to talk to before the D-Bus service exists (Milestone 3), and
//! doubles as the "is a daemon already running" check.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use crate::{Shared, load, lock, log};

/// The commands the daemon understands.
pub const COMMANDS: &[&str] = &["status", "pause", "resume", "toggle", "reload", "quit"];

fn socket_path() -> Result<PathBuf, String> {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir).join("mouse-profiles.sock")),
        None => Err("XDG_RUNTIME_DIR is not set (are you in a desktop session?)".into()),
    }
}

/// Client side: sends one command to the running daemon and returns its reply.
pub fn send(command: &str) -> Result<String, String> {
    let path = socket_path()?;
    let mut stream = UnixStream::connect(&path).map_err(|_| {
        "the daemon is not running (start it with: mouse-profiles daemon)".to_string()
    })?;
    writeln!(stream, "{command}").map_err(|error| format!("cannot send the command: {error}"))?;
    let mut reply = String::new();
    stream
        .read_to_string(&mut reply)
        .map_err(|error| format!("cannot read the reply: {error}"))?;
    Ok(reply.trim_end().to_string())
}

/// Daemon side: claims the socket, refusing if another daemon answers on it.
pub(crate) fn bind() -> Result<UnixListener, String> {
    let path = socket_path()?;
    if UnixStream::connect(&path).is_ok() {
        return Err("another mouse-profiles daemon is already running".into());
    }
    // Nobody answered, so any file there is left over from a dead daemon.
    let _ = std::fs::remove_file(&path);
    UnixListener::bind(&path).map_err(|error| format!("cannot create {}: {error}", path.display()))
}

/// Daemon side: answers commands forever.
pub(crate) fn serve(listener: UnixListener, shared: Shared) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else { continue };
        let mut command = String::new();
        if BufReader::new(&stream).read_line(&mut command).is_err() {
            continue;
        }
        let command = command.trim();
        let reply = handle(command, &shared);
        let _ = writeln!(stream, "{reply}");
        if command == "quit" {
            // Held keys were released in handle(). Exiting drops every grab.
            log("quit requested; exiting");
            let _ = stream.flush();
            drop(stream);
            if let Ok(path) = socket_path() {
                let _ = std::fs::remove_file(path);
            }
            std::process::exit(0);
        }
    }
}

fn handle(command: &str, shared: &Shared) -> String {
    let mut state = lock(shared);
    match command {
        "status" => {
            let mut lines = vec![format!(
                "remapping: {}",
                if state.paused { "paused" } else { "active" }
            )];
            lines.push(format!(
                "profile: {}",
                state.loaded.profile_name(state.active_profile)
            ));
            lines.push(format!("focus switching: {}", state.focus_status));
            if !state.recent.is_empty() {
                lines.push("recent windows (newest first):".into());
            }
            for window in &state.recent {
                lines.push(format!(
                    "  class \"{}\"  program \"{}\"  title \"{}\"",
                    window.class,
                    window.exe.as_deref().unwrap_or("?"),
                    window.title
                ));
            }
            for device in &state.loaded.config.devices {
                lines.push(format!("device: {}", device.name));
            }
            if state.active.is_empty() {
                lines.push("connected: none".into());
            }
            for path in &state.active {
                lines.push(format!("connected: {}", path.display()));
            }
            lines.join("\n")
        }
        "pause" | "resume" | "toggle" => {
            let pause = match command {
                "pause" => true,
                "resume" => false,
                _ => !state.paused,
            };
            if pause && !state.paused {
                // Let go of remapped keys before bindings stop applying.
                state.release_everything();
            }
            state.paused = pause;
            log(if pause { "paused" } else { "resumed" });
            format!("remapping: {}", if pause { "paused" } else { "active" })
        }
        "reload" => {
            let loaded = match load::config_dir().and_then(|dir| load::load(&dir)) {
                Ok(loaded) => loaded,
                Err(error) => return format!("reload failed, keeping the old config:\n{error}"),
            };
            let same_devices = loaded.config.devices.len() == state.loaded.config.devices.len()
                && loaded
                    .config
                    .devices
                    .iter()
                    .zip(&state.loaded.config.devices)
                    .all(|(new, old)| new.ids == old.ids);
            if !same_devices {
                return "the [[device]] list changed; restart the daemon to apply it".into();
            }
            // Stay on the same profile if it still exists under the same name.
            let active_name = state.loaded.profile_name(state.active_profile).to_string();
            let active = loaded
                .profiles
                .iter()
                .position(|profile| profile.name == active_name);
            let mut reply = vec![format!(
                "reloaded ({} app profile(s))",
                loaded.profiles.len()
            )];
            reply.extend(
                loaded
                    .warnings
                    .iter()
                    .map(|warning| format!("warning: {warning}")),
            );
            state.loaded = loaded;
            // Held buttons keep what they emitted; the engine handles that.
            state.apply_profile(active);
            log("config reloaded");
            reply.join("\n")
        }
        "quit" => {
            state.release_everything();
            if let Some(backend) = &state.focus_backend {
                backend.cleanup();
            }
            "stopping".into()
        }
        other => format!("unknown command \"{other}\""),
    }
}
