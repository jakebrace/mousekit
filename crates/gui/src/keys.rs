//! Friendly names for keys and buttons, and the bits of logic the binding
//! editor needs: splitting a chord into modifiers plus a main key, the
//! preset shortcuts, and suggested labels for captured buttons.

use std::fmt;

use mousekit_core::{Key, ScrollAxis, Trigger};

/// A key or button as shown in a dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChoice {
    pub key: Key,
    pub name: String,
}

impl fmt::Display for KeyChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

fn key(name: &str) -> Key {
    Key::from_name(name).unwrap_or_else(|| panic!("{name} is not a kernel key name"))
}

/// The four modifiers offered as checkboxes, in the order they are pressed.
pub const MODIFIERS: [(&str, &str); 4] = [
    ("KEY_LEFTCTRL", "Ctrl"),
    ("KEY_LEFTSHIFT", "Shift"),
    ("KEY_LEFTALT", "Alt"),
    ("KEY_LEFTMETA", "Super"),
];

pub fn modifier_keys() -> [Key; 4] {
    MODIFIERS.map(|(name, _)| key(name))
}

fn is_modifier(candidate: Key) -> bool {
    const NAMES: [&str; 8] = [
        "KEY_LEFTCTRL",
        "KEY_RIGHTCTRL",
        "KEY_LEFTSHIFT",
        "KEY_RIGHTSHIFT",
        "KEY_LEFTALT",
        "KEY_RIGHTALT",
        "KEY_LEFTMETA",
        "KEY_RIGHTMETA",
    ];
    NAMES.iter().any(|name| key(name) == candidate)
}

/// Names that are not obvious from the kernel name.
const SPECIAL_NAMES: &[(&str, &str)] = &[
    ("KEY_LEFTCTRL", "Ctrl"),
    ("KEY_RIGHTCTRL", "Right Ctrl"),
    ("KEY_LEFTSHIFT", "Shift"),
    ("KEY_RIGHTSHIFT", "Right Shift"),
    ("KEY_LEFTALT", "Alt"),
    ("KEY_RIGHTALT", "Right Alt"),
    ("KEY_LEFTMETA", "Super"),
    ("KEY_RIGHTMETA", "Right Super"),
    ("KEY_ENTER", "Enter"),
    ("KEY_ESC", "Escape"),
    ("KEY_TAB", "Tab"),
    ("KEY_SPACE", "Space"),
    ("KEY_BACKSPACE", "Backspace"),
    ("KEY_DELETE", "Delete"),
    ("KEY_INSERT", "Insert"),
    ("KEY_HOME", "Home"),
    ("KEY_END", "End"),
    ("KEY_PAGEUP", "Page Up"),
    ("KEY_PAGEDOWN", "Page Down"),
    ("KEY_UP", "Up Arrow"),
    ("KEY_DOWN", "Down Arrow"),
    ("KEY_LEFT", "Left Arrow"),
    ("KEY_RIGHT", "Right Arrow"),
    ("KEY_MINUS", "-"),
    ("KEY_EQUAL", "="),
    ("KEY_LEFTBRACE", "["),
    ("KEY_RIGHTBRACE", "]"),
    ("KEY_SEMICOLON", ";"),
    ("KEY_APOSTROPHE", "'"),
    ("KEY_GRAVE", "`"),
    ("KEY_BACKSLASH", "\\"),
    ("KEY_COMMA", ","),
    ("KEY_DOT", "."),
    ("KEY_SLASH", "/"),
    ("KEY_CAPSLOCK", "Caps Lock"),
    ("KEY_SCROLLLOCK", "Scroll Lock"),
    ("KEY_SYSRQ", "Print Screen"),
    ("KEY_PAUSE", "Pause"),
    ("KEY_KPPLUS", "Numpad +"),
    ("KEY_KPMINUS", "Numpad -"),
    ("KEY_KPASTERISK", "Numpad *"),
    ("KEY_KPSLASH", "Numpad /"),
    ("KEY_KPDOT", "Numpad ."),
    ("KEY_KPENTER", "Numpad Enter"),
    ("KEY_MUTE", "Mute"),
    ("KEY_VOLUMEUP", "Volume Up"),
    ("KEY_VOLUMEDOWN", "Volume Down"),
    ("KEY_PLAYPAUSE", "Play / Pause"),
    ("KEY_NEXTSONG", "Next Track"),
    ("KEY_PREVIOUSSONG", "Previous Track"),
    ("BTN_LEFT", "Left Click"),
    ("BTN_RIGHT", "Right Click"),
    ("BTN_MIDDLE", "Middle Click"),
    ("BTN_SIDE", "Back"),
    ("BTN_EXTRA", "Forward"),
];

