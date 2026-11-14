//! The settings window.
//!
//! It follows iced's model: one `App` value holds everything on screen,
//! `update` changes it in response to a `Message`, and `view` (in the
//! `view` module) draws it. The window owns no settings of its own. Every
//! change is sent to the daemon over D-Bus, and what is shown always comes
//! from the daemon's latest snapshot, refreshed a few times a second.

mod client;
mod keys;
mod view;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iced::keyboard::{self, key};
use iced::widget::combo_box;
use iced::{Subscription, Task};
use mousekit_core::{Binding, Key, Macro, MacroMode, Profile, Snapshot, Step, Trigger};

use crate::client::Client;
use crate::keys::{Chord, KeyChoice, Preset};

/// How often the window asks the daemon for its state.
const REFRESH: Duration = Duration::from_millis(300);

/// Opens the settings window and blocks until it is closed.
pub fn run() -> Result<(), String> {
    iced::application(App::new, App::update, view::view)
        .title("MouseKit")
        .subscription(App::subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(980.0, 680.0),
            // Matches the name of the installed .desktop file, which is how
            // the desktop finds the window's icon and name.
            platform_specific: iced::window::settings::PlatformSpecific {
                application_id: mousekit_core::dbus::APP_ID.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .run()
        .map_err(|error| format!("cannot open the settings window: {error}"))
}

/// Which page the main area shows.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Page {
    /// A mouse, by its position in the config.
    Device(usize),
    /// The list of plugged-in mice that can be added.
    AddMouse,
    /// A profile, by name.
    Profile(String),
    /// A macro, by name.
    Macro(String),
}

/// The kinds of binding the editor offers for one control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// No entry in this profile: the default profile's binding applies, or
    /// in the default profile itself, the button behaves normally.
    Unset,
    /// Explicitly normal, overriding the default profile.
    Passthrough,
    Keys,
    MouseButton,
    Disabled,
    Macro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Up,
    Down,
}

#[derive(Debug, Clone)]
pub(crate) enum Message {
    Tick,
    Open(Page),
    DismissError,
    SetPaused(bool),

    // Device page.
    StartCapture,
    StopCapture,
    MoveControl(String, Direction),
    AddMouse(String, String),
    AskRemoveDevice,
    ConfirmRemoveDevice,
    DpiDraftChanged(String),
    ApplyDpiDraft,
    SetDpi(u32),
    SetPollingHz(u32),
    BeginRename(String),
    RenameChanged(String),
    CommitRename,
    CancelRename,
    RemoveControl(String),

    // Profile list.
    NewProfileNameChanged(String),
    CreateProfile,

    // Profile page.
    ProfileNameChanged(String),
    RenameProfile,
    DuplicateProfile,
    AskDeleteProfile,
    ConfirmDeleteProfile,
    ToggleWindowPicker,
    AddWindowClass(String),
    ClassDraftChanged(String),
    AddClassDraft,
    RemoveWindowClass(String),
    SetKind(String, Kind),
    EditKeys(String),
    DoneEditingKeys,
    ToggleModifier(usize, bool),
    SetMainKey(KeyChoice),
    ApplyPreset(Preset),
    SetMouseButton(String, KeyChoice),
    SetMacro(String, String),
    SetMacroMode(String, MacroMode),

    // Macros.
    CreateMacro,
    MacroNameChanged(String),
    RenameMacro,
    AskDeleteMacro,
    ConfirmDeleteMacro,
    StartRecording,
    StopRecording,
    Keyboard(keyboard::Event),
    RemoveStep(usize),
    MoveStep(usize, Direction),
    StepDelayChanged(usize, String),
    AddDelay,
    AddKeyTap(KeyChoice),
    DelayAllChanged(String),
    SetAllDelays,
}

/// A macro being recorded from the keyboard.
pub(crate) struct Recording {
    pub steps: Vec<Step>,
    /// When the last key event arrived, to measure the pause before the next.
    last_event: Option<Instant>,
}

pub(crate) struct App {
    client: Result<Client, String>,
    pub snapshot: Option<Snapshot>,
    /// Why the daemon could not be reached, when it cannot.
    pub offline: Option<String>,
    /// True once this window has started the daemon itself, so it only
    /// tries once.
    pub started_daemon: bool,
    /// True once the first snapshot has been used to pick a starting page.
    chose_first_page: bool,
    /// The last action that failed, until dismissed.
    pub error: Option<String>,
    pub page: Page,

