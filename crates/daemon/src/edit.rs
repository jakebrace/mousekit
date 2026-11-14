//! Changing the config on behalf of the GUI. The daemon is the only process
//! that writes config files: the GUI sends the new profile or device as
//! TOML text, and everything here checks it against the rest of the config
//! before a file is touched, so a bad edit can never leave the daemon with
//! files it cannot load.

use std::fs;
use std::path::{Path, PathBuf};

use mouse_profiles_core::{
    Config, DeviceConfig, DeviceSnapshot, Profile, Snapshot, WindowSummary, check_profiles, resolve,
};

use crate::State;

impl State {
    /// Everything the GUI shows, in one value.
    pub fn snapshot(&self) -> Snapshot {
        let connected_ids: Vec<_> = mouse_profiles_input::list_nodes()
            .unwrap_or_default()
            .into_iter()
            .filter(|node| self.active.contains(&node.path))
            .map(|node| node.id)
            .collect();
        Snapshot {
            paused: self.paused,
            active_profile: self.loaded.profile_name(self.active_profile).to_string(),
            override_profile: match self.override_profile {
                Some(choice) => self.loaded.profile_name(choice).to_string(),
                None => String::new(),
            },
            focus_status: self.focus_status.clone(),
            default_profile: self.loaded.default.clone(),
            profiles: self.loaded.profiles.clone(),
            devices: self
                .loaded
                .config
                .devices
                .iter()
                .map(|device| DeviceSnapshot {
                    connected: device.ids.iter().any(|id| connected_ids.contains(id)),
                    config: device.clone(),
                })
                .collect(),
            windows: self
                .windows
                .iter()
                .map(|window| WindowSummary {
                    class: window.class.clone(),
                    title: window.title.clone(),
                    exe: window.exe.clone().unwrap_or_default(),
                })
                .collect(),
        }
    }

    /// Creates or replaces a profile. `original` is the profile's current
    /// name, or empty to create a new one; the name inside `toml` may differ
    /// (a rename).
    pub fn save_profile(&mut self, original: &str, toml: &str) -> Result<(), String> {
        let profile = Profile::from_toml(toml).map_err(|error| error.to_string())?;

        let is_default = !original.is_empty() && original == self.loaded.default.name;
        let existing = self
            .loaded
            .profiles
            .iter()
            .position(|candidate| candidate.name == original);
        if !original.is_empty() && !is_default && existing.is_none() {
            return Err(format!("there is no profile named \"{original}\""));
        }

        // Build the profile set as it would be after the save, and check it.
        let mut default = self.loaded.default.clone();
        let mut profiles = self.loaded.profiles.clone();
        if is_default {
            default = profile.clone();
        } else if let Some(index) = existing {
            profiles[index] = profile.clone();
        } else {
            profiles.push(profile.clone());
        }
        check_all(&self.loaded.config, &default, &profiles)?;

        let profiles_dir = self.loaded.dir.join("profiles");
        let path = if is_default {
            profiles_dir.join("default.toml")
        } else if let Some(index) = existing {
            self.loaded.profile_paths[index].clone()
        } else {
            new_profile_path(&profiles_dir, &profile.name)
        };
        write_file(&path, &profile.to_toml())?;
        self.reload().map(|_| ())
    }

    pub fn delete_profile(&mut self, name: &str) -> Result<(), String> {
        if name == self.loaded.default.name {
            return Err("the default profile cannot be deleted".into());
        }
        let index = self
            .loaded
            .profiles
            .iter()
            .position(|candidate| candidate.name == name)
            .ok_or_else(|| format!("there is no profile named \"{name}\""))?;
        let path = &self.loaded.profile_paths[index];
        fs::remove_file(path)
            .map_err(|error| format!("cannot delete {}: {error}", path.display()))?;
        self.reload().map(|_| ())
    }

    /// Replaces one `[[device]]` block (its name and controls). The device's
    /// IDs cannot change this way; that needs a daemon restart.
    pub fn save_device(&mut self, index: usize, toml: &str) -> Result<(), String> {
        let device: DeviceConfig =
            toml::from_str(toml).map_err(|error| format!("bad device description: {error}"))?;
        let mut config = self.loaded.config.clone();
        let slot = config
            .devices
            .get_mut(index)
            .ok_or_else(|| format!("there is no device number {index}"))?;
        if slot.ids != device.ids {
            return Err("a device's IDs cannot be changed while the daemon is running".into());
        }
        *slot = device;
        config.validate().map_err(|error| error.to_string())?;
        check_all(&config, &self.loaded.default, &self.loaded.profiles)?;

        write_file(&self.loaded.dir.join("config.toml"), &config.to_toml())?;
        self.reload().map(|_| ())
    }

    /// Renames a control and every binding that refers to it, so profiles
    /// keep working.
    pub fn rename_control(&mut self, index: usize, old: &str, new: &str) -> Result<(), String> {
        let new = new.trim();
        if new.is_empty() {
            return Err("a control needs a label".into());
        }
        let mut config = self.loaded.config.clone();
        let device = config
            .devices
            .get_mut(index)
            .ok_or_else(|| format!("there is no device number {index}"))?;
        let control = device
            .controls
            .iter_mut()
            .find(|control| control.label == old)
            .ok_or_else(|| format!("there is no control labelled \"{old}\""))?;
        control.label = new.to_string();
        config.validate().map_err(|error| error.to_string())?;

        let rename_in = |profile: &Profile| -> Option<Profile> {
            let mut profile = profile.clone();
            let binding = profile.bindings.remove(old)?;
            profile.bindings.insert(new.to_string(), binding);
            Some(profile)
        };
        let default = rename_in(&self.loaded.default);
        let profiles: Vec<Option<Profile>> = self.loaded.profiles.iter().map(rename_in).collect();

        let profiles_dir = self.loaded.dir.join("profiles");
        write_file(&self.loaded.dir.join("config.toml"), &config.to_toml())?;
        if let Some(default) = default {
            write_file(&profiles_dir.join("default.toml"), &default.to_toml())?;
        }
        for (profile, path) in profiles.iter().zip(&self.loaded.profile_paths) {
            if let Some(profile) = profile {
                write_file(path, &profile.to_toml())?;
            }
        }
        self.reload().map(|_| ())
    }
}

/// The checks `load` would make, run before anything is written.
fn check_all(config: &Config, default: &Profile, profiles: &[Profile]) -> Result<(), String> {
    check_profiles(default, profiles).map_err(|error| error.to_string())?;
    for device in &config.devices {
        for overlay in std::iter::once(None).chain(profiles.iter().map(Some)) {
            resolve(device, default, overlay).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// A file name for a new profile: its name in lowercase with anything that
/// is not a letter or digit turned into a dash, made unique with a number.
fn new_profile_path(profiles_dir: &Path, name: &str) -> PathBuf {
    let mut slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    slug = slug.trim_matches('-').to_string();
    if slug.is_empty() || slug == "default" {
        slug = "profile".into();
    }
    let mut path = profiles_dir.join(format!("{slug}.toml"));
    let mut number = 2;
    while path.exists() {
        path = profiles_dir.join(format!("{slug}-{number}.toml"));
        number += 1;
    }
    path
}

/// Writes a whole file by writing a temporary one next to it and renaming
/// it into place, so a crash mid-write cannot leave half a file.
fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
    }
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, text)
        .and_then(|()| fs::rename(&temporary, path))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}
