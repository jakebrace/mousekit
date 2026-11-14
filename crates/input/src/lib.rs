//! Everything that touches /dev/input and /dev/uinput.
//!
//! - [`list_nodes`]: which input nodes exist, with their vendor:product IDs.
//! - [`Mouse`]: one opened node; grab it and read frames of events from it.
//! - [`VirtualMouse`]: the uinput device all output is written to.
//!
//! No remapping logic lives here. This crate only moves events between the
//! kernel and the types in `mousekit-core`.

mod mouse;
mod nodes;
mod virtual_mouse;

pub use mouse::{Frame, Mouse};
pub use nodes::{NodeInfo, list_nodes};
pub use virtual_mouse::VirtualMouse;
