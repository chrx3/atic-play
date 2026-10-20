//! What Atic-play remembers between runs, in `settings.json` next to the session.

use std::path::{Path, PathBuf};

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
    cx.set_global(Store { path, settings });
}

pub fn get(cx: &App) -> &Settings {
    &cx.global::<Store>().settings
}

/// Changes the settings and writes them to disk.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    let store = cx.global_mut::<Store>();
    change(&mut store.settings);
    if let Err(err) = write_json(&store.path, &store.settings) {
        log::warn!("could not save settings: {err}");
    }
}

/// Writes `value` as JSON to `path`.
pub fn write_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    // Write aside and rename, so a crash mid-write never leaves a broken file.
    let partial = path.with_extension("json.part");
    std::fs::write(&partial, json)?;
    std::fs::rename(&partial, path)
}