/// A readable name for any key or button.
pub fn friendly(candidate: Key) -> String {
    let Some(kernel) = candidate.name() else {
        // No kernel name. Extra mouse buttons continue the numbering after
        // left (1), right (2), middle (3), back (4), forward (5)...
        return match candidate.0 {
            0x110..0x120 => format!("Mouse button {}", candidate.0 - 0x110 + 1),
            _ => candidate.to_string(),
        };
    };
    if let Some((_, name)) = SPECIAL_NAMES.iter().find(|(k, _)| *k == kernel) {
        return name.to_string();
    }
    if let Some(digit) = kernel.strip_prefix("KEY_KP")
        && digit.len() == 1
    {
        return format!("Numpad {digit}");
    }
    match kernel.strip_prefix("KEY_") {
        // Letters, digits and function keys read fine as they are.
        Some(rest) => rest.to_string(),
        None => kernel.to_string(),
    }
}

/// The keys offered as the main key of a shortcut, in a sensible order.
pub fn main_key_choices() -> Vec<KeyChoice> {
    let mut names: Vec<String> = Vec::new();
    names.extend((0..=9).map(|n| format!("KEY_KP{n}")));
    names.extend(
        [
            "KPPLUS",
            "KPMINUS",
            "KPASTERISK",
            "KPSLASH",
            "KPDOT",
            "KPENTER",
        ]
        .map(|n| format!("KEY_{n}")),
    );
    names.extend((1..=24).map(|n| format!("KEY_F{n}")));
    names.extend(('A'..='Z').map(|c| format!("KEY_{c}")));
    names.extend((0..=9).map(|n| format!("KEY_{n}")));
    names.extend(
        [
            "ENTER",
            "ESC",
            "TAB",
            "SPACE",
            "BACKSPACE",
            "DELETE",
            "INSERT",
            "HOME",
            "END",
            "PAGEUP",
            "PAGEDOWN",
            "UP",
            "DOWN",
            "LEFT",
            "RIGHT",
            "MINUS",
            "EQUAL",
            "LEFTBRACE",
            "RIGHTBRACE",
            "SEMICOLON",
            "APOSTROPHE",
            "GRAVE",
            "BACKSLASH",
            "COMMA",
            "DOT",
            "SLASH",
            "CAPSLOCK",
            "SCROLLLOCK",
            "SYSRQ",
            "PAUSE",
            "MUTE",
            "VOLUMEUP",
            "VOLUMEDOWN",
            "PLAYPAUSE",
            "NEXTSONG",
            "PREVIOUSSONG",
        ]
        .map(|n| format!("KEY_{n}")),
    );
    names
        .iter()
        .map(|name| {
            let key = key(name);
            KeyChoice {
                key,
                name: friendly(key),
            }
        })
        .collect()
}

/// The buttons offered for a "mouse button" binding.
pub fn mouse_button_choices() -> Vec<KeyChoice> {
    [
        "BTN_LEFT",
        "BTN_RIGHT",
        "BTN_MIDDLE",
        "BTN_SIDE",
        "BTN_EXTRA",
    ]
    .iter()
    .map(|name| {
        let key = key(name);
        KeyChoice {
            key,
            name: friendly(key),
        }
    })
    .collect()
}

