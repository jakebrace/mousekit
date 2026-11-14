//! The single binary. `daemon` runs the engine, `gui` opens the settings
//! window, and every other command is a D-Bus call to the running daemon.
//!
//! The GUI toolkit is linked in but only loaded by `gui`; the daemon never
//! touches it, so its memory use stays small.

use mouse_profiles_daemon::service;

const USAGE: &str = "\
Usage: mouse-profiles <command>

  daemon           run the remapper (normally started at login)
  gui              open the settings window
  status           show the active profile, recent windows and connected mice
  pause            stop remapping (the mouse behaves as if unmodified)
  resume           start remapping again
  toggle           pause if active, resume if paused (bind this to a keyboard shortcut)
  profile <name>   force a profile; `profile auto` follows the focused window again
  capture          print the event name of the next mouse button you press
  reload           re-read the config files
  quit             stop the daemon";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["daemon"] => mouse_profiles_daemon::run(),
        ["gui"] => mouse_profiles_gui::run(),
        ["--help" | "-h" | "help"] => {
            println!("{USAGE}");
            Ok(())
        }
        [command, rest @ ..] if service::COMMANDS.contains(command) && rest.len() <= 1 => {
            service::run_command(command, rest.first().copied()).map(|reply| println!("{reply}"))
        }
        _ => Err(USAGE.to_string()),
    };
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(1);
    }
}
