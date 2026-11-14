//! Listing input nodes without opening them.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use mousekit_core::DeviceId;

/// One /dev/input/eventN node, described from sysfs. Reading sysfs needs no
/// permission on the node itself, so this works before the udev rule has
/// granted access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    /// Only valid until the device goes away; never store it in config.
    pub path: PathBuf,
    pub id: DeviceId,
    pub name: String,
    /// Whether it looks like a mouse: it reports pointer motion on both
    /// axes and has a left button.
    pub is_mouse: bool,
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
        let is_mouse = has_bits(&sysfs.join("capabilities/rel"), &[REL_X, REL_Y])
            && has_bits(&sysfs.join("capabilities/key"), &[BTN_LEFT]);
        nodes.push(NodeInfo {
            path,
            id: DeviceId { vendor, product },
            name,
            is_mouse,
        });
    }
    Ok(nodes)
}

const REL_X: usize = 0x00;
const REL_Y: usize = 0x01;
const BTN_LEFT: usize = 0x110;

/// Reads a sysfs capability file and checks that every listed bit is set.
fn has_bits(path: &Path, bits: &[usize]) -> bool {
    fs::read_to_string(path).is_ok_and(|text| bits.iter().all(|bit| capability_bit(&text, *bit)))
}

/// Tests one bit of a sysfs capability bitmap. The kernel prints it as
/// space-separated hexadecimal words of 64 bits each, highest word first.
fn capability_bit(text: &str, bit: usize) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    let Some(position) = words.len().checked_sub(1 + bit / 64) else {
        return false;
    };
    u64::from_str_radix(words[position], 16).is_ok_and(|word| word & (1 << (bit % 64)) != 0)
}

fn read_hex(path: &Path) -> Option<u16> {
    u16::from_str_radix(fs::read_to_string(path).ok()?.trim(), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_bitmaps_are_read_highest_word_first() {
        // The key capabilities of a real G502 X node: BTN_LEFT (0x110) is
        // bit 16 of the fifth word from the right.
        let keys = "3f3f3fffffff03ff ffffffff007fffff 700ff7fffffff 7fe003fffff000f \
                    f7ffffffffffffff ffffffff00000000 ff0000 1ffffffffffff07 \
                    ffffffffffffffff ffffffffffefffff fffffffffffffffe";
        assert!(capability_bit(keys, BTN_LEFT));
        assert!(!capability_bit(keys, 0));
        // A relative-axis bitmap with X, Y and both wheels.
        assert!(capability_bit("1943", REL_X) && capability_bit("1943", REL_Y));
        assert!(!capability_bit("1940", REL_X));
        // Asking beyond the end of a short bitmap is simply "not set".
        assert!(!capability_bit("3", BTN_LEFT));
        assert!(!capability_bit("", 0));
    }
}
