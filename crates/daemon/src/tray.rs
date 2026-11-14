//! The tray icon: shows the active profile, and offers a manual profile
//! choice, pause, and quit.
//!
//! The tray library runs the icon on its own thread and calls our menu
//! callbacks there. To keep locking simple the icon holds a plain copy of
//! what it displays (`View`); a small thread refreshes that copy whenever
//! the daemon's state changes.

use std::sync::mpsc;
use std::thread;

use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};

use crate::{Shared, State, actions, lock, log};

/// What the tray displays, copied out of the daemon state.
#[derive(Clone, PartialEq)]
struct View {
    paused: bool,
    active_profile: String,
    default_profile: String,
    profiles: Vec<String>,
    /// Which radio item is selected: 0 automatic, 1 the default profile,
    /// 2 and up the app profiles in order.
    selected: usize,
}

impl View {
    fn of(state: &State) -> View {
        View {
            paused: state.paused,
            active_profile: state.loaded.profile_name(state.active_profile).to_string(),
            default_profile: state.loaded.default.name.clone(),
            profiles: state
                .loaded
                .profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect(),
            selected: match state.override_profile {
                None => 0,
                Some(None) => 1,
                Some(Some(index)) => index + 2,
            },
        }
    }
}

struct Tray {
    shared: Shared,
    view: View,
}

/// Menu labels treat an underscore as "the next letter is a shortcut";
/// doubling it shows a real underscore.
fn label(text: &str) -> String {
    text.replace('_', "__")
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "mousekit".into()
    }

    fn title(&self) -> String {
        "MouseKit".into()
    }

    fn icon_name(&self) -> String {
        "input-mouse".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    /// Left-clicking the icon opens the settings window.
    fn activate(&mut self, _x: i32, _y: i32) {
        open_settings();
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "MouseKit".into(),
            description: if self.view.paused {
                "Remapping paused".into()
            } else {
                format!("Profile: {}", self.view.active_profile)
            },
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut options = vec![
            RadioItem {
                label: "Automatic (follow the focused window)".into(),
                ..Default::default()
            },
            RadioItem {
                label: label(&self.view.default_profile),
                ..Default::default()
            },
        ];
        options.extend(self.view.profiles.iter().map(|name| RadioItem {
            label: label(name),
            ..Default::default()
        }));

        vec![
            StandardItem {
                label: "Open settings".into(),
                activate: Box::new(|_: &mut Tray| open_settings()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: format!("Profile: {}", label(&self.view.active_profile)),
                enabled: false,
                ..Default::default()
            }
            .into(),
            SubMenu {
                label: "Switch profile".into(),
                submenu: vec![
                    RadioGroup {
                        selected: self.view.selected,
                        select: Box::new(|tray: &mut Tray, index| {
                            let name = match index {
                                0 => "auto".to_string(),
                                1 => tray.view.default_profile.clone(),
                                _ => match tray.view.profiles.get(index - 2) {
                                    Some(name) => name.clone(),
                                    None => return,
                                },
                            };
                            let mut state = lock(&tray.shared);
                            if let Err(error) = state.set_override(&name) {
                                log(&error);
                            }
                            tray.view = View::of(&state);
                        }),
                        options,
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Pause remapping".into(),
                checked: self.view.paused,
                activate: Box::new(|tray: &mut Tray| {
                    let mut state = lock(&tray.shared);
                    let paused = !state.paused;
                    state.set_paused(paused);
                    tray.view = View::of(&state);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|tray: &mut Tray| actions::quit(&tray.shared)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Starts the settings window as its own process (`mousekit gui`), so
/// the daemon never loads the GUI toolkit and a GUI crash cannot interrupt
/// remapping.
fn open_settings() {
    let started =
        std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("gui").spawn());
    match started {
        // Wait for it in the background so it does not linger as a zombie
        // process after its window is closed.
        Ok(mut child) => {
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => log(&format!("cannot open the settings window: {error}")),
    }
}

/// Shows the tray icon and keeps it up to date. If the desktop has no tray,
/// the daemon carries on without one.
pub(crate) fn start(shared: Shared) {
    let (refresh, refreshes) = mpsc::channel();
    let view = {
        let mut state = lock(&shared);
        state.tray_refresh = Some(refresh);
        View::of(&state)
    };
    let tray = Tray {
        shared: shared.clone(),
        view,
    };
    // At login the daemon can start before the desktop's tray does;
    // assume_sni_available makes the icon wait for it instead of failing.
    let handle = match tray.assume_sni_available(true).spawn() {
        Ok(handle) => handle,
        Err(error) => {
            log(&format!("no tray icon: {error}"));
            return;
        }
    };

    thread::spawn(move || {
        for () in refreshes {
            // Copy the state first and release its lock before touching the
            // tray, so this thread never holds both at once.
            let view = View::of(&lock(&shared));
            handle.update(|tray| tray.view = view);
        }
    });
}