/// "Alt + Tab" for a list of keys.
pub fn chord_text(keys: &[Key]) -> String {
    keys.iter()
        .map(|key| friendly(*key))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// A shortcut split into the four checkbox modifiers and one main key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    /// Ctrl, Shift, Alt, Super, in the order of [`MODIFIERS`].
    pub modifiers: [bool; 4],
    pub main: Option<Key>,
}

impl Chord {
    /// Reads a key list. A right-hand modifier counts as its left-hand twin.
    pub fn from_keys(keys: &[Key]) -> Chord {
        let twins = [
            ["KEY_LEFTCTRL", "KEY_RIGHTCTRL"],
            ["KEY_LEFTSHIFT", "KEY_RIGHTSHIFT"],
            ["KEY_LEFTALT", "KEY_RIGHTALT"],
            ["KEY_LEFTMETA", "KEY_RIGHTMETA"],
        ];
        let modifiers = twins.map(|pair| pair.iter().any(|name| keys.contains(&key(name))));
        let main = keys.iter().rev().copied().find(|key| !is_modifier(*key));
        Chord { modifiers, main }
    }

    /// The key list to store: modifiers first, then the main key. `None` if
    /// nothing at all is selected.
    pub fn to_keys(&self) -> Option<Vec<Key>> {
        let mut keys: Vec<Key> = modifier_keys()
            .into_iter()
            .zip(self.modifiers)
            .filter_map(|(key, on)| on.then_some(key))
            .collect();
        keys.extend(self.main);
        (!keys.is_empty()).then_some(keys)
    }
}

/// A ready-made shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    pub keys: Vec<Key>,
}

impl fmt::Display for Preset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

pub fn presets() -> Vec<Preset> {
    let preset = |name, keys: &[&str]| Preset {
        name,
        keys: keys.iter().map(|k| key(k)).collect(),
    };
    vec![
        preset("Alt + Tab (switch window)", &["KEY_LEFTALT", "KEY_TAB"]),
        preset("Ctrl + Tab (next tab)", &["KEY_LEFTCTRL", "KEY_TAB"]),
        preset(
            "Ctrl + Shift + Tab (previous tab)",
            &["KEY_LEFTCTRL", "KEY_LEFTSHIFT", "KEY_TAB"],
        ),
        preset("Ctrl + C (copy)", &["KEY_LEFTCTRL", "KEY_C"]),
        preset("Ctrl + V (paste)", &["KEY_LEFTCTRL", "KEY_V"]),
        preset("Ctrl + X (cut)", &["KEY_LEFTCTRL", "KEY_X"]),
        preset("Ctrl + Z (undo)", &["KEY_LEFTCTRL", "KEY_Z"]),
        preset("Ctrl + W (close tab)", &["KEY_LEFTCTRL", "KEY_W"]),
        preset("Ctrl + T (new tab)", &["KEY_LEFTCTRL", "KEY_T"]),
        preset("Super (app launcher)", &["KEY_LEFTMETA"]),
        preset("Play / Pause", &["KEY_PLAYPAUSE"]),
        preset("Volume Up", &["KEY_VOLUMEUP"]),
        preset("Volume Down", &["KEY_VOLUMEDOWN"]),
    ]
}

