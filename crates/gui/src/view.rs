//! Drawing the window. Nothing here changes state: each function turns part
//! of the `App` into widgets, and widgets send `Message`s back to `update`.

use std::fmt;

use iced::widget::{
    button, checkbox, column, combo_box, container, pick_list, row, rule, scrollable, space, text,
    text_input,
};
use iced::{Alignment, Element, Length};
use mouse_profiles_core::{
    Binding, DeviceSnapshot, Key, Macro, MacroMode, Profile, Snapshot, Step, Trigger,
};

use crate::keys::{self, KeyChoice};
use crate::{App, Direction, Kind, Message, Page, kind_of};

const SIDEBAR_WIDTH: f32 = 230.0;
const LABEL_WIDTH: f32 = 150.0;
const KIND_WIDTH: f32 = 210.0;

pub(crate) fn view(app: &App) -> Element<'_, Message> {
    let Some(snapshot) = &app.snapshot else {
        let reason = app
            .offline
            .as_deref()
            .unwrap_or("Connecting to the daemon...");
        return container(
            column![
                text("Mouse Profiles").size(26),
                text(reason),
                text("Start it with:  mouse-profiles daemon").style(text::secondary),
            ]
            .spacing(12),
        )
        .center(Length::Fill)
        .into();
    };

    let mut main = column![].spacing(16).padding(20);
    if let Some(offline) = &app.offline {
        main =
            main.push(text(format!("Lost contact with the daemon: {offline}")).style(text::danger));
    }
    if let Some(error) = &app.error {
        main = main.push(
            row![
                text(error).style(text::danger).width(Length::Fill),
                button("Dismiss")
                    .style(button::secondary)
                    .on_press(Message::DismissError),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        );
    }
    main = main.push(match &app.page {
        Page::Device(index) => match snapshot.devices.get(*index) {
            Some(device) => device_page(app, device),
            None => text("This mouse is no longer in the config.").into(),
        },
        Page::Profile(_) => match app.current_profile() {
            Some(profile) => profile_page(app, snapshot, profile),
            None => text("This profile no longer exists.").into(),
        },
        Page::Macro(_) => match app.current_macro() {
            Some(current) => macro_page(app, snapshot, current),
            None => text("This macro no longer exists.").into(),
        },
    });

    row![
        sidebar(app, snapshot),
        rule::vertical(1),
        scrollable(main).width(Length::Fill).height(Length::Fill),
    ]
    .into()
}

fn heading(label: &str) -> Element<'_, Message> {
    text(label).size(18).into()
}

