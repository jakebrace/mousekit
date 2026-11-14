//! Reading config files from disk and turning them into engine tables.
//! The parsing and rules live in `mousekit-core`; this is the file I/O.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mousekit_core::profile::ResolvedProfile;
use mousekit_core::{Action, Config, Macro, Profile, Step, check_macros, check_profiles, resolve};
use mousekit_input::VirtualMouse;

/// The result of loading the config directory.
pub struct Loaded {
    /// The config directory everything was read from.
    pub dir: PathBuf,
    pub config: Config,
    pub default: Profile,
    /// App-specific profiles, sorted by file name.
    pub profiles: Vec<Profile>,
    /// The file each app profile came from, same order as `profiles`.
    pub profile_paths: Vec<PathBuf>,
    /// Macros from `macros/*.toml`, sorted by file name.
    pub macros: Vec<Macro>,
    /// The file each macro came from, same order as `macros`.
    pub macro_paths: Vec<PathBuf>,
    /// Engine tables: `tables[device][0]` is the default profile for that
    /// device and `tables[device][i + 1]` is `profiles[i]` layered over it.
    tables: Vec<Vec<Arc<ResolvedProfile>>>,
    /// Problems that did not stop loading.
    pub warnings: Vec<String>,
}

impl Loaded {
    /// The engine table for one device. `active` is an index into
    /// `profiles`, or `None` for the default profile.
    pub fn table(&self, device: usize, active: Option<usize>) -> Arc<ResolvedProfile> {
        let slot = active.map_or(0, |index| index + 1);
        self.tables[device][slot].clone()
    }

    pub fn profile_name(&self, active: Option<usize>) -> &str {
        match active {
            Some(index) => &self.profiles[index].name,
            None => &self.default.name,
        }
    }
}

/// `$XDG_CONFIG_HOME/mousekit`, normally `~/.config/mousekit`.
pub fn config_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir).join("mousekit"));
    }
    match std::env::var_os("HOME") {
        Some(home) => Ok(PathBuf::from(home).join(".config/mousekit")),
        None => Err("neither XDG_CONFIG_HOME nor HOME is set".into()),
    }
}

/// Reads `config.toml` and every `profiles/*.toml` from `dir`.
/// `profiles/default.toml` is the default profile; every other file is an
/// app-specific profile.
pub fn load(dir: &Path) -> Result<Loaded, String> {
    let config_path = dir.join("config.toml");
    let text = match fs::read_to_string(&config_path) {
        Ok(text) => text,
        // First run: start with an empty config. The settings window adds
        // the first mouse, and the file is written then.
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", config_path.display())),
    };
    let config =
        Config::from_toml(&text).map_err(|error| format!("{}: {error}", config_path.display()))?;

    let mut warnings = Vec::new();
    if config.devices.is_empty() {
        warnings.push(
            "no mouse is set up yet; open the settings window (mousekit gui) to add one".into(),
        );
    }

    let profiles_dir = dir.join("profiles");
    let default_path = profiles_dir.join("default.toml");
    let default = match read_profile(&default_path) {
        Ok(profile) => profile,
        // No default profile file simply means nothing is bound by default.
        Err(ReadError::Missing) => {
            Profile::from_toml("name = \"Default\"").expect("built-in profile is valid")
        }
        Err(ReadError::Other(message)) => return Err(message),
    };

    let paths: Vec<PathBuf> = toml_files(&profiles_dir)
        .into_iter()
        .filter(|path| *path != default_path)
        .collect();
    let mut profiles = Vec::new();
    let mut profile_paths = Vec::new();
    for path in &paths {
        match read_profile(path) {
            Ok(profile) => {
                profiles.push(profile);
                profile_paths.push(path.clone());
            }
            // Deleted between listing and reading; nothing to load.
            Err(ReadError::Missing) => {}
            Err(ReadError::Other(message)) => return Err(message),
        }
    }
    check_profiles(&default, &profiles)
        .map_err(|error| format!("{}: {error}", profiles_dir.display()))?;
    for profile in &profiles {
        if profile.matches.window_class.is_empty() && profile.matches.exe.is_empty() {
            warnings.push(format!(
                "profile \"{}\" has no [match] rules, so it will never activate",
                profile.name
            ));
        }
    }

    let macros_dir = dir.join("macros");
    let mut macros = Vec::new();
    let mut macro_paths = Vec::new();
    for path in toml_files(&macros_dir) {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            // Deleted between listing and reading; nothing to load.
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        };
        let parsed =
            Macro::from_toml(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        for step in &parsed.steps {
            if let Step::KeyDown { key } | Step::KeyUp { key } = step
                && !VirtualMouse::can_emit(*key)
            {
                warnings.push(format!(
                    "macro \"{}\": {key} cannot be emitted by the virtual device and will do nothing",
                    parsed.name
                ));
            }
        }
        macros.push(parsed);
        macro_paths.push(path);
    }
    check_macros(&macros).map_err(|error| format!("{}: {error}", macros_dir.display()))?;

    let mut tables = Vec::new();
    for device in &config.devices {
        let mut device_tables = Vec::new();
        let overlays = std::iter::once(None).chain(profiles.iter().map(Some));
        for overlay in overlays {
            let (resolved, mut resolve_warnings) = resolve(device, &default, overlay, &macros)
                .map_err(|error| format!("{}: {error}", profiles_dir.display()))?;
            warnings.append(&mut resolve_warnings);
            for action in resolved.actions.values() {
                let Action::Keys(keys) = action else { continue };
                for key in keys.iter().filter(|key| !VirtualMouse::can_emit(**key)) {
                    warnings.push(format!(
                        "{key} cannot be emitted by the virtual device and will do nothing"
                    ));
                }
            }
            device_tables.push(Arc::new(resolved));
        }
        tables.push(device_tables);
    }
    // The default profile's bindings are resolved once per overlay, so the
    // same warning can come up several times.
    warnings.sort();
    warnings.dedup();

    Ok(Loaded {
        dir: dir.to_path_buf(),
        config,
        default,
        profiles,
        profile_paths,
        macros,
        macro_paths,
        tables,
        warnings,
    })
}

/// The `.toml` files in a directory, sorted by name. A missing directory
/// is simply empty.
fn toml_files(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
            .collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();
    paths
}

enum ReadError {
    Missing,
    Other(String),
}

fn read_profile(path: &Path) -> Result<Profile, ReadError> {
    let text = fs::read_to_string(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            ReadError::Missing
        } else {
            ReadError::Other(format!("cannot read {}: {error}", path.display()))
        }
    })?;
    Profile::from_toml(&text)
        .map_err(|error| ReadError::Other(format!("{}: {error}", path.display())))
}
