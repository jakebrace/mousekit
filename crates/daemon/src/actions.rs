//! Every way the daemon's state can change, in one place. The focus
//! backend, the D-Bus interface and the tray menu all call these, so they
//! cannot disagree about what "pause" or "switch profile" means.

use std::time::{Duration, Instant};

use mouse_profiles_core::event::{EV_MSC, REL_HWHEEL_HI_RES, REL_WHEEL_HI_RES};
use mouse_profiles_core::{Decision, Event, Key, Trigger, Window, WindowKind, decide};

use crate::{CaptureSession, Shared, State, autostart, load, lock, log};

/// How many recently focused windows `status` lists.
const RECENT_WINDOWS: usize = 6;

/// How long a capture stays open with nothing pressed before it ends by
/// itself. Every captured button restarts the clock.
pub(crate) const CAPTURE_TIMEOUT: Duration = Duration::from_secs(30);

impl State {
    /// Writes whatever is in `out` to the virtual device and clears it.
    /// If the virtual device stops accepting writes there is nothing useful
    /// left to do, so exit and let the kernel release the grab.
    pub fn flush(&mut self) {
        if let Err(error) = self.output.emit(&self.out) {
            log(&format!(
                "cannot write to the virtual device: {error}; exiting"
            ));
            std::process::exit(1);
        }
        self.out.clear();
    }

    /// Lets go of every key any engine is holding down.
    pub fn release_everything(&mut self) {
        for engine in &mut self.engines {
            engine.release_all(&mut self.out);
        }
        self.flush();
    }

    /// Tells the tray that something it shows has changed.
    fn notify_tray(&self) {
        if let Some(refresh) = &self.tray_refresh {
            let _ = refresh.send(());
        }
    }

    /// Points every engine at the profile that should be active: the manual
    /// override if there is one, otherwise what the focused window calls
    /// for. Buttons held right now keep what they emitted; the engine
    /// guarantees that.
    pub fn apply_profile(&mut self) {
        let wanted = self.override_profile.unwrap_or(self.auto_profile);
        let changed = wanted != self.active_profile;
        self.active_profile = wanted;
        for (device, engine) in self.engines.iter_mut().enumerate() {
            engine.set_profile(self.loaded.table(device, wanted), &mut self.out);
        }
        // Switching cuts macros short; this releases the keys they held.
        self.flush();
        if changed {
            log(&format!("profile: {}", self.loaded.profile_name(wanted)));
            self.notify_tray();
        }
    }

