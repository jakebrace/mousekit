//! Milestone 0, spike (a): grab a mouse, mirror it through a uinput virtual
//! device, and replace BTN_SIDE with KEY_F13. Throwaway code.
//!
//! Usage: spike-grab-remap [VENDOR:PRODUCT] [--quiet]
//!
//! Stop it with Ctrl+C in the terminal. The kernel drops the grab when the
//! process exits, so the mouse goes back to normal by itself.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

use evdev::raw_stream::RawDevice;
use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, BusType, EventSummary, EventType, InputEvent, InputId, KeyCode, RelativeAxisCode,
    SynchronizationCode,
};

/// The one hard-coded remap this spike exists to prove.
const FROM: KeyCode = KeyCode::BTN_SIDE;
const TO: KeyCode = KeyCode::KEY_F13;

/// Fixed identity for the virtual device, so KDE's per-device pointer
/// settings (acceleration profile) stick to it between runs.
const VIRTUAL_NAME: &str = "mouse-profiles spike virtual mouse";
const VIRTUAL_VENDOR: u16 = 0x1209;
const VIRTUAL_PRODUCT: u16 = 0x5350;

/// One /dev/input/eventN node, whether or not we were allowed to open it.
struct Node {
    path: PathBuf,
    name: String,
    vendor: u16,
    product: u16,
    /// The open device, or why it could not be opened.
    device: io::Result<RawDevice>,
}

impl Node {
    fn looks_like_mouse(&self) -> bool {
        match &self.device {
            Ok(dev) => dev
                .supported_relative_axes()
                .is_some_and(|axes| axes.contains(RelativeAxisCode::REL_X)),
            Err(_) => false,
        }
    }
}

fn main() {
    if let Err(message) = run() {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut wanted_id: Option<(u16, u16)> = None;
    let mut quiet = false;
    for arg in std::env::args().skip(1) {
        if arg == "--quiet" {
            quiet = true;
        } else if let Some(id) = parse_id(&arg) {
            wanted_id = Some(id);
        } else {
            return Err(format!(
                "unrecognised argument '{arg}'. Usage: spike-grab-remap [VENDOR:PRODUCT] [--quiet]"
            ));
        }
    }

    let mut nodes = scan_nodes().map_err(|e| format!("cannot read /dev/input: {e}"))?;
    if let Some((vendor, product)) = wanted_id {
        nodes.retain(|n| n.vendor == vendor && n.product == product);
        if nodes.is_empty() {
            return Err(format!(
                "no input device with ID {vendor:04x}:{product:04x}"
            ));
        }
    }

    let node = pick_node(nodes)?;
    let mut mouse = match node.device {
        Ok(dev) => dev,
        Err(e) => {
            return Err(format!(
                "cannot open {} ({}): {e}\n\
                 Your user has no access to this device. Install the udev rule in\n\
                 packaging/udev/ (see the instructions for this spike), then run again.",
                node.name,
                node.path.display()
            ));
        }
    };
    println!(
        "Using {:04x}:{:04x} {} ({})",
        node.vendor,
        node.product,
        node.name,
        node.path.display()
    );

    let mut output = build_virtual_device(&mouse).map_err(|e| {
        format!("cannot create the uinput virtual device: {e}\nCheck `getfacl /dev/uinput` lists your user.")
    })?;

    // A button held at the moment of the grab would never be seen released by
    // the desktop, so wait until nothing is pressed.
    wait_for_all_released(&mouse).map_err(|e| format!("cannot read button state: {e}"))?;
    mouse
        .grab()
        .map_err(|e| format!("cannot grab the mouse (is another remapper running?): {e}"))?;

    println!("Grabbed. Every event is mirrored to \"{VIRTUAL_NAME}\".");
    println!("{FROM:?} is replaced with {TO:?}. Press Ctrl+C here to stop.\n");

    forward_events(&mut mouse, &mut output, quiet).map_err(|e| {
        format!("stopped: {e} (if the mouse went to sleep or was unplugged, this is expected in the spike)")
    })
}

/// Parses "046d:409f" into (0x046d, 0x409f).
fn parse_id(text: &str) -> Option<(u16, u16)> {
    let (vendor, product) = text.split_once(':')?;
    Some((
        u16::from_str_radix(vendor, 16).ok()?,
        u16::from_str_radix(product, 16).ok()?,
    ))
}

/// Lists every /dev/input/eventN node. Nodes we cannot open are still listed,
/// with name and ID read from sysfs, so a permission problem is visible
/// instead of the mouse silently missing from the list.
fn scan_nodes() -> io::Result<Vec<Node>> {
    let mut numbered: Vec<(u32, PathBuf)> = Vec::new();
    for entry in fs::read_dir("/dev/input")? {
        let path = entry?.path();
        let number = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("event"))
            .and_then(|n| n.parse::<u32>().ok());
        if let Some(number) = number {
            numbered.push((number, path));
        }
    }
    numbered.sort();

    let mut nodes = Vec::new();
    for (number, path) in numbered {
        let node = match RawDevice::open(&path) {
            Ok(dev) => Node {
                name: dev.name().unwrap_or("(unnamed)").to_string(),
                vendor: dev.input_id().vendor(),
                product: dev.input_id().product(),
                path,
                device: Ok(dev),
            },
            Err(e) => {
                let sys = PathBuf::from(format!("/sys/class/input/event{number}/device"));
                Node {
                    name: read_sysfs(&sys.join("name")).unwrap_or_else(|| "(unknown)".into()),
                    vendor: read_sysfs_hex(&sys.join("id/vendor")),
                    product: read_sysfs_hex(&sys.join("id/product")),
                    path,
                    device: Err(e),
                }
            }
        };
        nodes.push(node);
    }
    Ok(nodes)
}

