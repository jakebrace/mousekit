# MouseKit

MouseKit remaps the buttons on your mouse and switches between sets of
bindings as you move between applications. The same thumb button can be a
spare key in a game and "back" in your browser, without you touching anything.

It is built for Linux on Wayland, where vendor tools such as Logitech G HUB
do not exist.

## What it does

- **Remaps buttons and scroll directions** to a key, a shortcut such as
  Alt+Tab, another mouse button, a macro, or nothing.
- **Switches profiles automatically** when a chosen application takes focus.
- **Records macros** from your keyboard and plays them once per press, while
  the button is held, or as an on/off toggle.
- **Sets DPI and report rate** on supported Logitech mice.
- **Works in games**, including Windows games running through Proton: the
  remapped input comes from an ordinary virtual device.
- **Stays out of the way**: a small background service with a tray icon, and a
  settings window that only runs while you have it open.

## Requirements

| Feature | Needs |
| --- | --- |
| Button remapping, macros | Any Linux desktop with `uinput` (every mainstream kernel) |
| Automatic profile switching | KDE Plasma 6 |
| Tray icon | A desktop with StatusNotifierItem support (Plasma has it built in) |
| DPI and report rate | A Logitech mouse that speaks HID++ 2.0 |

On other desktops everything except automatic switching works; you pick the
profile by hand from the tray menu or with `mousekit profile <name>`.

MouseKit has been developed and tested with a Logitech G502 X Lightspeed on
CachyOS with Plasma 6 on Wayland. Other mice should work for remapping. Other
setups are untested.

## Install

### Arch Linux, CachyOS and other Arch-based systems

Build and install the package from this repository:

```sh
git clone https://github.com/jakebrace/mousekit.git
cd mousekit/packaging/arch
makepkg -si
```

The package installs the `mousekit` command, a menu entry, and the udev rule
that lets your user (and only your user) read the mouse and create the virtual
device. MouseKit itself never runs as root.

### Other distributions

Build with a recent stable Rust toolchain, then install the pieces by hand:

```sh
cargo build --release --package mousekit
sudo install -Dm755 target/release/mousekit /usr/local/bin/mousekit
sudo install -Dm644 packaging/udev/70-mousekit.rules /etc/udev/rules.d/70-mousekit.rules
sudo install -Dm644 packaging/modules-load/mousekit.conf /etc/modules-load.d/mousekit.conf
sudo install -Dm644 packaging/desktop/io.github.jakebrace.mousekit.desktop \
    /usr/local/share/applications/io.github.jakebrace.mousekit.desktop
sudo modprobe uinput
sudo udevadm control --reload-rules && sudo udevadm trigger && sudo udevadm settle
```

## First run

1. Open **MouseKit** from your application menu, or run `mousekit gui`. This
   starts the background service, which from then on starts at login.
2. On the **Add a mouse** page, click **Add** next to your mouse.
3. On the mouse's page, click **Capture buttons** and press each button you
   want to remap. Press Esc when you are done, then rename the buttons to
   something you will recognise.
4. Open the **Default** profile to set what each button does everywhere, or
   add a profile for one application: type a name, click **Add profile**, pick
   the application under **Add from running apps**, and set its buttons.

Left click is never remapped, so a mistake cannot lock you out of your
desktop. If something does go wrong, `mousekit pause` (or the tray's **Pause
remapping**) makes the mouse behave normally, and `mousekit quit` stops
MouseKit altogether.

After setting it up, open your desktop's mouse settings and choose the pointer
speed and acceleration for the device called "mousekit virtual mouse". The
desktop sees that device instead of your real mouse.

## Command line

```text
mousekit gui              open the settings window
mousekit daemon           run the background service (normally automatic)
mousekit status           show the active profile, recent windows and connected mice
mousekit pause            stop remapping
mousekit resume           start remapping again
mousekit toggle           pause or resume; useful as a keyboard shortcut
mousekit profile <name>   force a profile; "profile auto" follows the focused window
mousekit capture          print the event name of the next button you press
mousekit reload           re-read the config files
mousekit quit             stop the background service
```

## Configuration files

Everything the settings window does is stored as plain TOML under
`~/.config/mousekit/`, and can be edited by hand. Run `mousekit reload`
afterwards.

| File | Holds |
| --- | --- |
| `config.toml` | Settings, and your mice with their named buttons, DPI and report rate |
| `profiles/default.toml` | Bindings used when no other profile matches |
| `profiles/<name>.toml` | One profile per application, layered over the default |
| `macros/<name>.toml` | One macro per file |

Commented examples are in [docs/examples](docs/examples). Saving from the
settings window rewrites a file without its comments.

## Good to know

- **Spare keys for games.** Numpad keys make good targets, because games
  accept them and most people never press them. F13 to F24 only work if your
  desktop's keyboard settings treat them as plain function keys; by default
  they are media keys that games ignore.
- **Macros stop when you switch apps.** A profile switch, pausing, or
  unplugging the mouse cuts any running macro short and releases its keys.
  Whether timed macros are allowed in a competitive game is up to that game.
- **Macros are recorded inside the settings window.** MouseKit never reads
  your keyboard device.
- **DPI and report rate are re-applied on connect.** They live in the mouse's
  volatile memory, so MouseKit sets them again each time the mouse connects.
  Setting either one switches the mouse out of its onboard-profile mode. Other
  tools that manage the same mouse, such as Solaar, will fight over these
  settings; use one or the other.
- **Games inside gamescope are invisible.** The desktop sees one `gamescope`
  window, so per-game switching cannot see through it.

## Troubleshooting

- **My mouse is listed but shows "Not connected".** The permission rule has
  probably not been applied to it yet. Unplug and replug the mouse or its
  receiver, or reboot. `mousekit status` and `journalctl --user -b | grep
  mousekit` show what the service sees.
- **A button does nothing when I capture it.** Some buttons, often the DPI
  and profile buttons, are handled inside the mouse and send nothing to the
  computer in its default mode.
- **The profile does not switch.** Run `mousekit status` right after focusing
  the application. The recent windows list shows the window class MouseKit
  saw; it must match what the profile lists under "Applies to".
- **The pointer feels different.** Set the speed and acceleration for
  "mousekit virtual mouse" in your desktop's mouse settings.

## Building and testing

```sh
cargo build --release --package mousekit
cargo test --workspace
```

One end-to-end test drives the real service with a fake mouse made through
`uinput`. It needs the udev rule installed and is skipped by default:

```sh
cargo test --package mousekit --test fake_mouse -- --ignored
```

### Building the package locally

`packaging/arch/PKGBUILD` builds the latest commit from GitHub. To package a
local checkout instead, point its source at your directory:

```sh
cd packaging/arch
sed "s|git+https://github.com/jakebrace/mousekit.git|git+file://$(git rev-parse --show-toplevel)|" PKGBUILD > PKGBUILD.local
makepkg -p PKGBUILD.local -si
```

Only committed changes are included.

The project plan and the reasoning behind its design are in
[docs/PLAN.md](docs/PLAN.md).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
