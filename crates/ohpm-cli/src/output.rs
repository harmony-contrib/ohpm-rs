//! Console output helpers (`PrintUtil`), styled with owo-colors.

use owo_colors::OwoColorize;

/// Print a plain line to stdout.
pub fn output(msg: &str) {
    println!("{msg}");
}

/// Print a success message (green).
pub fn succeed(msg: &str) {
    println!("{}", msg.green());
}

/// Print a warning to stderr (yellow).
pub fn warn(msg: &str) {
    eprintln!("{}", format!("ohpm warn: {msg}").yellow());
}