    /// True while a capture session is open on the device page.
    pub capturing: bool,
    /// What the current (or last) capture session did, one line per button.
    pub capture_log: Vec<String>,
    /// Control being renamed on the device page: (current label, new text).
    pub rename: Option<(String, String)>,

    /// Text being typed into the DPI box on the device page.
    pub dpi_draft: String,

    pub new_profile_name: String,
    pub profile_name_draft: String,
    pub confirm_delete: bool,
    pub show_window_picker: bool,
    pub class_draft: String,
    /// Label of the control whose shortcut editor is open.
    pub editing_keys: Option<String>,
    pub key_choices: combo_box::State<KeyChoice>,

    pub macro_name_draft: String,
    pub recording: Option<Recording>,
    /// Text being typed into a step's delay box, by step number.
    pub delay_drafts: HashMap<usize, String>,
    pub delay_all_draft: String,
}

impl App {
    fn new() -> App {
        let mut app = App {
            client: Client::connect(),
            snapshot: None,
            offline: None,
            started_daemon: false,
            chose_first_page: false,
            error: None,
            page: Page::Device(0),
            capturing: false,
            capture_log: Vec::new(),
            rename: None,
            dpi_draft: String::new(),
            new_profile_name: String::new(),
            profile_name_draft: String::new(),
            confirm_delete: false,
            show_window_picker: false,
            class_draft: String::new(),
            editing_keys: None,
            key_choices: combo_box::State::new(keys::main_key_choices()),
            macro_name_draft: String::new(),
            recording: None,
            delay_drafts: HashMap::new(),
            delay_all_draft: String::new(),
        };
        app.refresh();
        app
    }

    /// Fetches the daemon's state. Called on a timer and after every change.
    fn refresh(&mut self) {
        let result = match &self.client {
            Ok(client) => client.snapshot(),
            Err(error) => Err(error.clone()),
        };
        match result {
            Ok(snapshot) => {
                self.offline = None;
                // If the page's profile vanished (deleted or renamed
                // elsewhere), fall back to the default profile.
                let gone = match &self.page {
                    Page::Profile(name) => profile_in(&snapshot, name).is_none(),
                    Page::Macro(name) => !snapshot.macros.iter().any(|m| m.name == *name),
                    // A mouse page may briefly be one past the end right after
                    // adding a mouse, while the daemon restarts.
                    Page::Device(index) => *index > snapshot.devices.len(),
                    Page::AddMouse => false,
                };
                // With no mouse set up there is only one useful place to be.
                let first_page = (!self.chose_first_page && snapshot.devices.is_empty())
                    .then_some(Page::AddMouse);
                self.chose_first_page = true;
                if let Some(page) = first_page {
                    self.open(page);
                } else if gone {
                    self.open(match snapshot.devices.is_empty() {
                        true => Page::AddMouse,
                        false => Page::Device(0),
                    });
                }
                self.snapshot = Some(snapshot);
            }
            Err(error) => {
                // Opening the settings window is how most people first run
                // the app, so start the daemon if nobody has. It carries on
                // after this window closes, and sets up its own autostart.
                if !self.started_daemon && self.snapshot.is_none() && error.contains("not running")
                {
                    self.started_daemon = true;
                    let started = std::env::current_exe().and_then(|exe| {
                        std::process::Command::new(exe)
                            .arg("daemon")
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .spawn()
                    });
                    if let Err(start_error) = started {
                        self.error = Some(format!("cannot start the daemon: {start_error}"));
                    }
                }
                self.offline = Some(error);
                // The connection may have been made before the daemon
                // started; try again from scratch next time.
                if self.client.is_err() {
                    self.client = Client::connect();
                }
            }
        }
    }

    /// Runs one daemon call, shows its error if it fails, and refreshes.
    fn act(&mut self, action: impl FnOnce(&Client) -> Result<(), String>) -> bool {
        let result = match &self.client {
            Ok(client) => action(client),
            Err(error) => Err(error.clone()),
        };
        let ok = result.is_ok();
        self.error = result.err();
        self.refresh();
        ok
    }

