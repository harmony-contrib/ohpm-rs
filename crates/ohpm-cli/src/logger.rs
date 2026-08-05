//! Minimal console logger honoring the ohpm `log_level` config
//! (`error | warn | info | debug`).

use std::sync::atomic::{AtomicU8, Ordering};

/// Levels ordered 0..3 to match `DefaultConfig.logLevelType`.
const LEVEL_ERROR: u8 = 0;
const LEVEL_WARN: u8 = 1;
const LEVEL_INFO: u8 = 2;
const LEVEL_DEBUG: u8 = 3;

static CURRENT: AtomicU8 = AtomicU8::new(LEVEL_INFO);

struct ConsoleLogger;

impl log::Log for ConsoleLogger {
    fn enabled(&self, _meta: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let bits = match record.level() {
            log::Level::Error => LEVEL_ERROR,
            log::Level::Warn => LEVEL_WARN,
            log::Level::Info => LEVEL_INFO,
            log::Level::Debug | log::Level::Trace => LEVEL_DEBUG,
        };
        if bits <= CURRENT.load(Ordering::Relaxed) {
            eprintln!("{}: {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: ConsoleLogger = ConsoleLogger;

/// Install the console logger (no-op if already installed).
pub fn init() {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
    set_level(&default_level());
}

/// Set the threshold from a `log_level` string.
pub fn set_level(level: &str) {
    let bits = match level.to_ascii_lowercase().as_str() {
        "error" => LEVEL_ERROR,
        "warn" => LEVEL_WARN,
        "debug" => LEVEL_DEBUG,
        _ => LEVEL_INFO,
    };
    CURRENT.store(bits, Ordering::Relaxed);
}

fn default_level() -> String {
    std::env::var("OHPM_LOG_LEVEL").unwrap_or_else(|_| "info".to_string())
}
