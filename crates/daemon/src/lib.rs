//! The always-running half of the app.
//!
//! Threads, all plain blocking code:
//! - the main thread watches for configured mice appearing ([`run`]);
//! - one reader thread per grabbed mouse node pushes its events through the
//!   engine and out of the virtual device (`reader`);
//! - one thread answers `pause`, `status` and friends on a local socket
//!   ([`control`]). Milestone 3 replaces that socket with D-Bus;
//! - one thread runs the focus backend, which calls [`on_focus`] whenever
//!   the focused window changes.
//!
//! They share one [`State`] behind a mutex.

pub mod control;
pub mod load;
mod reader;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use mouse_profiles_core::{Decision, Engine, Event, Window, WindowKind, decide};
use mouse_profiles_focus::FocusBackend;
use mouse_profiles_input::{Mouse, NodeInfo, VirtualMouse, list_nodes};

use crate::load::Loaded;

/// How often to look for mice appearing. Reading a directory once a second
/// costs nothing measurable; a udev monitor can replace it later.
const SCAN_INTERVAL: Duration = Duration::from_secs(1);

/// How many recently focused windows `status` lists.
const RECENT_WINDOWS: usize = 6;

/// Everything the threads share.
pub(crate) struct State {
    pub loaded: Loaded,
    /// One engine per `[[device]]` block, same order as `loaded.config.devices`.
    pub engines: Vec<Engine>,
    pub output: VirtualMouse,
    pub paused: bool,
    /// The active app-specific profile, as an index into `loaded.profiles`;
    /// `None` means the default profile.
    pub active_profile: Option<usize>,
    /// The most recently focused app windows, newest first, one per window
    /// class. `status` lists them so you can look up a class for a profile.
    pub recent: Vec<Window>,
    /// "kde", or why focus switching is off.
    pub focus_status: String,
    pub focus_backend: Option<Arc<dyn FocusBackend>>,
    /// Nodes that currently have a reader thread.
    pub active: HashSet<PathBuf>,
    /// Scratch buffer for engine output.
    pub out: Vec<Event>,
}

pub(crate) type Shared = Arc<Mutex<State>>;

/// Locks the state. If another thread panicked while holding the lock, the
/// state may be half-updated, so the only safe move is to stop: exiting
/// releases the grab and gives the mouse back.
pub(crate) fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.lock().unwrap_or_else(|_| {
        log("internal error: a thread panicked; exiting so the mouse returns to normal");
        std::process::exit(1);
    })
}

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

    /// Points every engine at the tables for `active`. Buttons held right
    /// now keep what they emitted; the engine guarantees that.
    pub fn apply_profile(&mut self, active: Option<usize>) {
        self.active_profile = active;
        for (device, engine) in self.engines.iter_mut().enumerate() {
            engine.set_profile(self.loaded.table(device, active));
        }
    }

    /// Lets go of every key any engine is holding down.
    pub fn release_everything(&mut self) {
        for engine in &mut self.engines {
            engine.release_all(&mut self.out);
        }
        self.flush();
    }
}

pub(crate) fn log(message: &str) {
    eprintln!("mouse-profiles: {message}");
}

