//! One opened input node of a physical mouse.

use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use evdev::raw_stream::RawDevice;
use mousekit_core::Event;
use mousekit_core::event::EV_SYN;

/// The kernel's "your buffer overflowed, events were lost" marker.
const SYN_DROPPED: u16 = 3;

/// What [`Mouse::next_frame`] produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// The buffer holds one complete frame (possibly empty).
    Complete,
    /// The kernel dropped events because we read too slowly. The buffer is
    /// empty and button state may have changed without us seeing it.
    Dropped,
}

pub struct Mouse {
    device: RawDevice,
    /// Events read from the kernel but not yet handed out. One read can
    /// return several frames.
    queue: VecDeque<Event>,
    discarding: bool,
}

impl Mouse {
    /// Opens the node. Does not grab it yet.
    pub fn open(path: &Path) -> io::Result<Mouse> {
        Ok(Mouse {
            device: RawDevice::open(path)?,
            queue: VecDeque::new(),
            discarding: false,
        })
    }

    /// Takes exclusive hold of the node, so the desktop stops seeing its
    /// events directly. Waits until no button is down first: a button held
    /// across the grab would never be seen released by the desktop.
    ///
    /// The kernel drops the grab when this `Mouse` is dropped or the process
    /// dies, which is what makes a crash safe.
    pub fn grab_when_released(&mut self) -> io::Result<()> {
        while self.device.get_key_state()?.iter().next().is_some() {
            sleep(Duration::from_millis(10));
        }
        self.device.grab()
    }

    /// Blocks until a full frame has arrived and puts it in `frame` (cleared
    /// first), without the closing SYN_REPORT.
    ///
    /// An error means the mouse is gone (unplugged or asleep).
    pub fn next_frame(&mut self, frame: &mut Vec<Event>) -> io::Result<Frame> {
        frame.clear();
        loop {
            while let Some(event) = self.queue.pop_front() {
                if event.is_syn_report() {
                    if self.discarding {
                        self.discarding = false;
                        frame.clear();
                        return Ok(Frame::Dropped);
                    }
                    return Ok(Frame::Complete);
                } else if event.kind == EV_SYN && event.code == SYN_DROPPED {
                    // Everything since the last SYN_REPORT, and up to the
                    // next one, is unreliable.
                    self.discarding = true;
                    frame.clear();
                } else if !self.discarding {
                    frame.push(event);
                }
            }
            for event in self.device.fetch_events()? {
                self.queue.push_back(Event {
                    kind: event.event_type().0,
                    code: event.code(),
                    value: event.value(),
                });
            }
        }
    }
}
