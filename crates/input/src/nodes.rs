//! Listing input nodes without opening them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use mouse_profiles_core::DeviceId;

/// One /dev/input/eventN node, described from sysfs. Reading sysfs needs no
/// permission on the node itself, so this works before the udev rule has
/// granted access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    /// Only valid until the device goes away; never store it in config.
    pub path: PathBuf,
    pub id: DeviceId,
    pub name: String,
}

/// Lists every event node currently present, in no particular order.
pub fn list_nodes() -> io::Result<Vec<NodeInfo>> {
    let mut nodes = Vec::new();
    for entry in fs::read_dir("/dev/input")? {
        let path = entry?.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !file_name.starts_with("event") {
            continue;
        }
        let sysfs = PathBuf::from(format!("/sys/class/input/{file_name}/device"));
        // A node can vanish between the directory listing and these reads.
        let (Some(vendor), Some(product)) = (
            read_hex(&sysfs.join("id/vendor")),
            read_hex(&sysfs.join("id/product")),
        ) else {
            continue;
        };
        let name = fs::read_to_string(sysfs.join("name"))
            .map(|name| name.trim().to_string())
            .unwrap_or_default();
        nodes.push(NodeInfo {
            path,
            id: DeviceId { vendor, product },
            name,
        });
    }
    Ok(nodes)
}

fn read_hex(path: &Path) -> Option<u16> {
    u16::from_str_radix(fs::read_to_string(path).ok()?.trim(), 16).ok()
}
