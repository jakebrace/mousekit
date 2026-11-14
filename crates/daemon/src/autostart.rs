//! The "start at login" entry: a .desktop file in the user's autostart
//! directory, which every freedesktop desktop runs at login.

use std::fs;
use std::io;
use std::path::PathBuf;

fn entry_path() -> Result<PathBuf, String> {
    let config_home = match std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?).join(".config"),
    };
    Ok(config_home.join("autostart/mousekit.desktop"))
}

/// Creates, updates or removes the autostart entry to match `enabled`.
/// Returns a note for the log when something changed.
pub(crate) fn sync(enabled: bool) -> Result<Option<String>, String> {
    let path = entry_path()?;
    if !enabled {
        return match fs::remove_file(&path) {
            Ok(()) => Ok(Some(format!(
                "autostart is off; removed {}",
                path.display()
            ))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
        };
    }

    // Point the entry at whichever binary is running now, so it keeps
    // working when the binary moves (a build directory today, /usr/bin once
    // packaged).
    let exe = std::env::current_exe()
        .map_err(|error| format!("cannot find the daemon's own path for autostart: {error}"))?;
    let wanted = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=MouseKit\n\
         Comment=Per-application mouse button remapping\n\
         Exec=\"{}\" daemon\n\
         Icon=input-mouse\n\
         Terminal=false\n",
        exe.display()
    );
    if fs::read_to_string(&path).is_ok_and(|current| current == wanted) {
        return Ok(None);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    }
    fs::write(&path, wanted)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(Some(format!(
        "autostart is on; wrote {} (set autostart = false in config.toml to remove it)",
        path.display()
    )))
}
