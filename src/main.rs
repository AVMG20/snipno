mod adjust;
mod annotate;
mod config;
mod daemon;
mod hotkey;
mod icon;
mod macos;
mod overlay;
mod settings;
mod snap;
mod snapshot;
mod store;
mod theme;
mod toolbar;

use std::process::ExitCode;

const USAGE: &str =
    "usage: snipno [daemon | capture [--monitor <id>] | settings | monitors | icon <out.png> [size] [--tray]]";

/// `snipno monitors`: display ids for `capture --monitor`, with position and size in points.
fn list_monitors() -> Result<(), Box<dyn std::error::Error>> {
    for m in xcap::Monitor::all()? {
        println!(
            "{}\t{},{}\t{}x{}\t{}{}",
            m.id()?,
            m.x()?,
            m.y()?,
            m.width()?,
            m.height()?,
            m.name().unwrap_or_default(),
            if m.is_primary().unwrap_or(false) { " (primary)" } else { "" }
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None | Some("daemon") => daemon::run(),
        Some("capture") => {
            let monitor = args.iter().position(|a| a == "--monitor").and_then(|i| args.get(i + 1)?.parse().ok());
            overlay::run(monitor)
        }
        Some("settings") => settings::run(),
        Some("monitors") => list_monitors(),
        Some("icon") => icon::write_png(&args[1..]),
        Some("-h" | "--help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command {other:?}\n{USAGE}").into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("snipno: {e}");
            ExitCode::FAILURE
        }
    }
}