fn hint(label: impl text::IntoFragment<'static>) -> Element<'static, Message> {
    text(label).size(13).style(text::secondary).into()
}

fn nav_button<'a>(label: String, page: Page, current: &Page) -> Element<'a, Message> {
    let selected = page == *current;
    button(text(label))
        .width(Length::Fill)
        .style(if selected {
            button::primary
        } else {
            button::text
        })
        .on_press(Message::Open(page))
        .into()
}

fn sidebar<'a>(app: &'a App, snapshot: &'a Snapshot) -> Element<'a, Message> {
    let mut list = column![hint("MOUSE")].spacing(4);
    for (index, device) in snapshot.devices.iter().enumerate() {
        list = list.push(nav_button(
            device.config.name.clone(),
            Page::Device(index),
            &app.page,
        ));
    }
    if snapshot.devices.is_empty() {
        list = list.push(hint("No mouse in config.toml"));
    }

    list = list.push(space().height(12)).push(hint("PROFILES"));
    let profiles = std::iter::once(&snapshot.default_profile).chain(&snapshot.profiles);
    for profile in profiles {
        let active = profile.name == snapshot.active_profile;
        let label = if active {
            format!("{}  (active)", profile.name)
        } else {
            profile.name.clone()
        };
        list = list.push(nav_button(
            label,
            Page::Profile(profile.name.clone()),
            &app.page,
        ));
    }

    list = list.push(space().height(12)).push(hint("MACROS"));
    for one in &snapshot.macros {
        list = list.push(nav_button(
            one.name.clone(),
            Page::Macro(one.name.clone()),
            &app.page,
        ));
    }
    list = list.push(
        button("Add macro")
            .width(Length::Fill)
            .style(button::secondary)
            .on_press(Message::CreateMacro),
    );

    let new_profile = column![
        text_input("New profile name", &app.new_profile_name)
            .on_input(Message::NewProfileNameChanged)
            .on_submit(Message::CreateProfile),
        button("Add profile")
            .width(Length::Fill)
            .style(button::secondary)
            .on_press_maybe(
                (!app.new_profile_name.trim().is_empty()).then_some(Message::CreateProfile)
            ),
    ]
    .spacing(6);

    let status = column![
        rule::horizontal(1),
        checkbox(snapshot.paused)
            .label("Pause remapping")
            .on_toggle(Message::SetPaused),
        hint(if snapshot.override_profile.is_empty() {
            format!("Switching: automatic ({})", snapshot.focus_status)
        } else {
            "Switching: chosen by hand in the tray".to_string()
        }),
    ]
    .spacing(8);

    container(column![scrollable(list).height(Length::Fill), new_profile, status].spacing(14))
        .width(SIDEBAR_WIDTH)
        .padding(14)
        .into()
}

// ---------------------------------------------------------------- device page

fn device_page<'a>(app: &'a App, device: &'a DeviceSnapshot) -> Element<'a, Message> {
    let config = &device.config;
    let ids: Vec<String> = config.ids.iter().map(|id| id.to_string()).collect();
    let mut page = column![
        text(&config.name).size(26),
        if device.connected {
            text("Connected").style(text::success)
        } else {
            text("Not connected").style(text::secondary)
        },
        hint(format!("Device ID: {}", ids.join(", "))),
        space().height(4),
        heading("Buttons"),
        hint(
            "Give each button you want to remap a name here. Profiles then say what each \
             named button does."
        ),
    ]
    .spacing(8);

    for (position, control) in config.controls.iter().enumerate() {
        let renaming = app.rename.as_ref().filter(|(old, _)| *old == control.label);
        let what = keys::describe_trigger(&control.event);
        let detail = hint(format!("{what}  ({})", control.event));
        let line: Element<'_, Message> = match renaming {
            Some((_, draft)) => row![
                text_input("Name", draft)
                    .on_input(Message::RenameChanged)
                    .on_submit(Message::CommitRename)
                    .width(LABEL_WIDTH),
                button("Save").on_press(Message::CommitRename),
                button("Cancel")
                    .style(button::secondary)
                    .on_press(Message::CancelRename),
                detail,
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
            None => row![
                button("Up").style(button::secondary).on_press_maybe(
                    (position > 0)
                        .then(|| Message::MoveControl(control.label.clone(), Direction::Up))
                ),
                button("Down").style(button::secondary).on_press_maybe(
                    (position + 1 < config.controls.len())
                        .then(|| { Message::MoveControl(control.label.clone(), Direction::Down) })
                ),
                text(&control.label).width(LABEL_WIDTH),
                container(detail).width(Length::Fill),
                button("Rename")
                    .style(button::secondary)
                    .on_press(Message::BeginRename(control.label.clone())),
                button("Remove")
                    .style(button::danger)
                    .on_press(Message::RemoveControl(control.label.clone())),
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .into(),
        };
        page = page.push(line);
    }
    if config.controls.is_empty() {
        page = page.push(text("No buttons named yet."));
    }

    page = page.push(space().height(8)).push(heading("Add buttons"));
    let mut capture = column![].spacing(6);
    if app.capturing {
        capture = capture
            .push(text(
                "Capturing: press each button on the mouse you want to add, and scroll the \
                 wheel if you want to remap scrolling.",
            ))
            .push(
                row![
                    button("Stop capturing").on_press(Message::StopCapture),
                    hint(
                        "or press Esc. Left click still works. Stops by itself after 30 \
                         seconds without a press."
                    ),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
    } else {
        capture = capture
            .push(
                button("Capture buttons")
                    .on_press_maybe(device.connected.then_some(Message::StartCapture)),
            )
            .push(hint(if device.connected {
                "Click this, then press every button you want to add. Each new one is added \
                 to the list with a suggested name you can change afterwards."
            } else {
                "Connect the mouse to capture its buttons."
            }));
    }
    for line in &app.capture_log {
        capture = capture.push(text(line).size(14));
    }
    page.push(capture).into()
}

// --------------------------------------------------------------- profile page

fn profile_page<'a>(
    app: &'a App,
    snapshot: &'a Snapshot,
    profile: &'a Profile,
) -> Element<'a, Message> {
    let is_default = app.is_default(profile);
    let mut page = column![text(&profile.name).size(26)].spacing(10);

    if is_default {
        page = page.push(hint(
            "Used whenever no other profile matches the focused window. Other profiles \
             start from these bindings and change only what they list.",
        ));
    } else {
        let renamed = app.profile_name_draft.trim() != profile.name
            && !app.profile_name_draft.trim().is_empty();
        let delete: Element<'_, Message> = if app.confirm_delete {
            button("Really delete?")
                .style(button::danger)
                .on_press(Message::ConfirmDeleteProfile)
                .into()
        } else {
            button("Delete")
                .style(button::danger)
                .on_press(Message::AskDeleteProfile)
                .into()
        };
        page = page.push(
            row![
                text_input("Profile name", &app.profile_name_draft)
                    .on_input(Message::ProfileNameChanged)
                    .on_submit(Message::RenameProfile)
                    .width(260),
                button("Rename").on_press_maybe(renamed.then_some(Message::RenameProfile)),
                button("Duplicate")
                    .style(button::secondary)
                    .on_press(Message::DuplicateProfile),
                delete,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
        page = page.push(applies_to(app, snapshot, profile));
    }

    page = page.push(space().height(4)).push(heading("Buttons"));
    let controls: Vec<_> = snapshot
        .devices
        .iter()
        .flat_map(|device| &device.config.controls)
        .collect();
    if controls.is_empty() {
        page = page.push(text(
            "No buttons are named yet. Open the mouse's page and capture some first.",
        ));
    }
    let mut seen: Vec<&str> = Vec::new();
    for control in controls {
        // Two mice can share a label; show it once.
        if seen.contains(&control.label.as_str()) {
            continue;
        }
        seen.push(&control.label);
        page = page.push(binding_row(
            app,
            snapshot,
            profile,
            &control.label,
            &control.event,
        ));
    }

    // Bindings for labels no connected config knows about are kept in the
    // file; say so rather than silently hiding them.
    let orphans: Vec<&str> = profile
        .bindings
        .keys()
        .map(String::as_str)
        .filter(|label| !seen.contains(label))
        .collect();
    if !orphans.is_empty() {
        page = page.push(hint(format!(
            "Also bound in this profile, for buttons no mouse has: {}",
            orphans.join(", ")
        )));
    }
    page.into()
}

fn applies_to<'a>(
    app: &'a App,
    snapshot: &'a Snapshot,
    profile: &'a Profile,
) -> Element<'a, Message> {
    let mut section = column![
        heading("Applies to"),
        hint("This profile is active while one of these apps has focus."),
    ]
    .spacing(8);

    for class in &profile.matches.window_class {
        section = section.push(
            row![
                text(class).width(Length::Fill),
                button("Remove")
                    .style(button::secondary)
                    .on_press(Message::RemoveWindowClass(class.clone())),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
    }
    if profile.matches.window_class.is_empty() && profile.matches.exe.is_empty() {
        section = section.push(
            text("No app chosen yet, so this profile never activates by itself.")
                .style(text::danger),
        );
    }
    if !profile.matches.exe.is_empty() {
        section = section.push(hint(format!(
            "Also matches programs named: {} (set in the profile file)",
            profile.matches.exe.join(", ")
        )));
    }

    section = section.push(
        button(if app.show_window_picker {
            "Hide running apps"
        } else {
            "Add from running apps"
        })
        .on_press(Message::ToggleWindowPicker),
    );

    if app.show_window_picker {
        let mut picker = column![].spacing(6);
        let mut listed: Vec<&str> = Vec::new();
        for window in &snapshot.windows {
            if listed.contains(&window.class.as_str()) {
                continue;
            }
            listed.push(&window.class);
            let owner = owner_of(snapshot, &window.class);
            let action: Element<'_, Message> = match owner {
                Some(name) if name == profile.name => hint("added"),
                Some(name) => hint(format!("used by {name}")),
                None => button("Add")
                    .on_press(Message::AddWindowClass(window.class.clone()))
                    .into(),
            };
            picker = picker.push(
                row![
                    column![text(&window.title), hint(window.class.clone())].width(Length::Fill),
                    action,
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            );
        }
        if listed.is_empty() {
            picker = picker.push(hint(
                "No open windows reported. Start the app, then come back here.",
            ));
        }
        picker = picker.push(
            row![
                text_input("Or type a window class", &app.class_draft)
                    .on_input(Message::ClassDraftChanged)
                    .on_submit(Message::AddClassDraft)
                    .width(260),
                button("Add").style(button::secondary).on_press_maybe(
                    (!app.class_draft.trim().is_empty()).then_some(Message::AddClassDraft)
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
        section = section.push(container(picker).padding(12).style(container::bordered_box));
    }
    section.into()
}

/// The profile that already claims a window class, if any.
fn owner_of<'a>(snapshot: &'a Snapshot, class: &str) -> Option<&'a str> {
    snapshot
        .profiles
        .iter()
        .find(|profile| {
            profile
                .matches
                .window_class
                .iter()
                .any(|claimed| claimed.eq_ignore_ascii_case(class))
        })
        .map(|profile| profile.name.as_str())
}

/// One entry in the "what does this button do" dropdown.
#[derive(Debug, Clone, PartialEq)]
struct KindOption {
    kind: Kind,
    label: String,
}

impl fmt::Display for KindOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

fn binding_row<'a>(
    app: &'a App,
    snapshot: &'a Snapshot,
    profile: &'a Profile,
    label: &'a str,
    event: &'a Trigger,
) -> Element<'a, Message> {
    let name = column![text(label), hint(keys::describe_trigger(event))].width(LABEL_WIDTH);

    if *event == Trigger::Key(Key::BTN_LEFT) {
        return row![
            name,
            hint("Left click always works normally; it cannot be rebound.")
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into();
    }

    let is_default = app.is_default(profile);
    let binding = profile.bindings.get(label);
    let kind = kind_of(binding);

    // What "unset" means depends on the profile.
    let unset_label = if is_default {
        "Normal".to_string()
    } else {
        match snapshot.default_profile.bindings.get(label) {
            Some(inherited) => format!("Same as Default ({})", summary(inherited)),
            None => "Normal".to_string(),
        }
    };
    let mut options = vec![KindOption {
        kind: Kind::Unset,
        label: unset_label,
    }];
    // "Normal, even though Default binds it" only makes sense when Default
    // binds it.
    if !is_default && snapshot.default_profile.bindings.contains_key(label) {
        options.push(KindOption {
            kind: Kind::Passthrough,
            label: "Normal (ignore Default)".into(),
        });
    }
    options.push(KindOption {
        kind: Kind::Keys,
        label: "Key or shortcut".into(),
    });
    options.push(KindOption {
        kind: Kind::MouseButton,
        label: "Mouse button".into(),
    });
    options.push(KindOption {
        kind: Kind::Disabled,
        label: "Disabled".into(),
    });
    if !snapshot.macros.is_empty() {
        options.push(KindOption {
            kind: Kind::Macro,
            label: "Macro".into(),
        });
    }
    // A binding the list above does not offer (a macro that was deleted by
    // hand, or a stray passthrough) is still shown as what it is.
    if options.iter().all(|option| option.kind != kind) {
        options.push(KindOption {
            kind,
            label: binding.map(summary).unwrap_or_default(),
        });
    }
    let selected = options.iter().find(|option| option.kind == kind).cloned();
    let owned_label = label.to_string();
    let kind_picker = pick_list(options, selected, move |option: KindOption| {
        Message::SetKind(owned_label.clone(), option.kind)
    })
    .width(KIND_WIDTH);

    let detail: Element<'_, Message> = match binding {
        Some(Binding::Key { keys }) => {
            let editing = app.editing_keys.as_deref() == Some(label);
            button(text(keys::chord_text(keys)))
                .style(if editing {
                    button::primary
                } else {
                    button::secondary
                })
                .on_press(if editing {
                    Message::DoneEditingKeys
                } else {
                    Message::EditKeys(label.to_string())
                })
                .into()
        }
        Some(Binding::MouseButton { button }) => {
            let choices = keys::mouse_button_choices();
            let selected = choices.iter().find(|choice| choice.key == *button).cloned();
            let owned_label = label.to_string();
            pick_list(choices, selected, move |choice: KeyChoice| {
                Message::SetMouseButton(owned_label.clone(), choice)
            })
            .placeholder(keys::friendly(*button))
            .into()
        }
        Some(Binding::Macro { name, mode }) => {
            let names: Vec<String> = snapshot.macros.iter().map(|m| m.name.clone()).collect();
            let selected = names.iter().find(|candidate| *candidate == name).cloned();
            let modes = mode_options();
            let selected_mode = modes.iter().find(|option| option.mode == *mode).cloned();
            let label_for_macro = label.to_string();
            let label_for_mode = label.to_string();
            row![
                pick_list(names, selected, move |name: String| {
                    Message::SetMacro(label_for_macro.clone(), name)
                })
                .placeholder(format!("{name} (missing)")),
                pick_list(modes, selected_mode, move |option: ModeOption| {
                    Message::SetMacroMode(label_for_mode.clone(), option.mode)
                }),
            ]
            .spacing(8)
            .into()
        }
        _ => space().into(),
    };

    let line = row![name, kind_picker, detail]
        .spacing(12)
        .align_y(Alignment::Center);

    if app.editing_keys.as_deref() == Some(label)
        && let Some(chord) = app.editing_chord()
    {
        column![line, chord_editor(app, &chord)].spacing(8).into()
    } else {
        line.into()
    }
}

/// The expanded editor under a "Key or shortcut" row.
fn chord_editor<'a>(app: &'a App, chord: &keys::Chord) -> Element<'a, Message> {
    let mut modifiers = row![].spacing(14).align_y(Alignment::Center);
    for (index, (_, name)) in keys::MODIFIERS.iter().enumerate() {
        modifiers = modifiers.push(
            checkbox(chord.modifiers[index])
                .label(*name)
                .on_toggle(move |on| Message::ToggleModifier(index, on)),
        );
    }
    let main_choice = chord.main.map(|key| KeyChoice {
        key,
        name: keys::friendly(key),
    });
    let main_key = combo_box(
        &app.key_choices,
        "Type to search keys",
        main_choice.as_ref(),
        Message::SetMainKey,
    )
    .width(220);

    container(
        column![
            row![text("Hold:"), modifiers, text("and press:"), main_key]
                .spacing(12)
                .align_y(Alignment::Center),
            row![
                pick_list(keys::presets(), None::<keys::Preset>, Message::ApplyPreset)
                    .placeholder("Or pick a common shortcut")
                    .width(300),
                space().width(Length::Fill),
                button("Done").on_press(Message::DoneEditingKeys),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            hint(
                "Numpad keys are good spare keys for games. F13 to F24 only work if your \
                 desktop treats them as plain function keys."
            ),
        ]
        .spacing(10),
    )
    .padding(12)
    .style(container::bordered_box)
    .into()
}

/// One entry in the "how does the macro play" dropdown.
#[derive(Debug, Clone, PartialEq)]
struct ModeOption {
    mode: MacroMode,
    label: &'static str,
}

impl fmt::Display for ModeOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label)
    }
}