/// The kernel key for a physical keyboard key as the window toolkit
/// reports it, used by the macro recorder. Physical keys are used rather
/// than the characters they type, so a recording does not depend on the
/// keyboard layout.
pub fn from_physical(code: iced::keyboard::key::Code) -> Option<Key> {
    const TABLE: &[(&str, &str)] = &[
        ("Backquote", "GRAVE"),
        ("Backslash", "BACKSLASH"),
        ("BracketLeft", "LEFTBRACE"),
        ("BracketRight", "RIGHTBRACE"),
        ("Comma", "COMMA"),
        ("Equal", "EQUAL"),
        ("Minus", "MINUS"),
        ("Period", "DOT"),
        ("Quote", "APOSTROPHE"),
        ("Semicolon", "SEMICOLON"),
        ("Slash", "SLASH"),
        ("AltLeft", "LEFTALT"),
        ("AltRight", "RIGHTALT"),
        ("Backspace", "BACKSPACE"),
        ("CapsLock", "CAPSLOCK"),
        ("ControlLeft", "LEFTCTRL"),
        ("ControlRight", "RIGHTCTRL"),
        ("Enter", "ENTER"),
        ("SuperLeft", "LEFTMETA"),
        ("SuperRight", "RIGHTMETA"),
        ("ShiftLeft", "LEFTSHIFT"),
        ("ShiftRight", "RIGHTSHIFT"),
        ("Space", "SPACE"),
        ("Tab", "TAB"),
        ("Delete", "DELETE"),
        ("End", "END"),
        ("Home", "HOME"),
        ("Insert", "INSERT"),
        ("PageDown", "PAGEDOWN"),
        ("PageUp", "PAGEUP"),
        ("ArrowDown", "DOWN"),
        ("ArrowLeft", "LEFT"),
        ("ArrowRight", "RIGHT"),
        ("ArrowUp", "UP"),
        ("NumpadAdd", "KPPLUS"),
        ("NumpadSubtract", "KPMINUS"),
        ("NumpadMultiply", "KPASTERISK"),
        ("NumpadDivide", "KPSLASH"),
        ("NumpadDecimal", "KPDOT"),
        ("NumpadEnter", "KPENTER"),
        ("Escape", "ESC"),
        ("PrintScreen", "SYSRQ"),
        ("ScrollLock", "SCROLLLOCK"),
        ("Pause", "PAUSE"),
        ("NumLock", "NUMLOCK"),
    ];
    // The toolkit's names are regular: "KeyA", "Digit1", "F5", "Numpad7".
    let name = format!("{code:?}");
    let kernel = if let Some(letter) = name.strip_prefix("Key") {
        letter.to_string()
    } else if let Some(digit) = name.strip_prefix("Digit") {
        digit.to_string()
    } else if let Some(digit) = name.strip_prefix("Numpad")
        && digit.len() == 1
        && digit.chars().all(|c| c.is_ascii_digit())
    {
        format!("KP{digit}")
    } else if let Some((_, kernel)) = TABLE.iter().find(|(toolkit, _)| *toolkit == name) {
        kernel.to_string()
    } else {
        // Function keys ("F1".."F24") already match.
        name
    };
    Key::from_name(&format!("KEY_{kernel}"))
}

/// A macro step in words: "Press B", "Release B", "Wait 40 ms".
pub fn describe_step(step: &mousekit_core::Step) -> String {
    use mousekit_core::Step;
    match step {
        Step::KeyDown { key } => format!("Press {}", friendly(*key)),
        Step::KeyUp { key } => format!("Release {}", friendly(*key)),
        Step::Delay { ms } => format!("Wait {ms} ms"),
    }
}

/// What a control's event is, in words: "Back button", "Scroll up".
pub fn describe_trigger(trigger: &Trigger) -> String {
    match trigger {
        Trigger::Key(key) => match friendly(*key).as_str() {
            name @ ("Left Click" | "Right Click" | "Middle Click") => name.to_string(),
            name @ ("Back" | "Forward") => format!("{name} button"),
            name => name.to_string(),
        },
        Trigger::Scroll { axis, positive } => match (axis, positive) {
            (ScrollAxis::Vertical, true) => "Scroll up".into(),
            (ScrollAxis::Vertical, false) => "Scroll down".into(),
            (ScrollAxis::Horizontal, true) => "Scroll / tilt right".into(),
            (ScrollAxis::Horizontal, false) => "Scroll / tilt left".into(),
        },
    }
}