    fn open(&mut self, page: Page) {
        self.stop_capture();
        self.capture_log.clear();
        self.rename = None;
        self.confirm_delete = false;
        self.show_window_picker = false;
        self.class_draft.clear();
        self.editing_keys = None;
        self.dpi_draft.clear();
        self.recording = None;
        self.delay_drafts.clear();
        self.delay_all_draft.clear();
        self.profile_name_draft = match &page {
            Page::Profile(name) => name.clone(),
            _ => String::new(),
        };
        self.macro_name_draft = match &page {
            Page::Macro(name) => name.clone(),
            _ => String::new(),
        };
        self.page = page;
    }

    /// The profile the current page shows, if it is a profile page.
    pub fn current_profile(&self) -> Option<&Profile> {
        let Page::Profile(name) = &self.page else {
            return None;
        };
        profile_in(self.snapshot.as_ref()?, name)
    }

    pub fn is_default(&self, profile: &Profile) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.default_profile.name == profile.name)
    }

    /// Applies a change to the current profile and saves it.
    fn edit_profile(&mut self, change: impl FnOnce(&mut Profile)) {
        let Some(mut profile) = self.current_profile().cloned() else {
            return;
        };
        let original = profile.name.clone();
        change(&mut profile);
        let renamed_to = profile.name.clone();
        if self.act(|client| client.save_profile(&original, &profile)) && renamed_to != original {
            self.page = Page::Profile(renamed_to.clone());
            self.profile_name_draft = renamed_to;
        }
    }

    /// The macro the current page shows, if it is a macro page.
    pub fn current_macro(&self) -> Option<&Macro> {
        let Page::Macro(name) = &self.page else {
            return None;
        };
        self.snapshot
            .as_ref()?
            .macros
            .iter()
            .find(|candidate| candidate.name == *name)
    }

    /// Applies a change to the current macro and saves it.
    fn edit_macro(&mut self, change: impl FnOnce(&mut Macro)) {
        let Some(mut edited) = self.current_macro().cloned() else {
            return;
        };
        let original = edited.name.clone();
        change(&mut edited);
        let renamed_to = edited.name.clone();
        if self.act(|client| client.save_macro(&original, &edited)) && renamed_to != original {
            self.page = Page::Macro(renamed_to.clone());
            self.macro_name_draft = renamed_to;
        }
    }

    /// Adds one keyboard event to the recording in progress.
    fn record(&mut self, event: keyboard::Event) {
        let Some(recording) = &mut self.recording else {
            return;
        };
        let (physical, pressed) = match event {
            // Holding a key makes the keyboard repeat it; record it once.
            keyboard::Event::KeyPressed { repeat: true, .. } => return,
            keyboard::Event::KeyPressed { physical_key, .. } => (physical_key, true),
            keyboard::Event::KeyReleased { physical_key, .. } => (physical_key, false),
            keyboard::Event::ModifiersChanged(_) => return,
        };
        let key::Physical::Code(code) = physical else {
            return;
        };
        let Some(key) = keys::from_physical(code) else {
            return;
        };
        // A release with no press before it belongs to something typed
        // before recording started.
        let already_down = recording.steps.iter().fold(false, |down, step| match step {
            Step::KeyDown { key: k } if *k == key => true,
            Step::KeyUp { key: k } if *k == key => false,
            _ => down,
        });
        if pressed == already_down {
            return;
        }
        if recording.steps.len() + 2 > mousekit_core::macros::MAX_STEPS {
            return;
        }
        let now = Instant::now();
        if let Some(last) = recording.last_event {
            let ms = now.duration_since(last).as_millis();
            let ms = ms.min(u128::from(mousekit_core::macros::MAX_DELAY_MS)) as u32;
            if ms > 0 {
                recording.steps.push(Step::Delay { ms });
            }
        }
        recording.last_event = Some(now);
        recording.steps.push(if pressed {
            Step::KeyDown { key }
        } else {
            Step::KeyUp { key }
        });
    }

    /// Escape backs out of whatever is open, capture first.
    fn escape(&mut self) {
        if self.capturing {
            self.stop_capture();
        } else if self.rename.is_some() {
            self.rename = None;
        } else if self.editing_keys.is_some() {
            self.editing_keys = None;
        }
    }

    /// The keys of the binding whose shortcut editor is open.
    pub fn editing_chord(&self) -> Option<Chord> {
        let label = self.editing_keys.as_ref()?;
        match self.current_profile()?.bindings.get(label)? {
            Binding::Key { keys } => Some(Chord::from_keys(keys)),
            _ => None,
        }
    }

    fn set_editing_chord(&mut self, change: impl FnOnce(&mut Chord)) {
        let (Some(label), Some(mut chord)) = (self.editing_keys.clone(), self.editing_chord())
        else {
            return;
        };
        change(&mut chord);
        // Unticking the last thing would leave an empty shortcut; ignore it.
        if let Some(keys) = chord.to_keys() {
            self.edit_profile(|profile| {
                profile.bindings.insert(label, Binding::Key { keys });
            });
        }
    }

    /// What the window listens to besides its own widgets: a timer for
    /// refreshing, and the keyboard (for Escape and for recording macros).
    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::time::every(REFRESH).map(|_| Message::Tick),
            keyboard::listen().map(Message::Keyboard),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                self.refresh();
                if self.capturing {
                    self.poll_capture();
                }
            }
            Message::Open(page) => self.open(page),
            Message::DismissError => self.error = None,
            Message::SetPaused(paused) => {
                self.act(|client| client.set_paused(paused));
            }

            Message::StartCapture => {
                if self.act(Client::start_capture) {
                    self.capturing = true;
                    self.capture_log.clear();
                    self.rename = None;
                }
            }
            Message::StopCapture => self.stop_capture(),
            Message::AddMouse(id, name) => {
                if self.act(|client| client.add_device(&id, &name)) {
                    // The daemon restarts to pick the mouse up; the new
                    // mouse is the last one in the list when it is back.
                    let position = self.snapshot.as_ref().map_or(0, |s| s.devices.len());
                    self.open(Page::Device(position));
                }
            }
            Message::AskRemoveDevice => self.confirm_delete = true,
            Message::ConfirmRemoveDevice => {
                if let Page::Device(index) = self.page {
                    self.confirm_delete = false;
                    if self.act(|client| client.remove_device(index)) {
                        self.open(Page::AddMouse);
                    }
                }
            }
            Message::DpiDraftChanged(text) => self.dpi_draft = text,
            Message::ApplyDpiDraft => {
                // Snap what was typed to the nearest value the sensor has.
                let wanted = self.dpi_draft.trim().parse::<u32>().ok();
                let nearest = match (&self.page, &self.snapshot, wanted) {
                    (Page::Device(index), Some(snapshot), Some(wanted)) => snapshot
                        .devices
                        .get(*index)
                        .and_then(|device| device.hardware.as_ref())
                        .and_then(|hardware| hardware.nearest_dpi(wanted)),
                    _ => None,
                };
                if let Some(dpi) = nearest {
                    return self.update(Message::SetDpi(dpi));
                }
            }
            Message::SetDpi(dpi) => {
                if let Page::Device(index) = self.page
                    && self.act(|client| client.set_dpi(index, dpi))
                {
                    self.dpi_draft.clear();
                }
            }
            Message::SetPollingHz(hz) => {
                if let Page::Device(index) = self.page {
                    self.act(|client| client.set_polling_hz(index, hz));
                }
            }
            Message::MoveControl(label, direction) => {
                if let Page::Device(index) = self.page
                    && let Some(mut device) = self.device(index)
                    && let Some(from) = device.controls.iter().position(|c| c.label == label)
                {
                    let to = match direction {
                        Direction::Up => from.checked_sub(1),
                        Direction::Down => Some(from + 1).filter(|to| *to < device.controls.len()),
                    };
                    if let Some(to) = to {
                        device.controls.swap(from, to);
                        self.act(|client| client.save_device(index, &device));
                    }
                }
            }
            Message::BeginRename(label) => self.rename = Some((label.clone(), label)),
            Message::RenameChanged(text) => {
                if let Some((_, new)) = &mut self.rename {
                    *new = text;
                }
            }
            Message::CancelRename => self.rename = None,
            Message::CommitRename => {
                if let (Page::Device(index), Some((old, new))) = (&self.page, self.rename.take()) {
                    let index = *index;
                    if old != new.trim() {
                        self.act(|client| client.rename_control(index, &old, &new));
                    }
                }
            }
            Message::RemoveControl(label) => {
                if let Page::Device(index) = self.page
                    && let Some(mut device) = self.device(index)
                {
                    device.controls.retain(|control| control.label != label);
                    self.act(|client| client.save_device(index, &device));
                }
            }

            Message::NewProfileNameChanged(text) => self.new_profile_name = text,
            Message::CreateProfile => {
                let name = self.new_profile_name.trim().to_string();
                if !name.is_empty() {
                    let profile = empty_profile(&name);
                    if self.act(|client| client.save_profile("", &profile)) {
                        self.new_profile_name.clear();
                        self.open(Page::Profile(name));
                        // A new profile does nothing until it knows which
                        // app it is for, so offer the picker straight away.
                        self.show_window_picker = true;
                    }
                }
            }

            Message::ProfileNameChanged(text) => self.profile_name_draft = text,
            Message::RenameProfile => {
                let name = self.profile_name_draft.trim().to_string();
                if !name.is_empty() {
                    self.edit_profile(|profile| profile.name = name);
                }
            }
            Message::DuplicateProfile => {
                if let (Some(profile), Some(snapshot)) = (self.current_profile(), &self.snapshot) {
                    let mut copy = profile.clone();
                    copy.name = unused_name(snapshot, &format!("{} copy", profile.name));
                    // Two profiles cannot claim the same app, so the copy
                    // starts with no apps.
                    copy.matches = Default::default();
                    let name = copy.name.clone();
                    if self.act(|client| client.save_profile("", &copy)) {
                        self.open(Page::Profile(name));
                    }
                }
            }
            Message::AskDeleteProfile => self.confirm_delete = true,
            Message::ConfirmDeleteProfile => {
                if let Some(name) = self.current_profile().map(|profile| profile.name.clone()) {
                    self.act(|client| client.delete_profile(&name));
                    self.confirm_delete = false;
                }
            }
            Message::ToggleWindowPicker => self.show_window_picker = !self.show_window_picker,
            Message::AddWindowClass(class) => {
                self.edit_profile(|profile| {
                    if !profile.matches.window_class.contains(&class) {
                        profile.matches.window_class.push(class);
                    }
                });
            }
            Message::ClassDraftChanged(text) => self.class_draft = text,
            Message::AddClassDraft => {
                let class = self.class_draft.trim().to_string();
                if !class.is_empty() {
                    self.class_draft.clear();
                    return self.update(Message::AddWindowClass(class));
                }
            }
            Message::RemoveWindowClass(class) => {
                self.edit_profile(|profile| profile.matches.window_class.retain(|c| *c != class));
            }
            Message::SetKind(label, kind) => self.set_kind(label, kind),
            Message::EditKeys(label) => self.editing_keys = Some(label),
            Message::DoneEditingKeys => self.editing_keys = None,
            Message::ToggleModifier(index, on) => {
                self.set_editing_chord(|chord| chord.modifiers[index] = on);
            }
            Message::SetMainKey(choice) => {
                self.set_editing_chord(|chord| chord.main = Some(choice.key));
            }
            Message::ApplyPreset(preset) => {
                if let Some(label) = self.editing_keys.clone() {
                    self.edit_profile(|profile| {
                        profile
                            .bindings
                            .insert(label, Binding::Key { keys: preset.keys });
                    });
                }
            }
            Message::SetMacro(label, name) => {
                self.edit_profile(|profile| {
                    let mode = match profile.bindings.get(&label) {
                        Some(Binding::Macro { mode, .. }) => *mode,
                        _ => MacroMode::Once,
                    };
                    profile
                        .bindings
                        .insert(label, Binding::Macro { name, mode });
                });
            }
            Message::SetMacroMode(label, mode) => {
                self.edit_profile(|profile| {
                    if let Some(Binding::Macro { mode: current, .. }) =
                        profile.bindings.get_mut(&label)
                    {
                        *current = mode;
                    }
                });
            }

            Message::CreateMacro => {
                if let Some(snapshot) = &self.snapshot {
                    let name = (1..)
                        .map(|n| format!("Macro {n}"))
                        .find(|name| {
                            !snapshot
                                .macros
                                .iter()
                                .any(|m| m.name.eq_ignore_ascii_case(name))
                        })
                        .expect("an unused number always exists");
                    let new = Macro {
                        name: name.clone(),
                        steps: Vec::new(),
                    };
                    if self.act(|client| client.save_macro("", &new)) {
                        self.open(Page::Macro(name));
                    }
                }
            }
            Message::MacroNameChanged(text) => self.macro_name_draft = text,
            Message::RenameMacro => {
                let name = self.macro_name_draft.trim().to_string();
                if !name.is_empty() {
                    self.edit_macro(|edited| edited.name = name);
                }
            }
            Message::AskDeleteMacro => self.confirm_delete = true,
            Message::ConfirmDeleteMacro => {
                if let Some(name) = self.current_macro().map(|m| m.name.clone()) {
                    self.act(|client| client.delete_macro(&name));
                    self.confirm_delete = false;
                }
            }
            Message::StartRecording => {
                self.delay_drafts.clear();
                self.recording = Some(Recording {
                    steps: Vec::new(),
                    last_event: None,
                });
            }
            Message::StopRecording => {
                if let Some(recording) = self.recording.take() {
                    self.edit_macro(|edited| edited.steps = recording.steps);
                }
            }
            Message::Keyboard(event) => {
                if self.recording.is_some() {
                    self.record(event);
                } else if let keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(key::Named::Escape),
                    ..
                } = event
                {
                    self.escape();
                }
            }
            Message::RemoveStep(index) => {
                self.delay_drafts.clear();
                self.edit_macro(|edited| {
                    if index < edited.steps.len() {
                        edited.steps.remove(index);
                    }
                });
            }
            Message::MoveStep(index, direction) => {
                self.delay_drafts.clear();
                self.edit_macro(|edited| {
                    let to = match direction {
                        Direction::Up => index.checked_sub(1),
                        Direction::Down => Some(index + 1).filter(|to| *to < edited.steps.len()),
                    };
                    if let Some(to) = to
                        && index < edited.steps.len()
                    {
                        edited.steps.swap(index, to);
                    }
                });
            }
            Message::StepDelayChanged(index, text) => {
                // Keep what is being typed even while it is not yet a
                // number; save as soon as it is one.
                let parsed = text.trim().parse::<u32>().ok();
                self.delay_drafts.insert(index, text);
                if let Some(ms) = parsed {
                    self.edit_macro(|edited| {
                        if let Some(Step::Delay { ms: current }) = edited.steps.get_mut(index) {
                            *current = ms;
                        }
                    });
                }
            }
            Message::AddDelay => {
                self.edit_macro(|edited| edited.steps.push(Step::Delay { ms: 50 }));
            }
            Message::AddKeyTap(choice) => {
                self.edit_macro(|edited| {
                    edited.steps.push(Step::KeyDown { key: choice.key });
                    edited.steps.push(Step::Delay { ms: 30 });
                    edited.steps.push(Step::KeyUp { key: choice.key });
                });
            }
            Message::DelayAllChanged(text) => self.delay_all_draft = text,
            Message::SetAllDelays => {
                if let Ok(ms) = self.delay_all_draft.trim().parse::<u32>() {
                    self.delay_drafts.clear();
                    self.edit_macro(|edited| {
                        for step in &mut edited.steps {
                            if let Step::Delay { ms: current } = step {
                                *current = ms;
                            }
                        }
                    });
                }
            }
            Message::SetMouseButton(label, choice) => {
                self.edit_profile(|profile| {
                    profile
                        .bindings
                        .insert(label, Binding::MouseButton { button: choice.key });
                });
            }
        }
        Task::none()
    }

    fn device(&self, index: usize) -> Option<mousekit_core::DeviceConfig> {
        Some(self.snapshot.as_ref()?.devices.get(index)?.config.clone())
    }

    /// Ends the capture session, if one is open.
    fn stop_capture(&mut self) {
        if self.capturing {
            // Collect anything pressed since the last tick before closing.
            self.poll_capture();
            self.capturing = false;
            if let Ok(client) = &self.client {
                let _ = client.cancel_capture();
            }
        }
    }

    /// Collects newly captured buttons from the daemon and adds each one
    /// that is not already in the list, under a suggested name.
    fn poll_capture(&mut self) {
        let Ok(client) = &self.client else { return };
        let (open, events) = match client.poll_capture() {
            Ok(result) => result,
            Err(error) => {
                self.capturing = false;
                self.error = Some(error);
                return;
            }
        };
        let Page::Device(index) = self.page else {
            return;
        };

        if let Some(mut device) = self.device(index) {
            let mut added = false;
            for event in events {
                let Ok(trigger) = event.parse::<Trigger>() else {
                    self.capture_log.push(format!(
                        "Skipped an event this version does not know: {event}"
                    ));
                    continue;
                };
                let what = keys::describe_trigger(&trigger);
                if let Some(existing) = device.controls.iter().find(|c| c.event == trigger) {
                    self.capture_log.push(format!(
                        "{what} is already in the list as \"{}\"",
                        existing.label
                    ));
                    continue;
                }
                let taken: Vec<&str> = device.controls.iter().map(|c| c.label.as_str()).collect();
                let label = keys::suggest_label(&trigger, &taken);
                self.capture_log
                    .push(format!("Added {what} as \"{label}\""));
                device.controls.push(mousekit_core::Control {
                    label,
                    event: trigger,
                });
                added = true;
            }
            if added {
                self.act(|client| client.save_device(index, &device));
            }
        }

        if !open && self.capturing {
            self.capturing = false;
            self.capture_log
                .push("Capture stopped by itself after 30 seconds without a press.".into());
        }
    }

    fn set_kind(&mut self, label: String, kind: Kind) {
        let previous = self
            .current_profile()
            .and_then(|profile| profile.bindings.get(&label).cloned());
        let binding = match kind {
            Kind::Unset => None,
            Kind::Passthrough => Some(Binding::Passthrough),
            Kind::Disabled => Some(Binding::Disabled),
            // Keep what is there when re-selecting the same kind.
            Kind::Keys => Some(match previous {
                Some(binding @ Binding::Key { .. }) => binding,
                _ => Binding::Key {
                    keys: vec![Key::from_name("KEY_KP1").expect("KEY_KP1 exists")],
                },
            }),
            Kind::MouseButton => Some(match previous {
                Some(binding @ Binding::MouseButton { .. }) => binding,
                _ => Binding::MouseButton {
                    button: Key::from_name("BTN_MIDDLE").expect("BTN_MIDDLE exists"),
                },
            }),
            Kind::Macro => Some(match previous {
                Some(binding @ Binding::Macro { .. }) => binding,
                _ => {
                    let Some(first) = self
                        .snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.macros.first())
                    else {
                        return;
                    };
                    Binding::Macro {
                        name: first.name.clone(),
                        mode: MacroMode::Once,
                    }
                }
            }),
        };
        self.editing_keys = (kind == Kind::Keys).then(|| label.clone());
        self.edit_profile(|profile| {
            match binding {
                Some(binding) => profile.bindings.insert(label, binding),
                None => profile.bindings.remove(&label),
            };
        });
    }
}

