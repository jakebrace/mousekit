//! Prints the mouse's sensor settings. Read-only: changes nothing.
//!
//!     cargo run -p mouse-profiles-hw --example info -- 046d:409f

fn main() {
    let Some(id) = std::env::args().nth(1).and_then(|text| text.parse().ok()) else {
        eprintln!("usage: info VENDOR:PRODUCT   (for example 046d:409f)");
        std::process::exit(2);
    };
    match mouse_profiles_hw::open(&[id]) {
        Ok(Some(mut hardware)) => match hardware.info() {
            Ok(info) => {
                println!("DPI: {}", info.dpi);
                println!(
                    "DPI choices: {} values from {:?} to {:?}",
                    info.dpi_values.len(),
                    info.dpi_values.first(),
                    info.dpi_values.last()
                );
                println!("Report rate: {} Hz", info.polling_hz);
                println!("Report rate choices: {:?}", info.polling_values);
            }
            Err(error) => eprintln!("error: {error}"),
        },
        Ok(None) => eprintln!("no supported, connected mouse with that ID"),
        Err(error) => eprintln!("error: {error}"),
    }
}
