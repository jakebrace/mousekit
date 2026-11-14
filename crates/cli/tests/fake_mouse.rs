//! End-to-end test against a fake mouse made with uinput. No real hardware
//! is touched: the test creates its own mouse, runs the real daemon binary
//! against it with a temporary config, and reads what comes out of the
//! daemon's virtual device.
//!
//! It needs /dev/uinput and the udev rule from packaging/udev, so it cannot
//! run in CI and is ignored by default. Run it by hand with:
//!
//!     cargo test -p mouse-profiles --test fake_mouse -- --ignored
//!
//! It is safe to run while a real daemon is in use: the test daemon gets its
//! own config, bus name and virtual device name, and no tray, focus backend
//! or autostart entry (that is what MOUSE_PROFILES_INSTANCE does).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use evdev::uinput::VirtualDevice;
use evdev::{
    AttributeSet, BusType, Device, EventSummary, EventType, InputEvent, InputId, KeyCode,
    RelativeAxisCode,
};

const BINARY: &str = env!("CARGO_BIN_EXE_mouse-profiles");
const FAKE_NAME: &str = "mouse-profiles test mouse";
const OUTPUT_NAME: &str = "mouse-profiles virtual mouse (test)";
const TIMEOUT: Duration = Duration::from_secs(8);

/// Kills the daemon even when an assertion fails, so a failed test never
/// leaves a process holding a grab.
struct Daemon {
    child: Child,
    config_home: PathBuf,
}

impl Daemon {
    fn cli(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BINARY);
        command
            .args(args)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("MOUSE_PROFILES_INSTANCE", "test");
        command
    }

    fn command(&self, command: &str) -> String {
        let args: Vec<&str> = command.split(' ').collect();
        let output = self.cli(&args).output().expect("run the CLI");
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr)
    }

    fn wait_for_status(&self, wanted: &str) {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let status = self.command("status");
            if status.contains(wanted) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "status never contained {wanted:?}; last status:\n{status}"
            );
            sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fake_mouse() -> VirtualDevice {
    VirtualDevice::builder()
        .expect("open /dev/uinput")
        .name(FAKE_NAME)
        .input_id(InputId::new(BusType::BUS_USB, 0x1209, 0x0f01, 1))
        .with_keys(&AttributeSet::from_iter([
            KeyCode::BTN_LEFT,
            KeyCode::BTN_RIGHT,
            KeyCode::BTN_SIDE,
        ]))
        .unwrap()
        .with_relative_axes(&AttributeSet::from_iter([
            RelativeAxisCode::REL_X,
            RelativeAxisCode::REL_Y,
            RelativeAxisCode::REL_WHEEL,
        ]))
        .unwrap()
        .build()
        .expect("create the fake mouse")
}

/// Finds an event node by device name through sysfs.
fn node_named(name: &str) -> Option<PathBuf> {
    for entry in fs::read_dir("/sys/class/input").ok()? {
        let entry = entry.ok()?;
        let file_name = entry.file_name();
        let file_name = file_name.to_str()?;
        if !file_name.starts_with("event") {
            continue;
        }
        let found = fs::read_to_string(entry.path().join("device/name")).unwrap_or_default();
        if found.trim() == name {
            return Some(Path::new("/dev/input").join(file_name));
        }
    }
    None
}

/// Opens and grabs the daemon's output device, so nothing this test emits
/// reaches the desktop. Retries while udev is still granting access.
fn grab_output() -> Device {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(Ok(mut device)) = node_named(OUTPUT_NAME).map(Device::open) {
            device.grab().expect("grab the output device");
            device.set_nonblocking(true).unwrap();
            return device;
        }
        assert!(
            Instant::now() < deadline,
            "could not open the daemon's virtual device"
        );
        sleep(Duration::from_millis(50));
    }
}

fn key(code: KeyCode, value: i32) -> InputEvent {
    InputEvent::new(EventType::KEY.0, code.code(), value)
}

/// Waits for the next key or relative event from the output device,
/// skipping SYN and scan-code events.
fn next_output(output: &mut Device) -> (u16, u16, i32) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(events) = output.fetch_events() {
            for event in events {
                match event.destructure() {
                    EventSummary::Key(..) | EventSummary::RelativeAxis(..) => {
                        return (event.event_type().0, event.code(), event.value());
                    }
                    _ => {}
                }
            }
        }
        assert!(Instant::now() < deadline, "no output event arrived");
        sleep(Duration::from_millis(5));
    }
}

fn expect_key(output: &mut Device, code: KeyCode, value: i32) {
    assert_eq!(
        next_output(output),
        (EventType::KEY.0, code.code(), value),
        "expected {code:?} with value {value}"
    );
}

