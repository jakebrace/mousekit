//! Deciding which profile a focused window calls for.
//!
//! The compositor-specific code in `mouse-profiles-focus` turns whatever the
//! desktop reports into a [`Window`]; everything after that is decided here.

use std::collections::HashMap;

use crate::error::ConfigError;
use crate::profile::Profile;

/// What sort of thing took focus, as far as profile switching cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    /// An ordinary application window or dialog.
    App,
    /// Part of the desktop itself: a panel, the launcher, a popup, or
    /// nothing at all. These grab focus for a moment and must not flip the
    /// profile.
    Shell,
    /// The session is locked.
    LockScreen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// The window class the compositor reports, e.g. "firefox" or
    /// "steam_app_1422450".
    pub class: String,
    pub title: String,
    /// File name of the program that owns the window, when it can be found.
    pub exe: Option<String>,
    pub kind: WindowKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Leave the active profile as it is.
    Keep,
    /// Use the default profile.
    Default,
    /// Use the profile at this index in the list given to [`decide`].
    Profile(usize),
}

/// Picks the profile for a newly focused window. `profiles` are the
/// app-specific profiles, not including the default.
///
/// Window class is checked across all profiles before program name, because
/// the class is what the compositor reports for the live window; the program
/// name is a fallback. Both compare without regard to case.
pub fn decide(profiles: &[Profile], window: &Window) -> Decision {
    match window.kind {
        WindowKind::Shell => return Decision::Keep,
        WindowKind::LockScreen => return Decision::Default,
        WindowKind::App => {}
    }
    let by_class = profiles.iter().position(|profile| {
        profile
            .matches
            .window_class
            .iter()
            .any(|class| class.eq_ignore_ascii_case(&window.class))
    });
    let by_exe = || {
        let exe = window.exe.as_deref()?;
        profiles.iter().position(|profile| {
            profile
                .matches
                .exe
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(exe))
        })
    };
    match by_class.or_else(by_exe) {
        Some(index) => Decision::Profile(index),
        None => Decision::Default,
    }
}

/// Enforces "one app, one profile": no window class or program name may be
/// claimed by two profiles, and no two profiles may share a name.
pub fn check_profiles(default: &Profile, profiles: &[Profile]) -> Result<(), ConfigError> {
    let mut names = HashMap::new();
    names.insert(default.name.to_lowercase(), ());
    let mut claims: HashMap<String, &str> = HashMap::new();
    for profile in profiles {
        if names.insert(profile.name.to_lowercase(), ()).is_some() {
            return Err(ConfigError::Invalid(format!(
                "two profiles are named \"{}\"",
                profile.name
            )));
        }
        let classes = profile
            .matches
            .window_class
            .iter()
            .map(|c| ("window class", c));
        let exes = profile.matches.exe.iter().map(|e| ("program", e));
        for (what, value) in classes.chain(exes) {
            let key = format!("{what}:{}", value.to_lowercase());
            if let Some(other) = claims.insert(key, &profile.name)
                && other != profile.name
            {
                return Err(ConfigError::Invalid(format!(
                    "{what} \"{value}\" is claimed by both \"{other}\" and \"{}\"",
                    profile.name
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, classes: &[&str], exes: &[&str]) -> Profile {
        let list = |items: &[&str]| {
            items
                .iter()
                .map(|item| format!("\"{item}\""))
                .collect::<Vec<_>>()
                .join(", ")
        };
        Profile::from_toml(&format!(
            "name = \"{name}\"\n[match]\nwindow_class = [{}]\nexe = [{}]\n",
            list(classes),
            list(exes)
        ))
        .unwrap()
    }

    fn app(class: &str, exe: Option<&str>) -> Window {
        Window {
            class: class.into(),
            title: String::new(),
            exe: exe.map(str::to_string),
            kind: WindowKind::App,
        }
    }

    #[test]
    fn class_match_picks_the_profile_and_no_match_means_default() {
        let profiles = [
            profile("Browser", &["firefox"], &[]),
            profile("Deadlock", &["steam_app_1422450"], &[]),
        ];
        assert_eq!(
            decide(&profiles, &app("steam_app_1422450", None)),
            Decision::Profile(1)
        );
        assert_eq!(
            decide(&profiles, &app("Firefox", None)),
            Decision::Profile(0)
        );
        assert_eq!(decide(&profiles, &app("dolphin", None)), Decision::Default);
    }

    #[test]
    fn exe_is_a_fallback_and_class_wins_over_it() {
        let profiles = [
            profile("ByExe", &[], &["game.exe"]),
            profile("ByClass", &["steam_app_1"], &[]),
        ];
        assert_eq!(
            decide(&profiles, &app("something", Some("Game.exe"))),
            Decision::Profile(0)
        );
        assert_eq!(
            decide(&profiles, &app("steam_app_1", Some("game.exe"))),
            Decision::Profile(1)
        );
    }

    #[test]
    fn shell_windows_keep_the_profile_and_the_lock_screen_resets_it() {
        let profiles = [profile("Browser", &["plasmashell"], &[])];
        let mut window = app("plasmashell", None);
        window.kind = WindowKind::Shell;
        assert_eq!(decide(&profiles, &window), Decision::Keep);
        window.kind = WindowKind::LockScreen;
        assert_eq!(decide(&profiles, &window), Decision::Default);
    }

    #[test]
    fn two_profiles_cannot_claim_the_same_app_or_name() {
        let default = profile("Default", &[], &[]);
        let clash = [
            profile("A", &["firefox"], &[]),
            profile("B", &["Firefox"], &[]),
        ];
        let error = check_profiles(&default, &clash).unwrap_err().to_string();
        assert!(error.contains("claimed by both"), "{error}");

        let same_name = [profile("A", &[], &[]), profile("a", &[], &[])];
        assert!(check_profiles(&default, &same_name).is_err());
        assert!(check_profiles(&default, &[profile("default", &[], &[])]).is_err());

        let fine = [
            profile("A", &["firefox"], &["x"]),
            profile("B", &["chromium"], &["y"]),
        ];
        assert!(check_profiles(&default, &fine).is_ok());
    }
}
