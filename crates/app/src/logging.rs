//! A small logger: every record goes to stderr and, when the user directory is known, to
//! `atic-play.log` inside the app's log folder.
//!
//! It is started before anything else so that startup, network and audio problems end up where
//! they can be read later. `RUST_LOG` works as usual (`info`, `debug`, `play_ytmusic=trace`,
//! `atic_play::page=debug`, ...); without it our crates log at info and everything else at warn.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{LevelFilter, Log, Metadata, Record};

/// Name of the log file, inside the folder [`log_path`] points at.
const FILE_NAME: &str = "atic-play.log";

/// Past this size the file is restarted on the next launch.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Starts the logger. Returns where the file is, or `None` when it could not be opened (the
/// records still go to stderr).
pub fn init() -> Option<PathBuf> {
    let path = log_path();
    let file = path.as_deref().and_then(open_file);
    let filter = Filter::from_env();
    let max = filter.max;
    let logger = Logger {
        filter,
        file: Mutex::new(file),
    };
    if log::set_boxed_logger(Box::new(logger)).is_err() {
        return path;
    }
    log::set_max_level(max);
    path
}

/// `<data dir>/atic-play/logs/atic-play.log`, by platform.
pub fn log_path() -> Option<PathBuf> {
    platform_log_dir().map(|dir| dir.join(FILE_NAME))
}

#[cfg(target_os = "windows")]
fn platform_log_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty())?;
    Some(PathBuf::from(base).join("atic-play").join("logs"))
}

#[cfg(target_os = "macos")]
fn platform_log_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|value| !value.is_empty())?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Logs")
            .join("atic-play"),
    )
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_log_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })?;
    Some(base.join("atic-play").join("logs"))
}

fn open_file(path: &Path) -> Option<File> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    let restart = std::fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_BYTES);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()?;
    if restart {
        file.set_len(0).ok()?;
    }
    Some(file)
}

struct Logger {
    filter: Filter,
    file: Mutex<Option<File>>,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.filter.level(metadata.target())
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} {}: {}",
            timestamp(SystemTime::now()),
            record.level(),
            record.target(),
            record.args()
        );
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{line}");
        drop(stderr);
        if let Ok(mut file) = self.file.lock()
            && let Some(file) = file.as_mut()
        {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
        if let Ok(mut file) = self.file.lock()
            && let Some(file) = file.as_mut()
        {
            let _ = file.flush();
        }
    }
}

/// Which levels each target may use.
struct Filter {
    rules: Vec<(Option<String>, LevelFilter)>,
    max: LevelFilter,
}

impl Filter {
    fn from_env() -> Self {
        let rules = match std::env::var("RUST_LOG") {
            Ok(text) if !text.trim().is_empty() => parse_rules(&text),
            _ => default_rules(),
        };
        let max = rules
            .iter()
            .map(|(_, level)| *level)
            .max()
            .unwrap_or(LevelFilter::Info);
        Self { rules, max }
    }

    /// The longest matching prefix wins; the rule without a target is the fallback.
    fn level(&self, target: &str) -> LevelFilter {
        let mut best: Option<(usize, LevelFilter)> = None;
        for (prefix, level) in &self.rules {
            match prefix {
                Some(prefix) if target.starts_with(prefix.as_str()) => {
                    if best.is_none_or(|(len, _)| prefix.len() >= len) {
                        best = Some((prefix.len(), *level));
                    }
                }
                None if best.is_none() => best = Some((0, *level)),
                _ => {}
            }
        }
        best.map_or(LevelFilter::Off, |(_, level)| level)
    }
}

fn default_rules() -> Vec<(Option<String>, LevelFilter)> {
    vec![
        (Some("atic_play".into()), LevelFilter::Info),
        (Some("play_".into()), LevelFilter::Info),
        (None, LevelFilter::Warn),
    ]
}

fn parse_rules(text: &str) -> Vec<(Option<String>, LevelFilter)> {
    let mut rules = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((target, level)) = part.split_once('=') {
            if let Some(level) = level_from(level) {
                rules.push((Some(target.trim().to_string()), level));
            }
        } else if let Some(level) = level_from(part) {
            rules.push((None, level));
        } else {
            // A bare target means "everything from it", as in env_logger.
            rules.push((Some(part.to_string()), LevelFilter::Trace));
        }
    }
    if rules.is_empty() {
        default_rules()
    } else {
        rules
    }
}

fn level_from(text: &str) -> Option<LevelFilter> {
    match text.trim().to_ascii_lowercase().as_str() {
        "off" => Some(LevelFilter::Off),
        "error" => Some(LevelFilter::Error),
        "warn" | "warning" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

/// `YYYY-MM-DD HH:MM:SS.mmm` in UTC, without pulling in a calendar crate.
fn timestamp(now: SystemTime) -> String {
    let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs() as i64;
    let millis = since.subsec_millis();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{millis:03}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a (year, month, day) civil date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (
        if month <= 2 { year + 1 } else { year },
        month,
        day,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_epoch_days_into_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_768), (2026, 11, 11));
        // 2000-02-29, the leap day that catches most off-by-one errors.
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn rust_log_directives_win_over_defaults() {
        let filter = Filter {
            rules: parse_rules("warn,play_ytmusic=debug,atic_play::page=trace"),
            max: LevelFilter::Trace,
        };
        assert_eq!(filter.level("gpui"), LevelFilter::Warn);
        assert_eq!(filter.level("play_ytmusic::lib"), LevelFilter::Debug);
        assert_eq!(filter.level("atic_play::page"), LevelFilter::Trace);
        assert_eq!(filter.level("atic_play::main"), LevelFilter::Warn);
        // A bare target means everything from it.
        let filter = Filter {
            rules: parse_rules("gpui"),
            max: LevelFilter::Trace,
        };
        assert_eq!(filter.level("gpui::window"), LevelFilter::Trace);
    }

    #[test]
    fn without_rust_log_our_crates_are_info_and_the_rest_warn() {
        let filter = Filter {
            rules: default_rules(),
            max: LevelFilter::Info,
        };
        assert_eq!(filter.level("atic_play"), LevelFilter::Info);
        assert_eq!(filter.level("play_audio::decode"), LevelFilter::Info);
        assert_eq!(filter.level("gpui"), LevelFilter::Warn);
        assert!(log::Level::Info <= filter.level("play_lyrics"));
        assert!(log::Level::Debug > filter.level("play_lyrics"));
    }
}