fn mode_options() -> Vec<ModeOption> {
    vec![
        ModeOption {
            mode: MacroMode::Once,
            label: "Once per press",
        },
        ModeOption {
            mode: MacroMode::RepeatWhileHeld,
            label: "Repeat while held",
        },
        ModeOption {
            mode: MacroMode::Toggle,
            label: "Toggle on / off",
        },
    ]
}

// ----------------------------------------------------------------- macro page

fn macro_page<'a>(
    app: &'a App,
    snapshot: &'a Snapshot,
    current: &'a Macro,
) -> Element<'a, Message> {
    let renamed =
        app.macro_name_draft.trim() != current.name && !app.macro_name_draft.trim().is_empty();
    let delete: Element<'_, Message> = if app.confirm_delete {
        button("Really delete?")
            .style(button::danger)
            .on_press(Message::ConfirmDeleteMacro)
            .into()
    } else {
        button("Delete")
            .style(button::danger)
            .on_press(Message::AskDeleteMacro)
            .into()
    };
    let users: Vec<&str> = std::iter::once(&snapshot.default_profile)
        .chain(&snapshot.profiles)
        .filter(|profile| {
            profile.bindings.values().any(
                |binding| matches!(binding, Binding::Macro { name, .. } if *name == current.name),
            )
        })
        .map(|profile| profile.name.as_str())
        .collect();

    let mut page = column![
        text(&current.name).size(26),
        row![
            // Disabled while recording, so typed keys go to the recording
            // and not into this box.
            text_input("Macro name", &app.macro_name_draft)
                .on_input_maybe(
                    app.recording
                        .is_none()
                        .then_some(Message::MacroNameChanged as fn(String) -> Message)
                )
                .on_submit_maybe(app.recording.is_none().then_some(Message::RenameMacro))
                .width(260),
            button("Rename").on_press_maybe(renamed.then_some(Message::RenameMacro)),
            delete,
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        hint(if users.is_empty() {
            "Not used yet. To use it, open a profile and set a button to \"Macro\".".to_string()
        } else {
            format!("Used by: {}", users.join(", "))
        }),
        space().height(4),
    ]
    .spacing(10);

    if let Some(recording) = &app.recording {
        page = page
            .push(heading("Recording"))
            .push(text(
                "Type the keys on your keyboard now. The pauses between them are recorded too.",
            ))
            .push(
                row![
                    button("Stop recording").on_press(Message::StopRecording),
                    hint("Click this with the mouse when you are done. Every key, including Esc, is recorded."),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
        let mut steps = column![].spacing(4);
        for step in &recording.steps {
            steps = steps.push(text(keys::describe_step(step)).size(14));
        }
        if recording.steps.is_empty() {
            steps = steps.push(hint("Nothing recorded yet."));
        }
        return page.push(steps).into();
    }

    page = page.push(heading("Steps")).push(
        row![
            button(if current.steps.is_empty() {
                "Record from keyboard"
            } else {
                "Record again (replaces these steps)"
            })
            .on_press(Message::StartRecording),
            hint(format!(
                "{} steps, {} ms per pass",
                current.steps.len(),
                current.duration_ms()
            )),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    );

    for (index, step) in current.steps.iter().enumerate() {
        let what: Element<'_, Message> = match step {
            Step::Delay { ms } => {
                let shown = app
                    .delay_drafts
                    .get(&index)
                    .cloned()
                    .unwrap_or_else(|| ms.to_string());
                row![
                    text("Wait"),
                    text_input("ms", &shown)
                        .on_input(move |text| Message::StepDelayChanged(index, text))
                        .width(80),
                    text("ms"),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .into()
            }
            other => text(keys::describe_step(other)).into(),
        };
        page = page.push(
            row![
                text(format!("{}.", index + 1)).width(36),
                container(what).width(Length::Fill),
                button("Up")
                    .style(button::secondary)
                    .on_press_maybe((index > 0).then_some(Message::MoveStep(index, Direction::Up))),
                button("Down").style(button::secondary).on_press_maybe(
                    (index + 1 < current.steps.len())
                        .then_some(Message::MoveStep(index, Direction::Down))
                ),
                button("Remove")
                    .style(button::danger)
                    .on_press(Message::RemoveStep(index)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        );
    }
    if current.steps.is_empty() {
        page = page.push(text("No steps yet. Record some, or add them below."));
    }

    let valid_all = app.delay_all_draft.trim().parse::<u32>().is_ok();
    page = page
        .push(space().height(4))
        .push(heading("Edit"))
        .push(
            row![
                combo_box(
                    &app.key_choices,
                    "Add a key press (type to search)",
                    None,
                    Message::AddKeyTap,
                )
                .width(280),
                button("Add a wait")
                    .style(button::secondary)
                    .on_press(Message::AddDelay),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
        .push(
            row![
                text("Set every wait to"),
                text_input("ms", &app.delay_all_draft)
                    .on_input(Message::DelayAllChanged)
                    .on_submit(Message::SetAllDelays)
                    .width(80),
                text("ms"),
                button("Apply")
                    .style(button::secondary)
                    .on_press_maybe(valid_all.then_some(Message::SetAllDelays)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .push(hint(
            "Whether timed macros are allowed in a competitive game is up to that game's rules.",
        ));
    page.into()
}

/// A binding in a few words, for dropdown labels.
fn summary(binding: &Binding) -> String {
    match binding {
        Binding::Key { keys } => keys::chord_text(keys),
        Binding::MouseButton { button } => keys::friendly(*button),
        Binding::Macro { name, .. } => format!("Macro: {name}"),
        Binding::Passthrough => "Normal".into(),
        Binding::Disabled => "Disabled".into(),
    }
}
