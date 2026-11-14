//! Finding out which window has focus.
//!
//! Wayland has no universal way to ask, so each desktop gets its own
//! [`FocusBackend`]. All compositor-specific code in the project lives in
//! this crate; the rest only sees [`Window`] values.

mod kde;

use std::path::Path;

pub use mouse_profiles_core::{Window, WindowKind};

/// Called with every focus change, from the backend's own thread.
pub type OnFocus = Box<dyn Fn(Window) + Send + Sync>;

pub trait FocusBackend: Send + Sync {
    /// Short name for logs and status, e.g. "kde".
    fn name(&self) -> &'static str;

    /// Starts reporting focus changes, beginning with the window focused
    /// right now. Blocks for as long as the backend is working; returns only
    /// if it cannot start or loses its connection.
    fn run(&self, on_focus: OnFocus) -> Result<(), String>;

    /// Undoes anything `run` left behind in the desktop. Safe to call from
    /// another thread while `run` is blocked, and when `run` never started.
    fn cleanup(&self);
}

/// Picks the backend for the running desktop, if there is one.
///
/// Setting `MOUSE_PROFILES_FOCUS=none` disables focus switching, which tests
/// use to stay out of the real desktop.
pub fn detect() -> Option<Box<dyn FocusBackend>> {
    if std::env::var_os("MOUSE_PROFILES_FOCUS").is_some_and(|value| value == "none") {
        return None;
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if desktop
        .split(':')
        .any(|part| part.eq_ignore_ascii_case("KDE"))
    {
        return Some(Box::new(kde::Kde));
    }
    None
}

/// The program name behind a process ID, for the `exe` matcher.
///
/// Uses the first word of the command line rather than the executable link,
/// because for a Windows game under Proton the link is Wine itself while the
/// command line starts with the game's own `.exe` path.
pub(crate) fn exe_name(pid: u32) -> Option<String> {
    let cmdline = std::fs::read(Path::new("/proc").join(pid.to_string()).join("cmdline")).ok()?;
    let first = cmdline.split(|byte| *byte == 0).next()?;
    let first = String::from_utf8_lossy(first);
    let name = first.rsplit(['/', '\\']).next()?;
    (!name.is_empty()).then(|| name.to_string())
}