#[test]
#[ignore = "needs /dev/uinput and the udev rule; run by hand"]
fn daemon_remaps_a_fake_mouse_across_pause_and_replug() {
    let temp = std::env::temp_dir().join(format!("mp-test-{}", std::process::id()));
    let config_dir = temp.join("config/mouse-profiles");
    fs::create_dir_all(config_dir.join("profiles")).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        r#"
        [[device]]
        ids = ["1209:0f01"]
        name = "Fake Mouse"
        [[device.control]]
        label = "M4"
        event = "BTN_SIDE"
        "#,
    )
    .unwrap();
    fs::write(
        config_dir.join("profiles/default.toml"),
        "name = \"Default\"\n[bindings]\nM4 = { type = \"key\", keys = [\"KEY_F13\"] }\n",
    )
    .unwrap();

    let mut fake = fake_mouse();
    let child = Command::new(BINARY)
        .arg("daemon")
        .env("XDG_CONFIG_HOME", temp.join("config"))
        .env("MOUSE_PROFILES_INSTANCE", "test")
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start the daemon");
    let mut daemon = Daemon {
        child,
        config_home: temp.join("config"),
    };

    let mut output = grab_output();
    daemon.wait_for_status("connected: /dev/input/event");

    // The default-profile binding works, and pointer motion is untouched.
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F13, 1);
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F13, 0);
    fake.emit(&[InputEvent::new(
        EventType::RELATIVE.0,
        RelativeAxisCode::REL_X.0,
        3,
    )])
    .unwrap();
    assert_eq!(
        next_output(&mut output),
        (EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, 3)
    );

    // Left click is never touched.
    fake.emit(&[key(KeyCode::BTN_LEFT, 1)]).unwrap();
    expect_key(&mut output, KeyCode::BTN_LEFT, 1);
    fake.emit(&[key(KeyCode::BTN_LEFT, 0)]).unwrap();
    expect_key(&mut output, KeyCode::BTN_LEFT, 0);

    // Pausing while the button is held releases the remapped key at once,
    // and while paused the button is itself again.
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F13, 1);
    assert!(daemon.command("pause").contains("paused"));
    expect_key(&mut output, KeyCode::KEY_F13, 0);
    // The physical release that follows produces nothing: the kernel drops a
    // release for a button the virtual device never pressed.
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::BTN_SIDE, 1);
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    expect_key(&mut output, KeyCode::BTN_SIDE, 0);
    assert!(daemon.command("toggle").contains("active"));

    // Unplugging with the button held releases the remapped key.
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F13, 1);
    drop(fake);
    expect_key(&mut output, KeyCode::KEY_F13, 0);
    daemon.wait_for_status("connected: none");

    // Replugging brings the binding back without restarting anything.
    let mut fake = fake_mouse();
    daemon.wait_for_status("connected: /dev/input/event");
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F13, 1);
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F13, 0);

    // A reload with a broken profile keeps the old one.
    fs::write(config_dir.join("profiles/default.toml"), "name = ").unwrap();
    assert!(daemon.command("reload").contains("reload failed"));
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F13, 1);
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F13, 0);

    // A reload with a changed profile applies to the next press.
    fs::write(
        config_dir.join("profiles/default.toml"),
        "name = \"Default\"\n[bindings]\nM4 = { type = \"key\", keys = [\"KEY_F14\"] }\n",
    )
    .unwrap();
    assert!(daemon.command("reload").contains("reloaded"));
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F14, 1);
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F14, 0);

    // A profile chosen by hand overrides the default until set back to auto.
    fs::write(
        config_dir.join("profiles/game.toml"),
        "name = \"Game\"\n[match]\nwindow_class = [\"some-game\"]\n[bindings]\nM4 = { type = \"mouse_button\", button = \"BTN_MIDDLE\" }\n",
    )
    .unwrap();
    assert!(daemon.command("reload").contains("1 app profile"));
    assert!(daemon.command("profile Nope").contains("no profile named"));
    assert!(daemon.command("profile game").contains("profile: Game"));
    assert!(daemon.command("status").contains("chosen by hand"));
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::BTN_MIDDLE, 1);
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    expect_key(&mut output, KeyCode::BTN_MIDDLE, 0);
    assert!(daemon.command("profile auto").contains("profile: Default"));
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F14, 1);
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F14, 0);

    // capture reports the button's real name and swallows that press, then
    // remapping carries on as before. Left click is not captured and keeps
    // working during the session.
    let capture = daemon
        .cli(&["capture"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start capture");
    sleep(Duration::from_millis(500));
    fake.emit(&[key(KeyCode::BTN_LEFT, 1)]).unwrap();
    expect_key(&mut output, KeyCode::BTN_LEFT, 1);
    fake.emit(&[key(KeyCode::BTN_LEFT, 0)]).unwrap();
    expect_key(&mut output, KeyCode::BTN_LEFT, 0);
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    let captured = capture.wait_with_output().expect("capture finishes");
    assert_eq!(String::from_utf8_lossy(&captured.stdout).trim(), "BTN_SIDE");
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F14, 1);
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F14, 0);

    // The calls the settings window uses, made here with busctl.
    let busctl = |method: &str, signature: &str, args: &[&str]| -> String {
        let output = Command::new("busctl")
            .args([
                "--user",
                "call",
                "io.github.mouseprofiles.Daemon.test",
                "/io/github/mouseprofiles/Daemon",
                "io.github.mouseprofiles.Daemon1",
                method,
                signature,
            ])
            .args(args)
            .output()
            .expect("run busctl");
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr)
    };

    // An open capture session records every button once, in order, and
    // stays open until cancelled.
    busctl("StartCapture", "", &[]);
    for button in [KeyCode::BTN_RIGHT, KeyCode::BTN_SIDE, KeyCode::BTN_RIGHT] {
        fake.emit(&[key(button, 1)]).unwrap();
        fake.emit(&[key(button, 0)]).unwrap();
    }
    sleep(Duration::from_millis(200));
    assert_eq!(
        busctl("PollCapture", "", &[]).trim(),
        "bas true 2 \"BTN_RIGHT\" \"BTN_SIDE\""
    );
    assert_eq!(busctl("PollCapture", "", &[]).trim(), "bas true 0");
    busctl("CancelCapture", "", &[]);
    assert_eq!(busctl("PollCapture", "", &[]).trim(), "bas false 0");
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F14, 1);
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F14, 0);

    let new_profile = "name = \"Made By Gui\"\n[match]\nwindow_class = [\"gui-app\"]\n[bindings]\nM4 = { type = \"disabled\" }\n";
    busctl("SaveProfile", "ss", &["", new_profile]);
    let new_path = config_dir.join("profiles/made-by-gui.toml");
    assert!(new_path.exists(), "a new profile gets its own file");

    // A profile that claims another profile's app is refused, and no file
    // is written for it.
    let clash = "name = \"Clash\"\n[match]\nwindow_class = [\"some-game\"]\n";
    assert!(busctl("SaveProfile", "ss", &["", clash]).contains("claimed by both"));
    assert!(!config_dir.join("profiles/clash.toml").exists());

    // Binding left click is refused too.
    let device_with_left = "ids = [\"1209:0f01\"]\nname = \"Fake Mouse\"\n[[control]]\nlabel = \"M4\"\nevent = \"BTN_LEFT\"\n";
    assert!(busctl("SaveDevice", "us", &["0", device_with_left]).contains("left click"));

    // Renaming a control renames it in every profile, so bindings survive.
    busctl("RenameControl", "uss", &["0", "M4", "Thumb"]);
    assert!(
        fs::read_to_string(config_dir.join("config.toml"))
            .unwrap()
            .contains("Thumb")
    );
    assert!(fs::read_to_string(&new_path).unwrap().contains("Thumb"));
    fake.emit(&[key(KeyCode::BTN_SIDE, 1)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F14, 1);
    fake.emit(&[key(KeyCode::BTN_SIDE, 0)]).unwrap();
    expect_key(&mut output, KeyCode::KEY_F14, 0);

    let snapshot = busctl("GetSnapshot", "", &[]);
    assert!(snapshot.contains("Made By Gui") && snapshot.contains("Thumb"));
    assert!(snapshot.contains("connected = true"));

    busctl("DeleteProfile", "s", &["Made By Gui"]);
    assert!(!new_path.exists());
    assert!(busctl("DeleteProfile", "s", &["Default"]).contains("cannot be deleted"));

    // A second daemon refuses to start while the first is running.
    let second = daemon
        .cli(&["daemon"])
        .output()
        .expect("run a second daemon");
    assert!(String::from_utf8_lossy(&second.stderr).contains("already running"));

    // quit stops the daemon cleanly, and a second command finds nobody home.
    assert!(daemon.command("quit").contains("stopping"));
    let exit = daemon.child.wait().expect("daemon exits");
    assert!(exit.success());
    assert!(daemon.command("status").contains("not running"));

    let _ = fs::remove_dir_all(&temp);
}
