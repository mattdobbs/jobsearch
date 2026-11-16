//! Minimal file + stderr logging. The app normally runs with no terminal attached,
//! so the log file is the only place a failed fetch can be diagnosed.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

static LOG: OnceLock<Mutex<Option<File>>> = OnceLock::new();

const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

pub fn init(path: &Path) {
    // Start a fresh file once the old one gets large; one generation is kept.
    if std::fs::metadata(path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    let file = OpenOptions::new().create(true).append(true).open(path).ok();
    let _ = LOG.set(Mutex::new(file));
}

pub fn write(level: &str, msg: &str) {
    let line = format!(
        "{} {level} {msg}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    eprint!("{line}");
    if let Some(lock) = LOG.get() {
        if let Ok(mut guard) = lock.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::logging::write("INFO", &format!($($arg)*)) };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::logging::write("WARN", &format!($($arg)*)) };
}
