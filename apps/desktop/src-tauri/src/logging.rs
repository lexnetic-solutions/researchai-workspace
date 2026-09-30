//! File logging under `<data_dir>/logs/`. Logs never leave the machine.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::error::{AppError, AppResult};

pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

struct FileLogger {
    file: Mutex<fs::File>,
}

impl FileLogger {
    fn open(target: &Path) -> std::io::Result<Self> {
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(target)?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Debug
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} [{}] {}\n",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f"),
            record.level(),
            record.target(),
            record.args()
        );
        if let Ok(mut f) = self.file.lock() {
            use std::io::Write;
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            use std::io::Write;
            let _ = f.flush();
        }
    }
}

/// Initialise the global file logger. `log` requires a `&'static` logger, so
/// the instance is intentionally leaked once.
pub fn init(data_dir: &Path) -> AppResult<()> {
    let dir = log_dir(data_dir);
    fs::create_dir_all(&dir)?;
    let target = dir.join(format!(
        "researchai-{}.log",
        chrono::Local::now().format("%Y-%m-%d")
    ));
    let logger = FileLogger::open(&target)
        .map_err(|e| AppError::msg(format!("Could not open log file {}: {e}", target.display())))?;

    log::set_logger(Box::leak(Box::new(logger)))
        .map_err(|e| AppError::msg(format!("Logger already initialised: {e}")))?;
    log::set_max_level(if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    });
    Ok(())
}
