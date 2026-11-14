//! The single binary. `daemon` runs the engine; every other command is a
//! one-line message to the running daemon.

use mouse_profiles_daemon::control;

const USAGE: &str = "\
Usage: mouse-profiles <command>

  daemon   run the remapper in the foreground
  status   show whether remapping is active and which mice are connected
  pause    stop remapping (the mouse behaves as if unmodified)
  resume   start remapping again
  toggle   pause if active, resume if paused (bind this to a keyboard shortcut)
  reload   re-read the config files
  quit     stop the daemon";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [command] if command == "daemon" => mouse_profiles_daemon::run(),
        [command] if control::COMMANDS.contains(&command.as_str()) => {
            control::send(command).map(|reply| println!("{reply}"))
        }
        [flag] if flag == "--help" || flag == "-h" || flag == "help" => {
            println!("{USAGE}");
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    };
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(1);
    }
}
