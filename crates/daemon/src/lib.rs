//! The always-running half of the app.
//!
//! Threads, all plain blocking code:
//! - the main thread watches for configured mice appearing ([`run`]);
//! - one reader thread per grabbed mouse node pushes its events through the
//!   engine and out of the virtual device (`reader`);
//! - one thread runs the focus backend, which calls `on_focus` whenever the
//!   focused window changes;
//! - the D-Bus library answers the control interface ([`service`]) on its
//!   own thread, and the tray library does the same for the tray icon.
//!
//! They share one [`State`] behind a mutex. Everything that changes the
//! state lives in `actions`, so the tray, the D-Bus interface and the focus
//! backend all go through the same code.

mod actions;
mod autostart;
mod edit;
pub mod load;
mod reader;
pub mod service;
mod tray;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use mouse_profiles_core::{Engine, Event, Trigger, Window};
use mouse_profiles_focus::{FocusBackend, FocusEvent};
use mouse_profiles_input::{Mouse, NodeInfo, VirtualMouse, list_nodes};

use crate::load::Loaded;

/// How often to look for mice appearing. Reading a directory once a second
/// costs nothing measurable; a udev monitor can replace it later.
const SCAN_INTERVAL: Duration = Duration::from_secs(1);

/// How long to wait before trying a failed focus backend again. At login the
/// daemon can start before the window manager is ready.
const FOCUS_RETRY: Duration = Duration::from_secs(5);

/// A button capture in progress: while it lasts, remapping is suspended and
/// every button pressed is recorded instead of acted on.
pub(crate) struct CaptureSession {
    /// The session ends by itself when nothing has been pressed for a
    /// while, so a client that vanishes cannot leave the mouse captured.
    pub deadline: Instant,
    /// Everything captured so far, each once.
    pub seen: Vec<Trigger>,
    /// Captured but not yet collected by `PollCapture`.
    pub unreported: Vec<Trigger>,
}

/// Everything the threads share.
pub(crate) struct State {
    pub loaded: Loaded,
    /// One engine per `[[device]]` block, same order as `loaded.config.devices`.
    pub engines: Vec<Engine>,
    pub output: VirtualMouse,
    pub paused: bool,
    /// The profile the focused window calls for: an index into
    /// `loaded.profiles`, or `None` for the default profile.
    pub auto_profile: Option<usize>,
    /// A profile forced from the tray or CLI; `None` means automatic.
    pub override_profile: Option<Option<usize>>,
    /// The profile the engines are using right now.
    pub active_profile: Option<usize>,
    /// The most recently focused app windows, newest first, one per window
    /// class. `status` lists them so you can look up a class for a profile.
    pub recent: Vec<Window>,
    /// Application windows open right now, as the focus backend last
    /// reported them.
    pub windows: Vec<Window>,
    /// "kde", or why focus switching is off.
    pub focus_status: String,
    pub focus_backend: Option<Arc<dyn FocusBackend>>,
    pub capture: Option<CaptureSession>,
    /// Nodes that currently have a reader thread.
    pub active: HashSet<PathBuf>,
    /// Scratch buffer for engine output.
    pub out: Vec<Event>,
    /// Poked whenever something the tray shows has changed.
    pub tray_refresh: Option<Sender<()>>,
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

pub(crate) fn log(message: &str) {
    eprintln!("mouse-profiles: {message}");
}

/// Set by tests (`MOUSE_PROFILES_INSTANCE=test`) to run a second, isolated
/// daemon: its own bus name and virtual device, and no tray, focus backend
/// or autostart file. Never set in normal use.
pub(crate) fn instance() -> Option<String> {
    std::env::var(mouse_profiles_core::dbus::INSTANCE_VARIABLE)
        .ok()
        .filter(|name| !name.is_empty())
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

    // Claim the bus name before touching any device, so a second daemon
    // stops here instead of fighting the first one for the mouse.
    let connection = service::claim_name()?;

    let isolated = instance();
    let output_name = match &isolated {
        Some(name) => format!("{} ({name})", VirtualMouse::NAME),
        None => VirtualMouse::NAME.to_string(),
    };
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

    let focus_backend: Option<Arc<dyn FocusBackend>> = match isolated {
        Some(_) => None,
        None => mouse_profiles_focus::detect().map(Arc::from),
    };
    let focus_status = match &focus_backend {
        Some(backend) => backend.name().to_string(),
        None => "off (no backend for this desktop)".to_string(),
    };
    if focus_backend.is_none() {
        log("no focus backend; only the default profile is used unless you pick one by hand");
    }

    let mut state = State {
        loaded,
        engines,
        output,
        paused: false,
        auto_profile: None,
        override_profile: None,
        active_profile: None,
        recent: Vec::new(),
        windows: Vec::new(),
        focus_status,
        focus_backend: focus_backend.clone(),
        capture: None,
        active: HashSet::new(),
        out: Vec::new(),
        tray_refresh: None,
    };
    state.apply_profile();
    if isolated.is_none() {
        state.sync_autostart();
    }
    let shared: Shared = Arc::new(Mutex::new(state));

    service::serve(&connection, shared.clone())?;
    if isolated.is_none() {
        tray::start(shared.clone());
    }
    if let Some(backend) = focus_backend {
        let focus_state = shared.clone();
        thread::spawn(move || run_focus_backend(backend, focus_state));
    }

    watch_for_mice(shared)
}

/// Keeps the focus backend running, retrying while it cannot start.
fn run_focus_backend(backend: Arc<dyn FocusBackend>, shared: Shared) {
    let mut last_reason = String::new();
    loop {
        {
            let mut state = lock(&shared);
            state.focus_status = backend.name().to_string();
        }
        let callback_state = shared.clone();
        let result = backend.run(Box::new(move |event| {
            let mut state = lock(&callback_state);
            match event {
                FocusEvent::Focused(window) => state.window_focused(window),
                FocusEvent::Windows(windows) => state.windows = windows,
            }
        }));
        // run() only returns when the backend cannot work.
        let reason = result.err().unwrap_or_else(|| "backend stopped".into());
        if reason != last_reason {
            log(&format!(
                "focus switching is off: {reason}; retrying every {} s",
                FOCUS_RETRY.as_secs()
            ));
            last_reason = reason.clone();
        }
        {
            let mut state = lock(&shared);
            state.focus_status = format!("off ({reason})");
            state.auto_profile = None;
            state.apply_profile();
        }
        thread::sleep(FOCUS_RETRY);
    }
}

/// The main loop: once a second, start a reader for any configured mouse
/// node that does not have one. Covers startup, replugging, and a wireless
/// mouse waking up, all the same way.
fn watch_for_mice(shared: Shared) -> ! {
    // Remember the last error so a persistent problem (no udev rule) is
    // logged once, not every second.
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
