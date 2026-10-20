//! What the user listened to, newest first, in `history.json`. It stays on this computer, so it
//! works without an account.

use std::path::{Path, PathBuf};

use gpui::{App, AppContext, Context, Entity, Global};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use ytm_core::Track;

use crate::settings::write_json;

/// Older plays are forgotten past this many.
const MAX_ENTRIES: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub track: Track,
    /// Unix time, in seconds.
    pub played_at: i64,
}

pub struct History {
    path: PathBuf,
    entries: Vec<HistoryEntry>,
}

struct GlobalHistory(Entity<History>);

impl Global for GlobalHistory {}

/// Loads the history from `data_dir`.
pub fn init(data_dir: &Path, cx: &mut App) {
    let path = data_dir.join("history.json");
    let entries = std::fs::read_to_string(&path)
        .ok()
        .and_then(|json| {
            serde_json::from_str(&json)
                .inspect_err(|err| log::warn!("ignoring {}: {err}", path.display()))
                .ok()
        })
        .unwrap_or_default();
    let history = cx.new(|_| History { path, entries });
    cx.set_global(GlobalHistory(history));
}

impl History {
    /// The app's history; views observe it.
    pub fn global(cx: &App) -> Entity<History> {
        cx.global::<GlobalHistory>().0.clone()
    }

    /// Newest first.
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Notes that `track` was played just now.
    pub fn record(&mut self, track: Track, cx: &mut Context<Self>) {
        let played_at = OffsetDateTime::now_utc().unix_timestamp();
        self.entries.insert(0, HistoryEntry { track, played_at });
        self.entries.truncate(MAX_ENTRIES);
        self.save();
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.entries.clear();
        self.save();
        cx.notify();
    }

    fn save(&self) {
        if let Err(err) = write_json(&self.path, &self.entries) {
            log::warn!("could not save the history: {err}");
        }
    }
}