/// Runs the daemon. Returns only on a startup error; after that it runs
/// until `mouse-profiles quit` or the process is killed.
pub fn run() -> Result<(), String> {
    let config_dir = load::config_dir()?;
    let loaded = load::load(&config_dir)?;
    log(&format!("config loaded from {}", config_dir.display()));
    for warning in &loaded.warnings {
        log(&format!("warning: {warning}"));
    }

    // Claim the control socket before touching any device, so a second
    // daemon stops here instead of fighting the first one for the mouse.
    let listener = control::bind()?;

    // Tests give their daemon's device another name so they can never pick
    // up (and grab) the device of a daemon that is really in use.
    let output_name =
        std::env::var("MOUSE_PROFILES_VIRTUAL_NAME").unwrap_or(VirtualMouse::NAME.to_string());
    let output = VirtualMouse::create(&output_name).map_err(|error| {
        format!(
            "cannot create the virtual device: {error}\n\
             Your user needs access to /dev/uinput; see packaging/udev/."
        )
    })?;

    let engines = loaded
        .config
        .devices
        .iter()
        .map(|_| Engine::new())
        .collect();
    for device in &loaded.config.devices {
        log(&format!("watching for \"{}\"", device.name));
    }

    let focus_backend: Option<Arc<dyn FocusBackend>> =
        mouse_profiles_focus::detect().map(Arc::from);
    let focus_status = match &focus_backend {
        Some(backend) => backend.name().to_string(),
        None => "off (no backend for this desktop)".to_string(),
    };
    if focus_backend.is_none() {
        log("no focus backend for this desktop; only the default profile is used");
    }

    let mut state = State {
        loaded,
        engines,
        output,
        paused: false,
        active_profile: None,
        recent: Vec::new(),
        focus_status,
        focus_backend: focus_backend.clone(),
        active: HashSet::new(),
        out: Vec::new(),
    };
    state.apply_profile(None);
    let shared: Shared = Arc::new(Mutex::new(state));

    if let Some(backend) = focus_backend {
        let focus_state = shared.clone();
        thread::spawn(move || {
            let callback_state = focus_state.clone();
            let result = backend.run(Box::new(move |window| on_focus(&callback_state, window)));
            // run() only returns when the backend cannot work.
            let reason = result.err().unwrap_or_else(|| "backend stopped".into());
            log(&format!(
                "focus switching is off: {reason}; staying on the default profile"
            ));
            let mut state = lock(&focus_state);
            state.focus_status = format!("off ({reason})");
            state.apply_profile(None);
        });
    }

    let control_state = shared.clone();
    thread::spawn(move || control::serve(listener, control_state));

    watch_for_mice(shared)
}

/// Called by the focus backend for every focus change.
fn on_focus(shared: &Shared, window: Window) {
    let mut state = lock(shared);
    let wanted = match decide(&state.loaded.profiles, &window) {
        Decision::Keep => state.active_profile,
        Decision::Default => None,
        Decision::Profile(index) => Some(index),
    };
    if wanted != state.active_profile {
        state.apply_profile(wanted);
        log(&format!(
            "profile: {} (window class \"{}\")",
            state.loaded.profile_name(wanted),
            window.class
        ));
    }
    if window.kind == WindowKind::App {
        state.recent.retain(|seen| seen.class != window.class);
        state.recent.insert(0, window);
        state.recent.truncate(RECENT_WINDOWS);
    }
}

/// The main loop: once a second, start a reader for any configured mouse
/// node that does not have one. Covers startup, replugging, and a wireless
/// mouse waking up, all the same way.
fn watch_for_mice(shared: Shared) -> ! {
    // Remember the last error per node so a persistent problem (no udev
    // rule) is logged once, not every second.
    let mut last_error: Option<(PathBuf, String)> = None;
    loop {
        match list_nodes() {
            Ok(nodes) => {
                for node in nodes {
                    try_start_reader(&shared, node, &mut last_error);
                }
            }
            Err(error) => log(&format!("cannot list /dev/input: {error}")),
        }
        thread::sleep(SCAN_INTERVAL);
    }
}

fn try_start_reader(shared: &Shared, node: NodeInfo, last_error: &mut Option<(PathBuf, String)>) {
    let device_index = {
        let state = lock(shared);
        if state.active.contains(&node.path) {
            return;
        }
        let position = state
            .loaded
            .config
            .devices
            .iter()
            .position(|device| device.ids.contains(&node.id));
        match position {
            Some(index) => index,
            None => return,
        }
    };

    let mouse = match Mouse::open(&node.path) {
        Ok(mouse) => mouse,
        Err(error) => {
            let seen = (node.path.clone(), error.to_string());
            if last_error.as_ref() != Some(&seen) {
                log(&format!(
                    "found {} \"{}\" at {} but cannot open it yet: {error} (retrying; if this persists, the udev rule is missing)",
                    node.id,
                    node.name,
                    node.path.display()
                ));
                *last_error = Some(seen);
            }
            return;
        }
    };
    *last_error = None;

    lock(shared).active.insert(node.path.clone());
    let shared = shared.clone();
    thread::spawn(move || reader::run(shared, device_index, node, mouse));
}
