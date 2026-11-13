//! What the app remembers between runs: the theme, the volume and where the window was.
//!
//! It is a small JSON file under the user's data directory — `%LOCALAPPDATA%\atic-play` on
//! Windows, `~/Library/Application Support/atic-play` on macOS — and never holds the account:
//! the session is a secret and lives in the keyring, not here.
//!
//! The file is read once, on start, and written whenever something changes. A damaged file is
//! not a crash: it is logged and replaced.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// The smallest side a saved window may have, in logical pixels: below it the rail and the page
/// stop making sense.
const MIN_SIDE: f32 = 360.;

/// Where the window was, and how big.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowPrefs {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

impl WindowPrefs {
    /// A saved position is only useful when it is big enough to be true.
    pub fn sane(&self) -> bool {
        self.width.is_finite()
            && self.height.is_finite()
            && self.x.is_finite()
            && self.y.is_finite()
            && self.width >= MIN_SIDE
            && self.height >= MIN_SIDE
    }
}

/// Everything the file holds. Every field is optional so an old file still reads.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `None` means "follow the system appearance".
    pub dark: Option<bool>,
    /// 0..=100, like the volume slider.
    pub volume: Option<f32>,
    pub window: Option<WindowPrefs>,
}

impl Settings {
    /// Clamps what a hand-edited file could get wrong.
    fn sanitized(mut self) -> Self {
        if let Some(volume) = self.volume {
            self.volume = if volume.is_finite() {
                Some(volume.clamp(0., 100.))
            } else {
                None
            };
        }
        if self.window.is_some_and(|window| !window.sane()) {
            self.window = None;
        }
        self
    }
}

/// The file, loaded once; `None` until the first read.
fn cache() -> &'static Mutex<Option<Settings>> {
    static SETTINGS: OnceLock<Mutex<Option<Settings>>> = OnceLock::new();
    SETTINGS.get_or_init(|| Mutex::new(None))
}

/// `<data dir>/atic-play/settings.json`.
pub fn path() -> PathBuf {
    data_dir().join("atic-play").join("settings.json")
}

fn data_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
            })
    };
    base.unwrap_or_else(std::env::temp_dir)
}

/// The settings on disk, read the first time and kept from then on.
pub fn get() -> Settings {
    let mut cache = cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.is_none() {
        *cache = Some(read());
    }
    cache.clone().unwrap_or_default()
}

fn read() -> Settings {
    let path = path();
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Settings::default();
    };
    match serde_json::from_str::<Settings>(&raw) {
        Ok(settings) => settings.sanitized(),
        Err(err) => {
            log::warn!("settings at {} are damaged ({err}); starting fresh", path.display());
            Settings::default()
        }
    }
}

/// Changes the settings in memory and writes the file.
pub fn update(f: impl FnOnce(&mut Settings)) {
    let mut cache = cache().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut settings = cache.clone().unwrap_or_else(read);
    f(&mut settings);
    let settings = settings.sanitized();
    *cache = Some(settings.clone());
    if let Err(err) = write(&settings) {
        log::warn!("could not save the settings: {err}");
    }
}

fn write(settings: &Settings) -> std::io::Result<()> {
    let path = path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    // Write next to the file and rename: a crash mid-write never leaves half a settings file.
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, json)?;
    std::fs::rename(&temporary, &path)
}

/// The theme the user picked, if they picked one.
pub fn dark() -> Option<bool> {
    get().dark
}

pub fn save_theme(dark: bool) {
    update(|settings| settings.dark = Some(dark));
}

/// The saved volume, 0..=100.
pub fn volume() -> Option<f32> {
    get().volume
}

pub fn save_volume(volume: f32) {
    let volume = volume.clamp(0., 100.);
    // The slider sends a value per drag pixel: skip the writes that change nothing.
    if get().volume.is_some_and(|saved| (saved - volume).abs() < 0.5) {
        return;
    }
    update(|settings| settings.volume = Some(volume));
}

/// Where the window was the last time it closed.
pub fn window() -> Option<WindowPrefs> {
    get().window.filter(WindowPrefs::sane)
}

pub fn save_window(window: WindowPrefs) {
    if !window.sane() {
        return;
    }
    if get().window == Some(window) {
        return;
    }
    update(|settings| settings.window = Some(window));
}

// ---------------------------------------------------------------- system preferences

/// Whether Windows is set to animate windows and controls. `SPI_GETCLIENTAREAANIMATION` is the
/// switch behind Settings ▸ Accessibility ▸ Visual effects ▸ Animation effects.
#[cfg(target_os = "windows")]
pub fn system_animations() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };

    // `pvParam` is a `BOOL *`: non-zero when the animations are on.
    let mut enabled: i32 = 1;
    let result = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut i32 as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    match result {
        Ok(()) => enabled != 0,
        Err(_) => true,
    }
}

/// macOS and the others: no reading of the preference yet, so the app animates.
#[cfg(not(target_os = "windows"))]
pub fn system_animations() -> bool {
    true
}

/// Whether the app should move as little as possible: `M3_REDUCED=1` wins over the system.
pub fn reduced_motion() -> bool {
    if std::env::var("M3_REDUCED").is_ok_and(|value| value == "1") {
        return true;
    }
    !system_animations()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_lives_in_the_app_directory() {
        let path = path();
        assert_eq!(path.file_name().unwrap(), "settings.json");
        assert_eq!(path.parent().unwrap().file_name().unwrap(), "atic-play");
    }

    #[test]
    fn a_file_without_fields_is_valid() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, Settings::default());
        let partial: Settings = serde_json::from_str(r#"{"dark": true}"#).unwrap();
        assert_eq!(partial.dark, Some(true));
        assert_eq!(partial.volume, None);
    }

    #[test]
    fn nonsense_values_are_dropped() {
        let settings: Settings =
            serde_json::from_str::<Settings>(r#"{"volume": 300, "window": {"x": 0, "y": 0, "width": 10, "height": 10}}"#)
                .unwrap()
                .sanitized();
        assert_eq!(settings.volume, Some(100.));
        assert_eq!(settings.window, None);
    }

    #[test]
    fn a_window_round_trips() {
        let prefs = WindowPrefs {
            x: 40.,
            y: 60.,
            width: 1280.,
            height: 800.,
            maximized: false,
        };
        let json = serde_json::to_string(&Settings {
            dark: Some(true),
            volume: Some(42.),
            window: Some(prefs),
        })
        .unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.window, Some(prefs));
        assert!(prefs.sane());
        assert!(!WindowPrefs { width: 100., ..prefs }.sane());
        assert!(!WindowPrefs { x: f32::NAN, ..prefs }.sane());
    }
}
