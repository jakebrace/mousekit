# Linux Mouse Profile Manager — Project Plan

Nov 12, 2026 · @Jake

## Summary

The app is a Rust background daemon that grabs the mouse at the kernel input layer, rewrites button events according to the active profile, and switches profiles the moment window focus changes. A separate on-demand GUI edits the config. Everything you described is buildable; three parts need a different approach than Logitech's Windows software uses, and one of your goals (Flatpak as the main distribution format) comes with a real compromise.

The decisions that shape the rest of this plan:

| Decision | Choice | Why |
| --- | --- | --- |
| How buttons are remapped | Grab the mouse's evdev device, re-emit through a uinput virtual device | Works on Wayland and X11, and games under Proton see it as ordinary hardware. Every serious Linux remapper works this way. |
| How focus is detected | A small KWin script that pushes focus changes to the daemon over D-Bus; other compositors added later behind one trait | Wayland has no universal "active window" API. Each compositor needs its own backend. CachyOS defaults to KDE Plasma, so KDE ships first. |
| DPI and polling rate | Delegate to `ratbagd` (libratbag) over D-Bus, shown only when the mouse is supported | These are firmware settings behind vendor-specific protocols. Reimplementing them per brand is a separate project. |
| Process model | One binary, two modes: always-on `daemon` (engine + tray) and on-demand `gui` | The daemon stays at a few MB of RAM with no GUI toolkit loaded. A GUI crash cannot interrupt remapping. |
| Language and core crates | Rust: `evdev`, `zbus`, `ksni`, `tokio`, `serde` + TOML | Matches your lightweight goal; these cover input, D-Bus, tray and config. |
| Packaging order | Native build and AUR package first, Flatpak second | A Flatpak cannot install the udev rule the app needs, so Flatpak users get a one-time manual setup step. See Hurdles. |

The piece to prove before anything else is the focus-switch path on your machine: KWin script → D-Bus → profile swap, tested by alt-tabbing out of Deadlock. Milestone 0 is three throwaway spikes that de-risk this and the other two unknowns.

## Prior art

The remapping engine you need already exists in several open-source tools; what does not exist is a mouse-first GUI with reliable focus-based profile switching. That gap is the project. Read these before writing code, because they have already solved the hard parts.

