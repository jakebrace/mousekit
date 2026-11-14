//! The GUI's only link to the outside world: calls to the daemon's D-Bus
//! interface. The GUI never opens devices or config files itself.

use mousekit_core::dbus::{BUS_NAME, INSTANCE_VARIABLE, INTERFACE, OBJECT_PATH};
use mousekit_core::{DeviceConfig, Macro, Profile, Snapshot};
use zbus::blocking::Connection;
use zbus::zvariant::{DynamicDeserialize, DynamicType};

pub struct Client {
    connection: Connection,
    bus_name: String,
}

impl Client {
    pub fn connect() -> Result<Client, String> {
        let connection = Connection::session()
            .map_err(|error| format!("cannot connect to the session bus: {error}"))?;
        let bus_name = match std::env::var(INSTANCE_VARIABLE) {
            Ok(instance) if !instance.is_empty() => format!("{BUS_NAME}.{instance}"),
            _ => BUS_NAME.to_string(),
        };
        Ok(Client {
            connection,
            bus_name,
        })
    }

    fn call<R>(
        &self,
        method: &str,
        body: &(impl zbus::export::serde::Serialize + DynamicType),
    ) -> Result<R, String>
    where
        R: for<'a> DynamicDeserialize<'a>,
    {
        let reply = self
            .connection
            .call_method(
                Some(self.bus_name.as_str()),
                OBJECT_PATH,
                Some(INTERFACE),
                method,
                body,
            )
            .map_err(|error| match error {
                zbus::Error::MethodError(name, message, _) => {
                    if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" {
                        "The mousekit daemon is not running.".to_string()
                    } else {
                        message.unwrap_or_else(|| name.to_string())
                    }
                }
                other => format!("cannot reach the daemon: {other}"),
            })?;
        reply
            .body()
            .deserialize()
            .map_err(|error| format!("unexpected reply from the daemon: {error}"))
    }

    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let text: String = self.call("GetSnapshot", &())?;
        Snapshot::from_toml(&text)
            .map_err(|error| format!("cannot read the daemon's state: {error}"))
    }

    /// `original` is the profile's current name, or empty to create it.
    pub fn save_profile(&self, original: &str, profile: &Profile) -> Result<(), String> {
        self.call("SaveProfile", &(original, profile.to_toml()))
    }

    pub fn delete_profile(&self, name: &str) -> Result<(), String> {
        self.call("DeleteProfile", &(name,))
    }

    /// `original` is the macro's current name, or empty to create it.
    pub fn save_macro(&self, original: &str, new: &Macro) -> Result<(), String> {
        self.call("SaveMacro", &(original, new.to_toml()))
    }

    pub fn delete_macro(&self, name: &str) -> Result<(), String> {
        self.call("DeleteMacro", &(name,))
    }

    pub fn save_device(&self, index: usize, device: &DeviceConfig) -> Result<(), String> {
        self.call("SaveDevice", &(index as u32, device.to_toml()))
    }

    pub fn add_device(&self, id: &str, name: &str) -> Result<(), String> {
        self.call("AddDevice", &(id, name))
    }

    pub fn remove_device(&self, index: usize) -> Result<(), String> {
        self.call("RemoveDevice", &(index as u32,))
    }

    pub fn set_dpi(&self, index: usize, dpi: u32) -> Result<(), String> {
        self.call("SetDpi", &(index as u32, dpi))
    }

    pub fn set_polling_hz(&self, index: usize, hz: u32) -> Result<(), String> {
        self.call("SetPollingHz", &(index as u32, hz))
    }

    pub fn rename_control(&self, index: usize, old: &str, new: &str) -> Result<(), String> {
        self.call("RenameControl", &(index as u32, old, new))
    }

    pub fn set_paused(&self, paused: bool) -> Result<(), String> {
        let method = if paused { "Pause" } else { "Resume" };
        self.call::<bool>(method, &()).map(|_| ())
    }

    pub fn start_capture(&self) -> Result<(), String> {
        self.call("StartCapture", &())
    }

    pub fn cancel_capture(&self) -> Result<(), String> {
        self.call("CancelCapture", &())
    }

    /// Whether the capture session is still open, and the event names
    /// captured since the last poll.
    pub fn poll_capture(&self) -> Result<(bool, Vec<String>), String> {
        self.call("PollCapture", &())
    }
}
