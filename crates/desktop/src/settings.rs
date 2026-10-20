//! What Atic-play remembers between runs, in `settings.json` next to the session.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui::{App, Bounds, Global, Pixels, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};
use ytm_core::{RepeatMode, Track};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub volume: Option<f32>,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub queue: Vec<Track>,
    pub queue_index: Option<usize>,
    /// Where the current track was when the app closed, to resume from there.
    pub position_secs: f32,
    pub window: Option<SavedBounds>,
    pub dark: Option<bool>,
    pub queue_open: Option<bool>,
    /// Even out the volume between tracks (on unless set to false).
    pub normalize: Option<bool>,
    /// How many seconds tracks overlap at a change; 0 (the default) is gapless.
    pub crossfade_secs: f32,
    /// The latest searches, newest first, offered when the search field is empty.
    pub recent_searches: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SavedBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl SavedBounds {
    pub fn from_window(bounds: WindowBounds) -> Self {
        let (rect, maximized) = match bounds {
            WindowBounds::Windowed(rect) => (rect, false),
            WindowBounds::Maximized(rect) | WindowBounds::Fullscreen(rect) => (rect, true),
        };
        SavedBounds {
            x: rect.origin.x.into(),
            y: rect.origin.y.into(),
            width: rect.size.width.into(),
            height: rect.size.height.into(),
            maximized,
        }
    }

    pub fn to_window(self) -> WindowBounds {
        let rect: Bounds<Pixels> = Bounds {
            origin: point(px(self.x), px(self.y)),
            size: size(px(self.width), px(self.height)),
        };
        if self.maximized {
            WindowBounds::Maximized(rect)
        } else {
            WindowBounds::Windowed(rect)
        }
    }
}

/// The settings and the file they live in.
struct Store {
    path: PathBuf,
    settings: Settings,
    pending: Arc<PendingWrite>,
}

/// The settings waiting to be written, off the UI thread. Only the latest version is kept, and
/// writes take turns, so the file always ends with the newest settings.
#[derive(Default)]
struct PendingWrite {
    json: Mutex<Option<Vec<u8>>>,
    writing: Mutex<()>,
}

impl PendingWrite {
    fn write(&self, path: &Path) {
        let _turn = self.writing.lock().unwrap_or_else(|err| err.into_inner());
        let json = self
            .json
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take();
        if let Some(json) = json
            && let Err(err) = write_atomically(path, &json)
        {
            log::warn!("could not save settings: {err}");
        }
    }
}

impl Global for Store {}

/// Loads `settings.json` from `data_dir` (defaults if it is missing or unreadable).
pub fn init(data_dir: &Path, cx: &mut App) {
    let path = data_dir.join("settings.json");
    let settings = std::fs::read_to_string(&path)
        .ok()
        .and_then(|json| {
            serde_json::from_str(&json)
                .inspect_err(|err| log::warn!("ignoring {}: {err}", path.display()))
                .ok()
        })
        .unwrap_or_default();
    let pending = Arc::new(PendingWrite::default());
    // Whatever is still waiting when the app quits is written before it exits.
    let quit_path = path.clone();
    let quit_pending = pending.clone();
    cx.on_app_quit(move |_| {
        let path = quit_path.clone();
        let pending = quit_pending.clone();
        async move { pending.write(&path) }
    })
    .detach();
    cx.set_global(Store {
        path,
        settings,
        pending,
    });
}

pub fn get(cx: &App) -> &Settings {
    &cx.global::<Store>().settings
}

/// Changes the settings and writes them to disk in the background.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    let store = cx.global_mut::<Store>();
    change(&mut store.settings);
    let json = match serde_json::to_vec(&store.settings) {
        Ok(json) => json,
        Err(err) => {
            log::warn!("could not save settings: {err}");
            return;
        }
    };
    *store
        .pending
        .json
        .lock()
        .unwrap_or_else(|err| err.into_inner()) = Some(json);
    let path = store.path.clone();
    let pending = store.pending.clone();
    cx.background_executor()
        .spawn(async move { pending.write(&path) })
        .detach();
}

/// Writes `value` as JSON to `path`.
pub fn write_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    write_atomically(path, &json)
}

/// Writes aside and renames, so a crash mid-write never leaves a broken file.
fn write_atomically(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let partial = path.with_extension("json.part");
    std::fs::write(&partial, contents)?;
    std::fs::rename(&partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_latest_pending_settings_are_written() {
        let dir = std::env::temp_dir().join(format!("atic-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let pending = PendingWrite::default();
        *pending.json.lock().unwrap() = Some(b"{\"muted\":true}".to_vec());
        pending.write(&path);
        // A second write with nothing new leaves the file as it is.
        pending.write(&path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"muted\":true}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