fn profile_in<'a>(snapshot: &'a Snapshot, name: &str) -> Option<&'a Profile> {
    std::iter::once(&snapshot.default_profile)
        .chain(&snapshot.profiles)
        .find(|profile| profile.name == name)
}

fn empty_profile(name: &str) -> Profile {
    Profile {
        name: name.to_string(),
        matches: Default::default(),
        bindings: Default::default(),
    }
}

/// `wanted`, or `wanted 2`, `wanted 3`... if a profile already has the name.
fn unused_name(snapshot: &Snapshot, wanted: &str) -> String {
    let taken = |name: &str| {
        std::iter::once(&snapshot.default_profile)
            .chain(&snapshot.profiles)
            .any(|profile| profile.name.eq_ignore_ascii_case(name))
    };
    if !taken(wanted) {
        return wanted.to_string();
    }
    (2..)
        .map(|n| format!("{wanted} {n}"))
        .find(|name| !taken(name))
        .expect("an unused number always exists")
}

/// The kind of an existing binding (or of having none).
pub(crate) fn kind_of(binding: Option<&Binding>) -> Kind {
    match binding {
        None => Kind::Unset,
        Some(Binding::Passthrough) => Kind::Passthrough,
        Some(Binding::Key { .. }) => Kind::Keys,
        Some(Binding::MouseButton { .. }) => Kind::MouseButton,
        Some(Binding::Disabled) => Kind::Disabled,
        Some(Binding::Macro { .. }) => Kind::Macro,
    }
}
