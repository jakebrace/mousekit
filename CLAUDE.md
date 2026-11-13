# CLAUDE.md

## What this is
A Linux mouse profile manager in Rust: a daemon that grabs a mouse via evdev,
remaps buttons through a uinput virtual device, and switches profiles when the
focused window changes. Full plan: docs/PLAN.md. Read it before large changes.

## Hard rules
- Target: CachyOS, KDE Plasma 6 on Wayland first. Other compositors go behind
  the FocusBackend trait; never add compositor-specific code outside crates/focus.
- crates/core has no I/O, no D-Bus, no evdev. It must be testable with plain
  cargo test. Put logic there whenever possible.
- crates/gui talks to the daemon only over its D-Bus interface. It never opens
  devices or reads config files directly.
- Never run as root and never suggest sudo for the app itself. Device access
  comes from the udev rule in packaging/udev.
- Identify devices by vendor:product ID, never by /dev/input/eventN path.
- On profile switch, a held button must release whatever its press emitted.
- Before adding a crate, check crates.io for its current version and whether it
  is maintained. Pin GUI toolkit versions exactly.
- Do not copy code from xremap, makima, kdotool or input-remapper without
  checking the license first. Reading them for approach is encouraged.

## Workflow
- Work one milestone from docs/PLAN.md at a time. Do not start the next until
  its "Done when" test passes and I confirm.
- Run cargo fmt, cargo clippy -- -D warnings and cargo test before saying a
  task is finished.
- Anything needing real hardware (grab, uinput, KWin) cannot be verified by
  you. Give me the exact command to run and what output to expect.
- Keep spikes/ throwaway. Nothing in crates/ depends on it.