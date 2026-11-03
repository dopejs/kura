//! One line per event, to stderr and, once [`to_file`] is called, appended
//! to a log file (`~/Library/Logs/kura-device.log` on macOS).
//!
//! Lines never carry typed text, tokens, or pairing codes: callers log
//! lengths, ids and outcomes only.

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::OnceLock;

use parking_lot::Mutex;

static FILE: OnceLock<Mutex<File>> = OnceLock::new();

/// Also appends every line to `path`, creating it (mode 0600) if needed.
pub fn to_file(path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(path)?;
    let _ = FILE.set(Mutex::new(file));
    Ok(())
}

pub fn info(message: &str) {
    write("info", message);
}

pub fn warn(message: &str) {
    write("warn", message);
}

fn write(level: &str, message: &str) {
    let line = format!(
        "{} {level} {message}\n",
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ")
    );
    eprint!("{line}");
    if let Some(file) = FILE.get() {
        let _ = file.lock().write_all(line.as_bytes());
    }
}
