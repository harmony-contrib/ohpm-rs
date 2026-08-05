//! Console output helpers (`PrintUtil`).

/// Print a plain line to stdout.
pub fn output(msg: &str) {
    println!("{msg}");
}

/// Print a success message (green).
pub fn succeed(msg: &str) {
    println!("\x1b[32m{msg}\x1b[0m");
}

/// Print a warning to stderr (yellow).
pub fn warn(msg: &str) {
    eprintln!("\x1b[33mohpm warn: {msg}\x1b[0m");
}
