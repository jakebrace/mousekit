//! Profile files, and resolving a profile against one mouse.
//!
//! A profile file says "label M4 does X". Resolving looks each label up in
//! the mouse's control list and produces the table the engine actually uses:
//! "event BTN_SIDE does X".

use std::collections::{BTreeMap, HashMap};

use serde::Deserialize;

use crate::config::DeviceConfig;
use crate::error::ConfigError;
use crate::event::{Key, Trigger};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    /// Which windows activate this profile. Unused until Milestone 2.
    #[serde(default, rename = "match")]
    pub matches: MatchRules,
    /// Control label -> what it does.
    #[serde(default)]
    pub bindings: BTreeMap<String, Binding>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MatchRules {
    pub window_class: Vec<String>,
    pub exe: Vec<String>,
}

/// One entry in a profile's `[bindings]` table, e.g.
/// `M4 = { type = "key", keys = ["KEY_LEFTALT", "KEY_TAB"] }`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Binding {
    /// One key, or modifiers plus a key, held while the button is held.
    Key { keys: Vec<Key> },
    /// A literal mouse button code.
    MouseButton { button: Key },
    /// A stored macro. Parsed now, played from Milestone 5.
    Macro {
        name: String,
        #[serde(default)]
        mode: MacroMode,
    },
    /// The original event, overriding the default profile's binding.
    Passthrough,
    /// Nothing at all.
    Disabled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacroMode {
    #[default]
    Once,
    RepeatWhileHeld,
    Toggle,
}

impl Profile {
    /// Parses and validates the text of one profile file.
    pub fn from_toml(text: &str) -> Result<Profile, ConfigError> {
        let profile: Profile = toml::from_str(text)?;
        for (label, binding) in &profile.bindings {
            match binding {
                Binding::Key { keys } if keys.is_empty() => {
                    return Err(ConfigError::Invalid(format!(
                        "profile \"{}\": binding for \"{label}\" has an empty keys list",
                        profile.name
                    )));
                }
                Binding::MouseButton { button } if !button.is_button() => {
                    return Err(ConfigError::Invalid(format!(
                        "profile \"{}\": binding for \"{label}\" is a mouse_button but {button} is a keyboard key; use type = \"key\"",
                        profile.name
                    )));
                }
                _ => {}
            }
        }
        Ok(profile)
    }
}

/// What the engine does when a trigger fires. A trigger with no entry in a
/// [`ResolvedProfile`] passes through unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Hold these keys or buttons (pressed in order, released in reverse).
    Keys(Vec<Key>),
    /// Swallow the event.
    Disabled,
}

/// A profile flattened against one mouse: ready for the engine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedProfile {
    pub name: String,
    pub actions: HashMap<Trigger, Action>,
}

