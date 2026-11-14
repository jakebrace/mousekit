//! The per-mouse thread: read a frame, remap it, write it.

use mousekit_core::Event;
use mousekit_input::{Frame, Mouse, NodeInfo};

use crate::{Shared, lock, log};

pub(crate) fn run(shared: Shared, device_index: usize, node: NodeInfo, mut mouse: Mouse) {
    let label = format!("{} \"{}\"", node.id, node.name);

    match mouse.grab_when_released() {
        Ok(()) => {
            log(&format!("grabbed {label} at {}", node.path.display()));
            crate::hardware::start_sync(&shared, device_index);
            forward(&shared, device_index, &mut mouse);
            log(&format!("{label} disconnected"));
        }
        Err(error) => log(&format!("cannot grab {label}: {error}")),
    }

    // The mouse can no longer send releases, so let go of anything it held,
    // and free the node so the watcher picks it up when it returns.
    let mut guard = lock(&shared);
    // Reborrow so the fields can be borrowed separately.
    let state = &mut *guard;
    state.engines[device_index].release_all(&mut state.out);
    state.flush();
    state.active.remove(&node.path);
    if let Some(slot) = state.hardware.get_mut(device_index) {
        *slot = None;
    }
    if let Some(retry) = state.hardware_retry.get_mut(device_index) {
        *retry = None;
    }
}

/// Runs until reading fails, which means the mouse went away.
fn forward(shared: &Shared, device_index: usize, mouse: &mut Mouse) {
    let mut frame: Vec<Event> = Vec::with_capacity(16);
    loop {
        // Blocks here, outside the lock, until the mouse does something.
        let kind = match mouse.next_frame(&mut frame) {
            Ok(kind) => kind,
            Err(_) => return,
        };

        let mut guard = lock(shared);
        // Reborrow so the fields can be borrowed separately below.
        let state = &mut *guard;
        // An event means the mouse is awake, which is the moment a failed
        // read of its sensor settings is worth trying again.
        if crate::hardware::retry_is_due(state, device_index) {
            let retry_state = shared.clone();
            std::thread::spawn(move || crate::hardware::sync(&retry_state, device_index));
        }
        match kind {
            Frame::Complete if frame.is_empty() => continue,
            Frame::Complete if state.capturing() => state.capture_frame(&frame),
            Frame::Complete if state.paused => {
                // Paused: mirror the mouse exactly.
                state.out.extend_from_slice(&frame);
                state.out.push(Event::SYN);
            }
            Frame::Complete => {
                let now = state.started.elapsed();
                let engine = &mut state.engines[device_index];
                engine.process_frame(&frame, now, &mut state.out);
                if engine.next_wake().is_some() {
                    // A macro is playing; make sure the timer knows.
                    let _ = state.macro_wake.send(());
                }
            }
            Frame::Dropped => {
                // Button state is unknown now; releasing everything is the
                // safe side of wrong.
                log("the kernel dropped events (daemon too slow?); releasing held keys");
                state.engines[device_index].release_all(&mut state.out);
            }
        }
        state.flush();
    }
}
