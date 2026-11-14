//! Reading config files from disk and turning them into engine tables.
//! The parsing and rules live in `mouse-profiles-core`; this is the file I/O.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mouse_profiles_core::profile::ResolvedProfile;
use mouse_profiles_core::{Action, Config, Profile, resolve};
use mouse_profiles_input::VirtualMouse;

/// The result of loading the config directory.
pub struct Loaded {
    pub config: Config,
    /// The default profile resolved for each device, same order as
    /// `config.devices`.
    pub profiles: Vec<Arc<ResolvedProfile>>,
    /// Problems that did not stop loading.
    pub warnings: Vec<String>,
}

/// `$XDG_CONFIG_HOME/mouse-profiles`, normally `~/.config/mouse-profiles`.
pub fn config_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir).join("mouse-profiles"));
    }
    match std::env::var_os("HOME") {
        Some(home) => Ok(PathBuf::from(home).join(".config/mouse-profiles")),
        None => Err("neither XDG_CONFIG_HOME nor HOME is set".into()),
    }
}

/// Reads `config.toml` and `profiles/default.toml` from `dir`.
/// Other profile files are ignored until Milestone 2.
pub fn load(dir: &Path) -> Result<Loaded, String> {
    let config_path = dir.join("config.toml");
    let text = fs::read_to_string(&config_path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            format!(
                "no config file at {}\nCopy docs/examples/ from the repository to get started.",
                config_path.display()
            )
        } else {
            format!("cannot read {}: {error}", config_path.display())
        }
    })?;
    let config =
        Config::from_toml(&text).map_err(|error| format!("{}: {error}", config_path.display()))?;

    let mut warnings = Vec::new();
    if config.devices.is_empty() {
        warnings.push(format!(
            "{} lists no [[device]], so there is nothing to remap",
            config_path.display()
        ));
    }

    let default_path = dir.join("profiles/default.toml");
    let default = match fs::read_to_string(&default_path) {
        Ok(text) => Profile::from_toml(&text)
            .map_err(|error| format!("{}: {error}", default_path.display()))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            warnings.push(format!(
                "no default profile at {}; every button passes through",
                default_path.display()
            ));
            Profile::from_toml("name = \"Default\"").expect("built-in profile is valid")
        }
        Err(error) => return Err(format!("cannot read {}: {error}", default_path.display())),
    };

    let mut profiles = Vec::new();
    for device in &config.devices {
        let (resolved, mut resolve_warnings) = resolve(device, &default, None)
            .map_err(|error| format!("{}: {error}", default_path.display()))?;
        warnings.append(&mut resolve_warnings);
        for action in resolved.actions.values() {
            let Action::Keys(keys) = action else { continue };
            for key in keys.iter().filter(|key| !VirtualMouse::can_emit(**key)) {
                warnings.push(format!(
                    "profile \"{}\": {key} cannot be emitted by the virtual device and will do nothing",
                    resolved.name
                ));
            }
        }
        profiles.push(Arc::new(resolved));
    }

    Ok(Loaded {
        config,
        profiles,
        warnings,
    })
}
