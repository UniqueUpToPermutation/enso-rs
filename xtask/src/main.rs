//! Project automation, run as `cargo xtask <command>` (see `.cargo/config.toml`).
//!
//! The "xtask" pattern: helper commands are an ordinary Rust binary in the
//! workspace instead of shell scripts, so they work the same on every OS.

mod golden;

use std::process::ExitCode;

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  golden                 review golden-image test failures (opens a report,
                         asks whether to accept each one)
  golden accept <name>…  accept the new rendering(s) of the named examples
  golden accept --all    accept every failing example
  golden clean           delete leftover .actual.png / .diff.png files
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    let result = match args.as_slice() {
        ["golden"] | ["golden", "review"] => golden::review(),
        ["golden", "accept", names @ ..] if !names.is_empty() => golden::accept(names),
        ["golden", "clean"] => golden::clean(),
        _ => {
            eprint!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