/// A label to propose for a newly captured control, avoiding ones in use.
pub fn suggest_label(trigger: &Trigger, taken: &[&str]) -> String {
    let base = match trigger {
        Trigger::Key(key) => match key.name() {
            Some("BTN_LEFT") => "LMB".to_string(),
            Some("BTN_RIGHT") => "RMB".to_string(),
            Some("BTN_MIDDLE") => "M3".to_string(),
            Some("BTN_SIDE") => "M4".to_string(),
            Some("BTN_EXTRA") => "M5".to_string(),
            _ => "Button".to_string(),
        },
        Trigger::Scroll { .. } => describe_trigger(trigger),
    };
    if !taken.contains(&base.as_str()) && base != "Button" {
        return base;
    }
    (1..)
        .map(|n| format!("{base} {n}"))
        .find(|candidate| !taken.contains(&candidate.as_str()))
        .expect("an unused number always exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_key_exists_and_has_a_distinct_name() {
        let choices = main_key_choices();
        let mut names: Vec<&str> = choices.iter().map(|c| c.name.as_str()).collect();
        names.sort_unstable();
        let total = names.len();
        names.dedup();
        assert_eq!(names.len(), total, "two keys share a display name");
        assert_eq!(mouse_button_choices().len(), 5);
        assert!(!presets().is_empty());
        for (name, _) in SPECIAL_NAMES {
            key(name);
        }
    }

    #[test]
    fn friendly_names() {
        assert_eq!(friendly(key("KEY_KP7")), "Numpad 7");
        assert_eq!(friendly(key("KEY_F13")), "F13");
        assert_eq!(friendly(key("KEY_A")), "A");
        assert_eq!(friendly(key("KEY_LEFTALT")), "Alt");
        assert_eq!(
            chord_text(&[key("KEY_LEFTALT"), key("KEY_TAB")]),
            "Alt + Tab"
        );
    }

    #[test]
    fn unnamed_mouse_buttons_get_a_number() {
        assert_eq!(friendly(Key(0x119)), "Mouse button 10");
        let trigger = Trigger::Key(Key(0x119));
        assert_eq!(describe_trigger(&trigger), "Mouse button 10");
        assert_eq!(suggest_label(&trigger, &[]), "Button 1");
    }

    #[test]
    fn physical_keys_map_to_kernel_keys() {
        use iced::keyboard::key::Code;
        assert_eq!(from_physical(Code::KeyB), Some(key("KEY_B")));
        assert_eq!(from_physical(Code::Digit7), Some(key("KEY_7")));
        assert_eq!(from_physical(Code::F5), Some(key("KEY_F5")));
        assert_eq!(from_physical(Code::Numpad7), Some(key("KEY_KP7")));
        assert_eq!(from_physical(Code::NumpadAdd), Some(key("KEY_KPPLUS")));
        assert_eq!(from_physical(Code::ShiftLeft), Some(key("KEY_LEFTSHIFT")));
        assert_eq!(from_physical(Code::Escape), Some(key("KEY_ESC")));
        assert_eq!(from_physical(Code::ArrowUp), Some(key("KEY_UP")));
        assert_eq!(from_physical(Code::Period), Some(key("KEY_DOT")));
    }

    #[test]
    fn chords_split_and_rebuild() {
        let chord = Chord::from_keys(&[key("KEY_LEFTALT"), key("KEY_TAB")]);
        assert_eq!(chord.modifiers, [false, false, true, false]);
        assert_eq!(chord.main, Some(key("KEY_TAB")));
        assert_eq!(
            chord.to_keys(),
            Some(vec![key("KEY_LEFTALT"), key("KEY_TAB")])
        );

        // Modifier-only and empty chords.
        let only_shift = Chord::from_keys(&[key("KEY_RIGHTSHIFT")]);
        assert_eq!(only_shift.main, None);
        assert_eq!(only_shift.to_keys(), Some(vec![key("KEY_LEFTSHIFT")]));
        assert_eq!(Chord::from_keys(&[]).to_keys(), None);
    }

    #[test]
    fn suggested_labels_follow_the_plan_and_avoid_clashes() {
        let side = Trigger::Key(key("BTN_SIDE"));
        assert_eq!(suggest_label(&side, &[]), "M4");
        assert_eq!(suggest_label(&side, &["M4"]), "M4 1");
        let odd = Trigger::Key(key("BTN_TASK"));
        assert_eq!(suggest_label(&odd, &["Button 1"]), "Button 2");
        let up: Trigger = "REL_WHEEL:+".parse().unwrap();
        assert_eq!(suggest_label(&up, &[]), "Scroll up");
    }
}
