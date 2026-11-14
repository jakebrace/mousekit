//! The uinput device that all output goes to.

use std::io;
use std::path::PathBuf;

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, BusType, InputEvent, InputId, KeyCode, MiscCode, RelativeAxisCode};
use mousekit_core::event::{EV_KEY, EV_MSC, EV_REL};
use mousekit_core::{Event, Key};

/// Fixed identity. The desktop stores pointer settings (acceleration) per
/// device name and ID, so these must not change between runs.
/// 0x1209 is the pid.codes open-source vendor ID; the product ID is a
/// placeholder to settle before any public release.
const VENDOR: u16 = 0x1209;
const PRODUCT: u16 = 0x5351;

pub struct VirtualMouse {
    device: VirtualDevice,
    /// Reused conversion buffer.
    buffer: Vec<InputEvent>,
}

impl VirtualMouse {
    /// The name the real daemon uses.
    pub const NAME: &'static str = "mousekit virtual mouse";

    /// Creates the device. It can emit every key [`VirtualMouse::can_emit`]
    /// accepts plus pointer motion and both scroll wheels, so it never has to
    /// be rebuilt when a mouse or a profile changes.
    pub fn create(name: &str) -> io::Result<VirtualMouse> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for key in Key::all().filter(|key| VirtualMouse::can_emit(*key)) {
            keys.insert(KeyCode::new(key.0));
        }
        let axes = AttributeSet::<RelativeAxisCode>::from_iter([
            RelativeAxisCode::REL_X,
            RelativeAxisCode::REL_Y,
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_HWHEEL,
            RelativeAxisCode::REL_WHEEL_HI_RES,
            RelativeAxisCode::REL_HWHEEL_HI_RES,
        ]);
        let misc = AttributeSet::<MiscCode>::from_iter([MiscCode::MSC_SCAN]);

        let device = VirtualDevice::builder()?
            .name(name)
            .input_id(InputId::new(BusType::BUS_USB, VENDOR, PRODUCT, 1))
            .with_keys(&keys)?
            .with_relative_axes(&axes)?
            .with_msc(&misc)?
            .build()?;
        Ok(VirtualMouse {
            device,
            buffer: Vec::new(),
        })
    }

    /// Whether the virtual device is able to emit this key or button.
    ///
    /// Keyboard keys and mouse buttons are in. Joystick, gamepad and tablet
    /// buttons are deliberately out: declaring them would make the desktop
    /// treat the device as a controller or tablet instead of a mouse. (Even
    /// the four D-pad buttons at 0x220 are enough for udev to stop
    /// classifying the device as a mouse.)
    pub fn can_emit(key: Key) -> bool {
        matches!(key.0, 0x001..0x100 | 0x110..0x120 | 0x160..0x220 | 0x230..0x2c0)
    }

    /// Writes engine output: whole frames, each ended by a SYN_REPORT.
    /// Events the device cannot emit are ignored by the kernel.
    pub fn emit(&mut self, events: &[Event]) -> io::Result<()> {
        // evdev's emit() appends the final SYN_REPORT itself.
        let Some((last, rest)) = events.split_last() else {
            return Ok(());
        };
        debug_assert!(last.is_syn_report(), "engine output ends with SYN_REPORT");
        self.buffer.clear();
        self.buffer.extend(
            rest.iter()
                .filter(|event| {
                    matches!(event.kind, EV_KEY | EV_REL | EV_MSC) || event.is_syn_report()
                })
                .map(|event| InputEvent::new(event.kind, event.code, event.value)),
        );
        self.device.emit(&self.buffer)
    }

    /// The /dev/input/eventN path of this device, for status output and tests.
    pub fn node_path(&mut self) -> io::Result<PathBuf> {
        match self.device.enumerate_dev_nodes_blocking()?.next() {
            Some(path) => path,
            None => Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the virtual device has no event node yet",
            )),
        }
    }
}