| Tool | What it does | What it lacks for your goal | Borrow |
| --- | --- | --- | --- |
| [xremap](https://github.com/xremap/xremap) | Rust remapper on evdev + uinput with per-application rules and a separate client per compositor (KDE, GNOME, Hyprland, niri, COSMIC, wlroots, X11) | Config-file only, keyboard-centric, no tray or GUI | Its compositor client structure and its no-sudo udev setup doc |
| [makima](https://github.com/cyber-sushi/makima) | Rust daemon remapping mice, keyboards and controllers, with layouts that switch on the active window | No GUI; window switching covers Hyprland, Sway, Niri and X11 natively, and on Plasma it shells out to `kdotool` | Its handling of scroll and relative-axis events |
| [input-remapper](https://github.com/sezanzeb/input-remapper) | Python GUI for remapping any input device, with macros | No focus-based profile switching that I know of (verify); needs a root service | Its capture-a-button UI flow and macro model |
| [Piper](https://github.com/libratbag/piper) + libratbag | GUI over the `ratbagd` D-Bus daemon: DPI, polling rate, onboard button maps, LEDs | Writes to mouse firmware only; nothing switches with window focus | Use `ratbagd` directly as your DPI and polling backend |
| [kdotool](https://github.com/jinliu/kdotool) | Rust CLI that loads a generated KWin script over D-Bus to query windows | Query tool, not a remapper | The exact KWin scripting D-Bus calls, already written in Rust |

Check each project's license before copying code rather than ideas. The engine itself is small enough (a few hundred lines) that writing your own is reasonable, and it keeps the license choice yours.

## Hurdles

Four constraints decide the design. Each has a workable answer, but two of them change what you described.

### 1. Not every mouse button sends a signal

Your capture flow (press a button, the app records what arrives) only works for buttons that emit an event to the host. On many gaming mice, the DPI-shift, DPI-cycle and profile buttons are handled inside the mouse by default and send nothing. Logitech's Windows software gets around this by switching the mouse into a host-controlled mode over its vendor protocol.

- **Check first:** run `sudo evtest`, pick the mouse, and press every button. Any button that prints nothing is invisible to the app as-is.
- **Fix for silent buttons:** use Piper once to assign each one a distinct button or key in the mouse's onboard memory. After that, capture works normally.
- **Related:** side-button grids on some mice arrive as keyboard keys on a second input node. The app must treat every node belonging to one physical mouse as a single device and grab them all.

### 2. Wayland has no universal active-window API

Focus detection is compositor-specific, so it sits behind a `FocusBackend` trait with one implementation per desktop.

| Desktop | Mechanism | When |
| --- | --- | --- |
| KDE Plasma (CachyOS default) | KWin script subscribed to `workspace.windowActivated`, calling the daemon with `callDBus` | v1 |
| Hyprland, niri, Sway | Each compositor's IPC event socket | Later |
| Other wlroots compositors | `wlr-foreign-toplevel-management` protocol | Later |
| GNOME | Requires a companion Shell extension; no other route | Later, if wanted |
| X11 | `_NET_ACTIVE_WINDOW` property | Later |

The KWin route is push-based, so switching is event-driven with no polling. The same script can list `workspace.windowList()` for your live-application picker, returning each window's `resourceClass`, caption, PID and desktop file name. That list is what solves your launcher-versus-real-process concern: profiles match on what the compositor reports for the live window, not on a file path.

Two caveats to test in Milestone 0:

- **Proton games** report a window class such as `steam_app_<appid>`, not an exe name. Match on class first; exe name is a secondary matcher.
- **Gamescope** hides the game from KWin. A game launched inside gamescope shows up as one `gamescope` window, so per-game switching cannot see through it.

### 3. DPI and polling rate are firmware settings

Neither exists at the input-event layer. They are set by vendor-specific commands (Logitech HID++, Razer, SteelSeries and so on). `ratbagd` already implements these for many mice and exposes resolution and report rate per device over D-Bus, so the app reads the allowed values and writes the chosen one there.

- If `ratbagd` is missing or the mouse is unsupported, the DPI and polling controls are hidden and everything else still works.
- Run `ratbagctl list` on your machine to see whether your mouse is covered.
- libratbag's last tagged release I found is 0.18 from September 2024, so new mice may lag.

**For the G502 X specifically,** do not count on ratbagd. I found user reports of ratbagd and Piper failing to detect the G502 X behind its Lightspeed receiver ([report](https://redlib.hbubli.cc/r/linux_gaming/comments/1evjpqg/ratbagd_doesnt_detect_logitech_g502_x)) and of the G502 X Plus not being recognised by Piper. So the `hw` crate becomes a trait with more than one backend, chosen per device:

| Backend | Covers | Cost |
| --- | --- | --- |
| ratbagd | Many brands, when the device has a data file | Free if it works; may not for this mouse |
| Direct HID++ 2.0 over hidraw | Logitech only: DPI, report rate, onboard-profile mode, and button diversion | You implement a handful of HID++ features yourself; [Solaar](https://github.com/pwr-Solaar/Solaar) is the reference implementation to read |

Direct HID++ is the likely answer for your mouse, and it pays twice. The same protocol can divert buttons to the host, which makes the silent DPI and profile buttons capturable without first remapping them in onboard memory. That is how Logitech's own software does it.

Three things follow:

- Spike (c) should also run `solaar show`. If Solaar can read and set DPI and report rate on your G502 X, the HID++ route is confirmed and you have its feature list.
- Solaar's docs note that polling rate only takes effect once the mouse's Onboard Profiles mode is disabled. Expect the same in your implementation: the app switches the mouse to host mode before applying settings.
- HID++ needs access to the mouse's `hidraw` node, so the udev rule gains one more line, and the first-run check covers it.

Keep this work in Milestone 6, but read Solaar's HID++ code during Milestone 0 so the capture design accounts for diverted buttons arriving over HID++ instead of evdev.

### 4. Flatpak cannot grant itself input access

This is the one place your plan needs to bend. Reading `/dev/input/event*` and writing `/dev/uinput` requires host permissions that are set by a udev rule, and a Flatpak cannot install udev rules or run anything as root on the host.

- The Flatpak needs `--device=all` to see device nodes at all. The narrower `--device=input` arrived in Flatpak 1.16, but Flathub discouraged it for compatibility as of the threads I found; recheck current Flathub policy before publishing.
- Even with that flag, the user must install the udev rule once by hand. The app should detect missing access on first run and show the exact command.
- The sandbox cannot see host processes, so PID-to-exe lookup in the app picker is unavailable there. Window class and desktop file name from KWin still work.
- The Flatpak also needs `--talk-name=org.kde.KWin`, the tray watcher name, and `--system-talk-name=org.freedesktop.ratbag1`.

Recommendation: make the native package (AUR, which can ship the udev rule properly) the primary install path, and offer the Flatpak as a second option with a first-run setup screen. Flatpak-only distribution would mean every user hits a manual terminal step.

### Smaller gotchas

- **Pointer feel changes.** The desktop sees a new virtual mouse, so KDE applies default acceleration to it. Set the acceleration profile for the virtual device once in System Settings, and keep the virtual device's name and IDs stable so the setting persists. This matters for aim.
- **Stuck keys on profile switch.** If a button is held while focus changes, the release must emit the key-up for whatever the press emitted, not the new profile's binding. Track per-button press state.
- **Scroll arrives twice.** Modern kernels send both `REL_WHEEL` and `REL_WHEEL_HI_RES`. Remapping scroll means consuming both.
- **"Literal mouse button" bindings have a ceiling.** The virtual device can emit any Linux button code, but games generally recognise only left, right, middle, back and forward. For extra distinct bindable inputs, emitting F13 to F24 is the usual trick; confirm Deadlock accepts them.
- **Crash safety.** The kernel releases the grab when the daemon's file handle closes, so a crash returns the mouse to normal behaviour on its own.

## Architecture

One always-running daemon owns the mouse, the active profile and the tray icon; everything else talks to it over the session D-Bus.

&#91;embedded content: architecture · daemon, its three inner parts, and four neighbours\]

Mouse events flow left to right through the engine. Focus changes arrive from the bottom left and replace the active profile, which is the only state the engine consults per event.

- **Event path.** One task per grabbed device node reads events, resolves each against the active profile, and writes the result to a single virtual device carrying both mouse and keyboard capabilities. Pointer motion is forwarded untouched in the same sync batch.
- **Focus path.** At startup the daemon loads its embedded KWin script through `org.kde.kwin.Scripting`. The script calls a `FocusChanged(class, title, pid)` method on the daemon each time the active window changes. The daemon matches the class against profile rules and swaps an atomic pointer to the new profile.
- **Control path.** The GUI and the CLI use the same D-Bus interface: list devices, start and stop capture, list live windows, get and set profiles, reload config, pause. The daemon is the only process that writes config files.
- **Hardware path.** DPI and polling changes go from the daemon to `ratbagd` on the system bus, and only when the user changes them, not per profile.
- **Why two modes of one binary.** The daemon links no GUI toolkit, so its resident memory stays small. The tray's "Open settings" item spawns `<app> gui` as a child process, which exits when its window closes.

## Data model

Config is plain TOML under `~/.config/<app>/`, with device-level settings separate from profiles, which is the split you asked for: DPI and polling live with the device, button bindings live with the profile. Hand-editable files also mean the daemon is usable before any GUI exists.

`config.toml` holds global settings and one block per known mouse:

```toml
[settings]
autostart = true
start_in_tray = true

[[device]]
ids = ["046d:409f"]         # USB vendor:product, never the eventN path; list every ID the mouse appears as
name = "Example Gaming Mouse"
dpi = 1600                  # universal; applied through ratbagd when supported
polling_hz = 1000

  [[device.control]]        # one entry per button learned in capture mode
  label = "M4"
  event = "BTN_SIDE"

  [[device.control]]
  label = "Scroll Up"
  event = "REL_WHEEL:+"
```

Each profile is its own file in `profiles/`, so profiles can be shared or version-controlled one at a time:

```toml
# profiles/deadlock.toml
name = "Deadlock"

[match]
window_class = ["steam_app_1422450"]   # confirm the real value with the live picker
exe = []                                # optional secondary matcher

[bindings]
M4 = { type = "key", keys = ["KEY_LEFTALT", "KEY_TAB"] }
M5 = { type = "mouse_button", button = "BTN_MIDDLE" }
G7 = { type = "macro", name = "buy-combo", mode = "once" }
G9 = { type = "disabled" }
```

Rules the engine follows:

- **Bindings refer to labels, not raw codes.** The device's control list translates label to event, so a profile survives re-capturing a button.
- **Profiles overlay the default.** A control not listed in the active profile uses the default profile's binding; one not listed there passes through unchanged. An explicit `passthrough` type restores raw behaviour inside a profile.
- **One app, one profile.** The GUI refuses to add a window class that another profile already claims. No focused match means the default profile.
- **Macros are stored once** in `macros/<name>.toml` as a list of key-down, key-up and delay steps, and are referenced by name from any profile.

## Feature specs

Each feature below maps to something you asked for, with the behaviour pinned down enough to hand to Claude Code.

### Button capture

1. The GUI asks the daemon to enter capture mode for the selected device. Remapping pauses for that device while capture is active.
2. The daemon reports the next event that is not pointer motion: a button press, a scroll tick with its direction, or a key from one of the mouse's secondary nodes.
3. The GUI proposes a label for standard codes (`BTN_LEFT` as LMB, `BTN_RIGHT` as RMB, `BTN_MIDDLE` as M3, `BTN_SIDE` as M4, `BTN_EXTRA` as M5) and lets you rename it.
4. The control is saved to the device's list. Capturing an event that already has a label selects the existing entry instead of duplicating it.

### Binding types

| Type | Emits | Notes |
| --- | --- | --- |
| Key or chord | One key, or modifiers plus a key, held for as long as the button is held | Covers Alt+Tab, Ctrl+Tab and any shortcut; ship a preset list of common ones |
| Mouse button | A literal button code such as `BTN_MIDDLE` | The "bind it as real mouse button 3" case; games see a mouse button, not a key |
| Macro | A stored sequence | Modes: once per press, repeat while held, toggle |
| Passthrough | The original event | Default for anything unbound |
| Disabled | Nothing | Not available for left click, which is locked to passthrough |

### Macros

Record macros inside the GUI window using its normal key events while the window has focus. This avoids giving the app read access to your keyboard device, which would otherwise be a keylogging-grade permission. The recorder stores each key-down and key-up with the measured delay, and the editor lets you fix delays or set them all to a constant. Whether timed macros are allowed in a given competitive game is that game's rule to check.

### Profiles and the app picker

- Create, duplicate, rename and delete profiles; the default profile cannot be deleted.
- **Add from running windows (primary path):** the daemon asks the focus backend for the live window list and the GUI shows icon, title and window class. Picking one stores the class.
- **Add from file (secondary path):** choosing a `.desktop` file or executable stores its desktop ID or exe name. Flag these as unverified until the app has been seen running once.
- The tray menu shows the active profile and offers a manual override, for cases where detection picks wrong.

### Tray and autostart

- The tray icon uses the StatusNotifierItem protocol through the `ksni` crate. Menu: open settings, active profile, pause remapping, quit.
- Closing the settings window ends only the GUI process; the daemon keeps running.
- Autostart, native install: write a `.desktop` file to `~/.config/autostart/`. Flatpak: request it through the Background portal using the `ashpd` crate.

### Visual mouse picker (later)

The list-based editor is v1. The clickable mouse diagram is a second view over the same data: an SVG per supported mouse with a hotspot per control label. Because bindings key off labels, adding the diagram needs no engine or config change.

## Repo layout

Use one Cargo workspace with the engine as a library crate that has no GUI or D-Bus dependencies, so it can be unit-tested with synthetic events and reused by both modes.

```text
<app>/
├── Cargo.toml                 # workspace
├── CLAUDE.md                  # project rules for Claude Code
├── README.md
├── LICENSE
├── crates/
│   ├── core/                  # config types, binding resolution, macro model (pure logic)
│   ├── input/                 # evdev discovery, grab, uinput virtual device, hotplug
│   ├── focus/                 # FocusBackend trait + kde/ (later hyprland/, wlroots/, x11/)
│   │   └── kde/focus.js       # the KWin script, embedded with include_str!
│   ├── hw/                    # ratbagd D-Bus client for DPI and polling
│   ├── daemon/                # event loop, D-Bus service, tray, autostart
│   ├── gui/                   # settings window; talks to the daemon over D-Bus only
│   └── cli/                   # the single binary: `<app> daemon | gui | status | capture`
├── packaging/
│   ├── udev/70-<app>.rules
│   ├── arch/PKGBUILD
│   ├── flatpak/io.github.<user>.<App>.yml
│   └── desktop/               # .desktop file, icon, AppStream metainfo
├── spikes/                    # Milestone 0 throwaway programs
└── .github/workflows/ci.yml   # fmt, clippy, test, release build
```

The GUI toolkit is confined to `crates/gui`, so that choice can change without touching anything else. My suggestion is `iced`: pure Rust, Wayland-native, and no system GUI libraries to bundle. `egui` is the fallback if iced's API churn slows you down; it is quicker to get working but looks less like a desktop app. Pin whichever you choose to an exact version.

## Milestones

Build the headless daemon first and the GUI fourth: by the end of Milestone 2 you have auto-switching profiles driven by hand-edited TOML, which is the feature you called most important. Each milestone ends with a test you can run by hand.

| # | Milestone | Scope | Done when |
| --- | --- | --- | --- |
| 0 | Spikes | Three throwaway programs: (a) grab the mouse, re-emit through uinput, swap one side button for a key; (b) KWin script that reports focus changes over D-Bus; (c) `evtest` and `ratbagctl list` against your mouse | Swapped button works in Deadlock; focus changes print within a frame when alt-tabbing out of a fullscreen game; you know which buttons are silent and whether ratbagd supports the mouse |
| 1 | Engine | `core` + `input` crates, TOML loading, default profile, device hotplug, `<app> daemon` | Default-profile bindings work across a mouse unplug and replug; killing the daemon restores normal mouse behaviour |
| 2 | Auto-switch | `focus` crate with the KDE backend, profile matching, held-button handling | Browser and Deadlock profiles swap on alt-tab with no stuck keys |
| 3 | Service shell | Daemon D-Bus API, tray icon, pause toggle, autostart, `status` and `capture` CLI commands | Daemon starts at login, shows the active profile in the tray, and `capture` prints a pressed button |
| 4 | GUI v1 | Device page with capture, list-based binding editor, profile manager, live-window picker | A new profile for a new game can be made end to end without opening a text editor |
| 5 | Macros | Recorder, editor, three playback modes | A recorded macro plays back with correct timing in all three modes |
| 6 | DPI and polling | `hw` crate against ratbagd, controls on the device page | Changing DPI in the app changes it on the mouse and survives reboot |
| 7 | Packaging | udev rule, AUR PKGBUILD, Flatpak manifest with first-run permission check, CI release builds | Clean install works from the AUR package and from the Flatpak on a fresh user account |
| 8 | Polish and reach | Visual mouse picker, Hyprland and wlroots backends, X11 backend | Second compositor passes the Milestone 2 test |

Milestone 0 can change the plan. If spike (b) shows unacceptable switch latency or misses fullscreen transitions, the focus design needs rethinking before any engine work.

## Dev setup

Use VS Code with the official Claude Code extension and `rust-analyzer`. The extension is Anthropic's own, bundles the Claude Code CLI, and gives a side panel with inline diff review, so the whole loop stays in the editor ([comparison](https://lumadock.com/tutorials/claude-code-vscode-jetbrains), [editor roundup](https://www.secondtalent.com/resources/claude-code-editors/)).

| Editor | Claude Code integration | Trade-off for this project |
| --- | --- | --- |
| VS Code | Official extension, CLI bundled | Best-supported path; rust-analyzer is solid. Recommended. |
| JetBrains | Official plugin, still labelled beta; CLI installed separately | The supported-IDE lists I found do not name RustRover; confirm before choosing it |
| Zed | Runs Claude Code as an external agent over ACP | Fast native editor with good Rust support, but a less direct integration |

On Arch-based systems the repo `code` package is the open-source build, which uses a different extension registry. If the Claude Code extension does not appear there, install Microsoft's build from the AUR instead.

### One-time system setup on CachyOS

Package names are from memory; confirm with `pacman -Ss` before running.

```bash
sudo pacman -S --needed rustup base-devel evtest libratbag piper flatpak-builder
rustup default stable
rustup component add clippy rustfmt rust-analyzer

# load uinput at boot
echo uinput | sudo tee /etc/modules-load.d/uinput.conf
sudo modprobe uinput
```

A starting-point udev rule that grants the logged-in user access without adding you to the `input` group. Treat it as a draft to validate in Milestone 0:

```text
# /etc/udev/rules.d/70-<app>.rules  (must sort before 73-seat-late.rules for uaccess to apply)
KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess", OPTIONS+="static_node=uinput"
SUBSYSTEM=="input", KERNEL=="event*", ENV{ID_INPUT_MOUSE}=="1", TAG+="uaccess"
```

```bash
sudo udevadm control --reload-rules && sudo udevadm trigger
getfacl /dev/uinput        # your user should be listed
```

Two things to know about that rule:

- A mouse's secondary keyboard-type node is not tagged `ID_INPUT_MOUSE`, so mice with key-emitting buttons need an extra line matching their vendor and product ID. The first-run setup screen can generate it.
- Write access to `/dev/uinput` lets any program in your session inject input. That is inherent to every tool in this category; it is the reason the rule is scoped to the active local user rather than made world-writable.

### GitHub

Create the repo empty, let Claude Code scaffold the workspace, and add CI in Milestone 1 (`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`). Hardware-dependent code cannot run in CI, which is another reason the `core` crate stays pure logic.

## Adjustments before starting

A review of the plan found seven gaps. None blocks Milestone 0, but each changes a spec or a done-test, so they are recorded here for Claude Code.

| Gap | Why it matters | Adjustment |
| --- | --- | --- |
| Silent buttons block capture until HID++ diversion exists (Milestone 6) | The DPI and profile buttons cannot be bound for five milestones | Resolved: every button in use is already bound in the mouse's onboard memory. Confirm in spike (c) with evtest that each one prints an event, and note any two that print the same event. |
| KWin script lifetime | A script loaded over D-Bus lasts only until KWin restarts, and a stale copy from a previous daemon run can block reloading | On start: unload by name, load, and have the script report the current window immediately. Watch the `org.kde.KWin` bus name and reload when its owner changes. |
| Focus flapping | The app launcher, task switcher, panels and lock screen take focus briefly and would flip the profile to default and back | Ignore focus on shell and special window types and keep the previous profile. Use the default profile, with macros off, while the session is locked. |
| No latency number | "Feels fine" is not a test for a competitive player | Spike (a) logs kernel timestamp in versus write time out, and counts events per second. Pass: no dropped events at 1000 Hz and added delay well under one polling interval (1 ms). |
| No escape hatch | A bad binding on left click can leave the desktop unusable by mouse | `<app> pause` CLI command bound to a KDE global keyboard shortcut from Milestone 1. Left click is locked: the engine always passes it through, the config loader rejects any binding for it, and the GUI does not offer it. |
| No test strategy for the event path | Hardware code cannot run in CI, and regressions here are stuck keys | Record real evdev traces to files and replay them through `core` in unit tests. Add a local-only integration test that creates a fake mouse with uinput and asserts on the virtual device's output. |
| Scope of first Rust project | Async, D-Bus and evdev together are a steep start if Rust is new, and generated code you cannot review is a risk | Keep the engine synchronous (one thread per device, no async) until Milestone 3 needs D-Bus. Have Claude Code explain each module before moving on. |

DPI and polling do not need to wait for Milestone 6 either: set them once with the same Windows tool and they persist in the mouse's onboard memory.

## Milestone 0 results

Recorded Nov 13, 2026. All three spikes ran on the target machine (Plasma 6.8.2 on Wayland) and Jake confirmed they behaved as intended. Where a result differs from the text above, this section wins.

| Finding | Detail | Effect on the plan |
| --- | --- | --- |
| Device ID | The kernel exposes the mouse over its receiver as `046d:409f` ("Logitech G502 X LS"), not the receiver's own `046d:c547`. Solaar reports the wired USB ID as `c098` (not yet seen plugged in). | A device block has `ids = [...]`, a list, instead of a single `id`. |
| One input node | The mouse is a single evdev node carrying both mouse and keyboard capabilities. | No second node to grab for this mouse. Multi-node support stays a later concern. |
| Spike (a), grab and remap | Mirroring through uinput with `BTN_SIDE` swapped for `KEY_F13` worked, including the latency check. | The evdev + uinput design stands. `evdev` 0.13.2 is the crate. |
| Spike (b), focus | The KWin script loads over D-Bus, reports the current window at once, and delivered focus changes with 0 ms measured delay. Unload by name on start works. | The focus design stands. The Deadlock window class and what the launcher, task switcher and panels report were not written down; capture them at the start of Milestone 2. |
| udev rule | The draft rule works as written. `/dev/uinput` was already accessible through another package's rule. `udevadm trigger` returns before rules are applied, so follow it with `udevadm settle` in instructions. | Rule lives in `packaging/udev/70-mouse-profiles.rules`. |
| ratbagd | `ratbagctl list` reports no devices. | ratbagd is not a backend for this mouse. |
| HID++ | `solaar show` reads DPI, report rate and onboard-profile mode. The hidraw node is accessible through Solaar's udev rule. | Direct HID++ is the Milestone 6 route. Packaging needs its own hidraw rule for users without Solaar. |
| Onboard profiles | Solaar shows the mouse in host mode with onboard profiles disabled, so bindings stored in onboard memory are inactive while Solaar manages it. | Contradicts the "already bound in onboard memory" adjustment unless every button still emits its own event. Re-check if a button turns out silent or duplicated. |
| Button diversion | The mouse lists no reprogrammable-controls feature (0x1B04). It has Mouse Button Spy (0x8110) instead. | Hurdle 3's claim that HID++ can divert silent buttons is unproven for this mouse. Investigate in Milestone 6. |
| F13 as a spare key | With the default keyboard layout, F13 reaches games as the media key `XF86Tools`, and Deadlock registers nothing. Numpad keys (`KEY_KP7`) bind in Deadlock. KDE's layout option "Use F13-F24 as usual function keys" (`fkeys:basic_13-24`) is now enabled on this machine; whether Deadlock then accepts F13 is untested. | Suggest numpad keys as the default spare keys. The GUI's preset list should warn about F13-F24 unless the layout option is on. |
| zbus | `zbus` 6 (released Oct 2026) removed the blocking API. The tray crate `ksni` 0.3.7 still depends on `zbus` 5. | The project uses `zbus` 5.19 everywhere, with its blocking API, so the daemon carries one copy of the library and stays synchronous. Move to 6 when `ksni` does. |

## Open questions

The first three need answers from your machine before Milestone 1; the rest are choices that can wait.

- [ ] **Which mouse (or mice) is this for?** Logitech G502 X is the main target, with other mice later. Answered: G502 X Lightspeed on its USB receiver. Consequences: the system sees the receiver's USB ID (reported as 046d:c547; confirm with lsusb), the mouse shows a different ID when plugged in by cable, so the device entry must accept both; HID++ commands go through the receiver with a device index; and the mouse sleeping or reconnecting must be handled as hotplug.
- [ ] **Are you on Plasma Wayland?** Answered: yes, as the plan assumed. Run `echo $XDG_CURRENT_DESKTOP $XDG_SESSION_TYPE` to confirm.
- [ ] **Do you launch games through gamescope?** Answered: probably not, games run straight from Steam. Confirm in Steam that no game has gamescope in its launch options; spike (b) will show it either way.
- [ ] **Project name and app ID.** The Flatpak ID (`io.github.<user>.<App>`) is hard to change after release, so settle the name before Milestone 7.
- [ ] **License.** GPL-3.0 matches most tools in this space; MIT or Apache-2.0 is fine too since ratbagd is reached over D-Bus, not linked.
- [ ] **GUI toolkit.** iced or egui, per the Repo layout note.
- [ ] **Overlay or independent profiles?** The plan has profiles inherit unbound controls from the default. Logitech's software treats each profile as standalone; say if you prefer that.
- [ ] **Multiple mice.** Should a profile's bindings apply per device, or to whichever mouse is connected? The data model above is per-device labels with shared profiles.

## Claude Code handoff

Export this doc as Markdown into the repo as `docs/PLAN.md`, then drop the file below in the repo root. Claude Code reads `CLAUDE.md` at the start of every session, so it carries the rules that should never be re-argued.

```text
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
```

First prompt for the first session:

```text
Read CLAUDE.md and docs/PLAN.md. We are on Milestone 0. Scaffold the Cargo
workspace from the Repo layout section with empty crates, then build spike (a)
in spikes/grab-remap: list input devices, let me pick the mouse, grab it, mirror
every event to a uinput virtual device, and replace BTN_SIDE with KEY_F13.
Tell me how to run it and how to confirm it works. Stop there.
```

Then spike (b), the KWin focus reporter, as its own session. Keeping each spike to one session makes it easy to throw away.

## Sources

- [xremap](https://github.com/xremap/xremap) and its [crate docs](https://docs.rs/crate/xremap/0.15.5): compositor feature list, no-sudo udev setup
- [makima](https://github.com/cyber-sushi/makima): supported compositors for window-based switching, kdotool on Plasma
- [kdotool source](https://docs.rs/crate/kdotool/0.2.3/source/src/lib.rs): KWin `loadScript` D-Bus calls
- [KDE discussion on KWin scripts calling out over D-Bus](https://discuss.kde.org/t/feature-request-implement-wayland-protocol-in-kwin-to-support-activitywatch/30499)
- [ratbagd D-Bus interface](https://pkgbuild.com/~ffy00/documentation/libratbag/dbus.html) and [libratbag 0.18 announcement](https://lists.freedesktop.org/archives/input-tools/2024-September/001590.html)
- [Flatpak sandbox permissions](https://flatpak-docs.readthedocs.io/en/latest/sandbox-permissions.html), [Flatpak issue on /dev/uinput](https://github.com/flatpak/flatpak/issues/5459), [Flathub thread on device=input](https://discourse.flathub.org/t/support-for-device-input/6645), [Flatpak 1.16 release notes](https://feaneron.com/2025/01/14/flatpak-1-16-is-out/)
- [ksni](https://github.com/iovxw/ksni): StatusNotifierItem tray in Rust
- [Claude Code in VS Code, JetBrains and Zed](https://lumadock.com/tutorials/claude-code-vscode-jetbrains) and [editor comparison](https://www.secondtalent.com/resources/claude-code-editors/)

These were read as search results, not opened in full. Claims marked "verify" in the text rest on my own knowledge rather than a source.