fn read_sysfs(path: &Path) -> Option<String> {
    Some(fs::read_to_string(path).ok()?.trim().to_string())
}

fn read_sysfs_hex(path: &Path) -> u16 {
    read_sysfs(path)
        .and_then(|text| u16::from_str_radix(&text, 16).ok())
        .unwrap_or(0)
}

/// Prints the device list and asks which one to use. Skips the question when
/// there is only one candidate.
fn pick_node(mut nodes: Vec<Node>) -> Result<Node, String> {
    if nodes.is_empty() {
        return Err("no input devices found".into());
    }
    if nodes.len() == 1 {
        return Ok(nodes.remove(0));
    }

    println!("Input devices:");
    for (index, node) in nodes.iter().enumerate() {
        let note = match &node.device {
            Ok(_) if node.looks_like_mouse() => "  <- mouse",
            Ok(_) => "",
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => "  [no access]",
            Err(_) => "  [cannot open]",
        };
        println!(
            "  {index:>2}  {:04x}:{:04x}  {}{note}",
            node.vendor, node.product, node.name
        );
    }

    loop {
        print!("Pick the mouse by number: ");
        io::stdout().flush().map_err(|e| e.to_string())?;
        let mut line = String::new();
        let read = io::stdin()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        if read == 0 {
            return Err("no device picked".into());
        }
        match line.trim().parse::<usize>() {
            Ok(index) if index < nodes.len() => return Ok(nodes.remove(index)),
            _ => println!("Enter a number from 0 to {}.", nodes.len() - 1),
        }
    }
}

/// Creates a virtual device that can emit everything the real mouse can
/// (buttons, keys, relative axes, misc), plus the remap target key.
fn build_virtual_device(mouse: &RawDevice) -> io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    if let Some(supported) = mouse.supported_keys() {
        for key in supported.iter() {
            keys.insert(key);
        }
    }
    keys.insert(TO);

    let mut builder = VirtualDevice::builder()?
        .name(VIRTUAL_NAME)
        .input_id(InputId::new(
            BusType::BUS_USB,
            VIRTUAL_VENDOR,
            VIRTUAL_PRODUCT,
            1,
        ))
        .with_keys(&keys)?;
    if let Some(axes) = mouse.supported_relative_axes() {
        builder = builder.with_relative_axes(axes)?;
    }
    if let Some(misc) = mouse.misc_properties() {
        builder = builder.with_msc(misc)?;
    }
    builder.build()
}