    /// Called by the focus backend for every focus change.
    pub fn window_focused(&mut self, window: Window) {
        self.auto_profile = match decide(&self.loaded.profiles, &window) {
            Decision::Keep => self.auto_profile,
            Decision::Default => None,
            Decision::Profile(index) => Some(index),
        };
        self.apply_profile();
        if window.kind == WindowKind::App {
            self.recent.retain(|seen| seen.class != window.class);
            self.recent.insert(0, window);
            self.recent.truncate(RECENT_WINDOWS);
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        if paused {
            // Let go of remapped keys before bindings stop applying.
            self.release_everything();
        }
        self.paused = paused;
        log(if paused { "paused" } else { "resumed" });
        self.notify_tray();
    }

    /// Forces a profile by name, or returns to automatic switching.
    /// `"auto"` (or an empty name) clears the override; the default
    /// profile's own name selects the default.
    pub fn set_override(&mut self, name: &str) -> Result<(), String> {
        self.override_profile = if name.is_empty() || name.eq_ignore_ascii_case("auto") {
            None
        } else if name.eq_ignore_ascii_case(&self.loaded.default.name) {
            Some(None)
        } else {
            let index = self
                .loaded
                .profiles
                .iter()
                .position(|profile| profile.name.eq_ignore_ascii_case(name));
            match index {
                Some(index) => Some(Some(index)),
                None => return Err(format!("there is no profile named \"{name}\"")),
            }
        };
        self.apply_profile();
        self.notify_tray();
        Ok(())
    }

    /// Re-reads the config directory. On any error the old config stays.
    pub fn reload(&mut self) -> Result<String, String> {
        let loaded = load::config_dir().and_then(|dir| load::load(&dir))?;
        let same_devices = loaded.config.devices.len() == self.loaded.config.devices.len()
            && loaded
                .config
                .devices
                .iter()
                .zip(&self.loaded.config.devices)
                .all(|(new, old)| new.ids == old.ids);
        if !same_devices {
            return Err("the [[device]] list changed; restart the daemon to apply it".into());
        }

        // Profiles are tracked by position, and positions may have moved.
        // Carry each one over by name; one that no longer exists falls back
        // to the default (or, for the override, to automatic).
        let find = |index: Option<usize>| -> Option<Option<usize>> {
            match index {
                None => Some(None),
                Some(index) => {
                    let name = &self.loaded.profiles[index].name;
                    let position = loaded.profiles.iter().position(|p| &p.name == name)?;
                    Some(Some(position))
                }
            }
        };
        let auto_profile = find(self.auto_profile).unwrap_or(None);
        let override_profile = self.override_profile.and_then(find);

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
        self.loaded = loaded;
        self.auto_profile = auto_profile;
        self.override_profile = override_profile;
        self.apply_profile();
        if crate::instance().is_none() {
            self.sync_autostart();
        }
        log("config reloaded");
        self.notify_tray();
        Ok(reply.join("\n"))
    }

    /// Makes the login autostart entry match `settings.autostart`.
    pub fn sync_autostart(&self) {
        match autostart::sync(self.loaded.config.settings.autostart) {
            Ok(Some(note)) => log(&note),
            Ok(None) => {}
            Err(error) => log(&format!("warning: {error}")),
        }
    }

    pub fn status_text(&self) -> String {
        let mut lines = vec![format!(
            "remapping: {}",
            if self.paused { "paused" } else { "active" }
        )];
        let mode = match self.override_profile {
            Some(_) => " (chosen by hand; `mouse-profiles profile auto` to undo)",
            None => "",
        };
        lines.push(format!(
            "profile: {}{mode}",
            self.loaded.profile_name(self.active_profile)
        ));
        lines.push(format!("focus switching: {}", self.focus_status));
        if !self.recent.is_empty() {
            lines.push("recent windows (newest first):".into());
        }
        for window in &self.recent {
            lines.push(format!(
                "  class \"{}\"  program \"{}\"  title \"{}\"",
                window.class,
                window.exe.as_deref().unwrap_or("?"),
                window.title
            ));
        }
        for device in &self.loaded.config.devices {
            lines.push(format!("device: {}", device.name));
        }
        if self.active.is_empty() {
            lines.push("connected: none".into());
        }
        for path in &self.active {
            lines.push(format!("connected: {}", path.display()));
        }
        lines.join("\n")
    }

    /// Starts a capture session. Until it ends, remapping is suspended and
    /// every button pressed or scroll direction used is recorded.
    pub fn start_capture(&mut self) {
        self.release_everything();
        self.capture = Some(CaptureSession {
            deadline: Instant::now() + CAPTURE_TIMEOUT,
            seen: Vec::new(),
            unreported: Vec::new(),
        });
    }

    /// True while a capture session is open and not overdue.
    pub fn capturing(&self) -> bool {
        self.capture
            .as_ref()
            .is_some_and(|session| Instant::now() < session.deadline)
    }

    /// Hands over what has been captured since the last call. The flag says
    /// whether the session is still open; an overdue session is closed here.
    pub fn poll_capture(&mut self) -> (bool, Vec<Trigger>) {
        let open = self.capturing();
        let captured = match &mut self.capture {
            Some(session) => std::mem::take(&mut session.unreported),
            None => Vec::new(),
        };
        if !open {
            self.capture = None;
        }
        (open, captured)
    }

    /// Handles one frame from a mouse during a capture session. Presses and
    /// scroll ticks are recorded and swallowed; everything else is mirrored
    /// unchanged.
    ///
    /// Left click is the exception: it is never captured and always works,
    /// so the session can be stopped with the mouse.
    pub fn capture_frame(&mut self, frame: &[Event]) {
        let Some(session) = &mut self.capture else {
            return;
        };
        let capturable = |event: &Event| {
            Trigger::from_event(event).filter(|trigger| *trigger != Trigger::Key(Key::BTN_LEFT))
        };
        let found: Vec<Trigger> = frame.iter().filter_map(capturable).collect();
        let found_scroll = found
            .iter()
            .any(|trigger| matches!(trigger, Trigger::Scroll { .. }));
        for event in frame {
            let swallow = capturable(event).is_some()
                || !found.is_empty() && event.kind == EV_MSC
                || found_scroll && matches!(event.code, REL_WHEEL_HI_RES | REL_HWHEEL_HI_RES);
            if !swallow {
                self.out.push(*event);
            }
        }
        if !self.out.is_empty() {
            self.out.push(Event::SYN);
        }
        for trigger in found {
            session.deadline = Instant::now() + CAPTURE_TIMEOUT;
            if !session.seen.contains(&trigger) {
                session.seen.push(trigger);
                session.unreported.push(trigger);
            }
        }
    }
}

/// Stops the daemon cleanly. The exit is slightly delayed so whoever asked
/// (a D-Bus caller, the tray) gets its reply first.
pub(crate) fn quit(shared: &Shared) {
    let mut state = lock(shared);
    state.release_everything();
    if let Some(backend) = &state.focus_backend {
        backend.cleanup();
    }
    log("quit requested; exiting");
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(100));
        // Exiting closes every device, which drops the grabs.
        std::process::exit(0);
    });
}
