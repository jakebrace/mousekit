//! Prints every focus change the backend reports. Handy for finding a
//! window's class when writing a profile, and for checking a backend.
//!
//!     cargo run -p mouse-profiles-focus --example watch
//!
//! Do not run it while the daemon is running: both claim the same bus name.
//! Press Enter to stop.

use std::sync::Arc;

fn main() {
    let Some(backend) = mouse_profiles_focus::detect() else {
        eprintln!("no focus backend for this desktop");
        std::process::exit(1);
    };
    let backend: Arc<dyn mouse_profiles_focus::FocusBackend> = Arc::from(backend);

    let runner = backend.clone();
    std::thread::spawn(move || {
        let result = runner.run(Box::new(|window| {
            println!(
                "{:?}  class={:?}  exe={:?}  title={:?}",
                window.kind, window.class, window.exe, window.title
            );
        }));
        if let Err(error) = result {
            eprintln!("backend stopped: {error}");
            std::process::exit(1);
        }
    });

    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    backend.cleanup();
}