fn wait_for_all_released(mouse: &RawDevice) -> io::Result<()> {
    let mut told = false;
    while mouse.get_key_state()?.iter().next().is_some() {
        if !told {
            println!("Release all mouse buttons...");
            told = true;
        }
        sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// Running totals for the latency test from the plan: no dropped events at
/// 1000 Hz, and added delay well under 1 ms.
#[derive(Default)]
struct Stats {
    frames_this_second: u32,
    worst_this_second: Duration,
    total_frames: u64,
    total_delay: Duration,
    worst_ever: Duration,
    frames_over_1ms: u64,
    kernel_drops: u64,
}

/// The main loop. Reads events, collects them into frames (a frame is
/// everything up to a SYN_REPORT, i.e. one report from the mouse), swaps the
/// one remapped button, and writes each frame to the virtual device.
fn forward_events(
    mouse: &mut RawDevice,
    output: &mut VirtualDevice,
    quiet: bool,
) -> io::Result<()> {
    let mut frame: Vec<InputEvent> = Vec::with_capacity(16);
    let mut notes: Vec<String> = Vec::new();
    let mut stats = Stats::default();
    let mut second_started = Instant::now();

    loop {
        // Blocks until the kernel has events. The iterator borrows `mouse`,
        // so copy the batch out before doing anything else.
        let batch: Vec<InputEvent> = mouse.fetch_events()?.collect();

        for event in batch {
            match event.destructure() {
                EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _) => {
                    if !frame.is_empty() {
                        // emit() appends the SYN_REPORT itself.
                        output.emit(&frame)?;
                        frame.clear();
                    }
                    // Kernel timestamp of the frame versus now, after the write.
                    let delay = SystemTime::now()
                        .duration_since(event.timestamp())
                        .unwrap_or_default();
                    stats.frames_this_second += 1;
                    stats.total_frames += 1;
                    stats.total_delay += delay;
                    stats.worst_this_second = stats.worst_this_second.max(delay);
                    stats.worst_ever = stats.worst_ever.max(delay);
                    if delay > Duration::from_millis(1) {
                        stats.frames_over_1ms += 1;
                    }
                }
                EventSummary::Synchronization(_, SynchronizationCode::SYN_DROPPED, _) => {
                    // The kernel buffer overflowed because we read too slowly.
                    stats.kernel_drops += 1;
                }
                EventSummary::Key(_, key, value) => {
                    let out_key = if key == FROM { TO } else { key };
                    frame.push(InputEvent::new(EventType::KEY.0, out_key.code(), value));
                    // value 2 is key auto-repeat; not worth a line.
                    if !quiet && value != 2 {
                        let state = if value == 1 { "down" } else { "up" };
                        if out_key == key {
                            notes.push(format!("  {key:?} {state}"));
                        } else {
                            notes.push(format!("  {key:?} {state}  ->  {out_key:?} {state}"));
                        }
                    }
                }
                EventSummary::RelativeAxis(..) | EventSummary::Misc(..) => frame.push(event),
                // Anything else (LEDs, absolute axes, ...) is not mirrored.
                _ => {}
            }
        }

        // Printing happens after the write so it never sits inside the
        // measured delay.
        for note in notes.drain(..) {
            println!("{note}");
        }
        if second_started.elapsed() >= Duration::from_secs(1) {
            print_stats(&stats);
            stats.frames_this_second = 0;
            stats.worst_this_second = Duration::ZERO;
            second_started = Instant::now();
        }
    }
}

fn print_stats(stats: &Stats) {
    let average_us = stats.total_delay.as_micros() / u128::from(stats.total_frames.max(1));
    println!(
        "[stats] {:>4} frames in the last second, worst {:>4} us | total: {} frames, avg {} us, worst {} us, {} over 1 ms, {} kernel drops",
        stats.frames_this_second,
        stats.worst_this_second.as_micros(),
        stats.total_frames,
        average_us,
        stats.worst_ever.as_micros(),
        stats.frames_over_1ms,
        stats.kernel_drops,
    );
}