/// Builds the engine's table for one mouse.
///
/// `overlay` is the profile for the focused app, layered over `default`: a
/// control the overlay does not mention keeps the default's binding.
///
/// Returns the table plus warnings for things that are skipped but not fatal
/// (a label this mouse does not have, a macro before macros exist). Any
/// binding on left click is an error, because a bad one locks you out of
/// your desktop.
pub fn resolve(
    device: &DeviceConfig,
    default: &Profile,
    overlay: Option<&Profile>,
) -> Result<(ResolvedProfile, Vec<String>), ConfigError> {
    let mut actions = HashMap::new();
    let mut warnings = Vec::new();

    for profile in std::iter::once(default).chain(overlay) {
        for (label, binding) in &profile.bindings {
            let Some(control) = device.control(label) else {
                warnings.push(format!(
                    "profile \"{}\": \"{}\" has no control labelled \"{label}\"; binding ignored",
                    profile.name, device.name
                ));
                continue;
            };
            let trigger = control.event;
            if trigger == Trigger::Key(Key::BTN_LEFT) {
                return Err(ConfigError::Invalid(format!(
                    "profile \"{}\": \"{label}\" is left click, which cannot be rebound",
                    profile.name
                )));
            }
            match binding {
                Binding::Key { keys } => {
                    actions.insert(trigger, Action::Keys(keys.clone()));
                }
                Binding::MouseButton { button } => {
                    actions.insert(trigger, Action::Keys(vec![*button]));
                }
                Binding::Disabled => {
                    actions.insert(trigger, Action::Disabled);
                }
                Binding::Passthrough => {
                    actions.remove(&trigger);
                }
                Binding::Macro { name, .. } => {
                    warnings.push(format!(
                        "profile \"{}\": macro \"{name}\" on \"{label}\" is ignored; macros are not implemented yet",
                        profile.name
                    ));
                }
            }
        }
    }

    let name = overlay.unwrap_or(default).name.clone();
    Ok((ResolvedProfile { name, actions }, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn key(name: &str) -> Key {
        Key::from_name(name).unwrap()
    }

    fn device() -> DeviceConfig {
        let config = Config::from_toml(
            r#"
            [[device]]
            ids = ["046d:409f"]
            name = "Test Mouse"
            [[device.control]]
            label = "LMB"
            event = "BTN_LEFT"
            [[device.control]]
            label = "M4"
            event = "BTN_SIDE"
            [[device.control]]
            label = "M5"
            event = "BTN_EXTRA"
            [[device.control]]
            label = "Scroll Up"
            event = "REL_WHEEL:+"
            "#,
        )
        .unwrap();
        config.devices.into_iter().next().unwrap()
    }

    /// The files in docs/examples are what a new user copies, so they must
    /// always load cleanly.
    #[test]
    fn the_example_files_load_without_warnings() {
        let config = Config::from_toml(include_str!("../../../docs/examples/config.toml")).unwrap();
        let default =
            Profile::from_toml(include_str!("../../../docs/examples/profiles/default.toml"))
                .unwrap();
        let (resolved, warnings) = resolve(&config.devices[0], &default, None).unwrap();
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(
            resolved.actions[&Trigger::Key(key("BTN_SIDE"))],
            Action::Keys(vec![key("KEY_KP7")])
        );
    }

    #[test]
    fn parses_every_binding_type() {
        let profile = Profile::from_toml(
            r#"
            name = "Deadlock"

            [match]
            window_class = ["steam_app_1422450"]

            [bindings]
            M4 = { type = "key", keys = ["KEY_LEFTALT", "KEY_TAB"] }
            M5 = { type = "mouse_button", button = "BTN_MIDDLE" }
            G7 = { type = "macro", name = "buy-combo", mode = "once" }
            G8 = { type = "passthrough" }
            G9 = { type = "disabled" }
            "#,
        )
        .unwrap();
        assert_eq!(profile.matches.window_class, ["steam_app_1422450"]);
        assert_eq!(
            profile.bindings["M4"],
            Binding::Key {
                keys: vec![key("KEY_LEFTALT"), key("KEY_TAB")]
            }
        );
        assert_eq!(profile.bindings["G9"], Binding::Disabled);
    }

    #[test]
    fn rejects_malformed_bindings() {
        let empty_keys = "name = \"x\"\n[bindings]\nM4 = { type = \"key\", keys = [] }";
        assert!(matches!(
            Profile::from_toml(empty_keys),
            Err(ConfigError::Invalid(_))
        ));
        let key_as_button =
            "name = \"x\"\n[bindings]\nM4 = { type = \"mouse_button\", button = \"KEY_A\" }";
        assert!(matches!(
            Profile::from_toml(key_as_button),
            Err(ConfigError::Invalid(_))
        ));
        let unknown_type = "name = \"x\"\n[bindings]\nM4 = { type = \"teleport\" }";
        assert!(matches!(
            Profile::from_toml(unknown_type),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn overlay_inherits_and_overrides_the_default() {
        let default = Profile::from_toml(
            r#"
            name = "Default"
            [bindings]
            M4 = { type = "key", keys = ["KEY_F13"] }
            M5 = { type = "key", keys = ["KEY_F14"] }
            "Scroll Up" = { type = "disabled" }
            "#,
        )
        .unwrap();
        let game = Profile::from_toml(
            r#"
            name = "Game"
            [bindings]
            M5 = { type = "mouse_button", button = "BTN_MIDDLE" }
            "Scroll Up" = { type = "passthrough" }
            "#,
        )
        .unwrap();

        let (resolved, warnings) = resolve(&device(), &default, Some(&game)).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(resolved.name, "Game");
        // Inherited from the default.
        assert_eq!(
            resolved.actions[&Trigger::Key(key("BTN_SIDE"))],
            Action::Keys(vec![key("KEY_F13")])
        );
        // Overridden by the overlay.
        assert_eq!(
            resolved.actions[&Trigger::Key(key("BTN_EXTRA"))],
            Action::Keys(vec![key("BTN_MIDDLE")])
        );
        // Explicit passthrough removes the default's binding.
        assert_eq!(resolved.actions.len(), 2);
    }

    #[test]
    fn left_click_cannot_be_bound_to_anything() {
        for binding in ["{ type = \"disabled\" }", "{ type = \"passthrough\" }"] {
            let profile =
                Profile::from_toml(&format!("name = \"x\"\n[bindings]\nLMB = {binding}")).unwrap();
            let error = resolve(&device(), &profile, None).unwrap_err();
            assert!(error.to_string().contains("left click"), "{error}");
        }
    }

    #[test]
    fn unknown_labels_and_macros_warn_but_do_not_fail() {
        let profile = Profile::from_toml(
            r#"
            name = "x"
            [bindings]
            G9 = { type = "disabled" }
            M4 = { type = "macro", name = "combo" }
            "#,
        )
        .unwrap();
        let (resolved, warnings) = resolve(&device(), &profile, None).unwrap();
        assert!(resolved.actions.is_empty());
        assert_eq!(warnings.len(), 2);
    }
}
