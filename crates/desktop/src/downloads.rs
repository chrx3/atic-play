//! Offline downloads: playlists and albums the user keeps on disk to listen without a
//! connection. The list lives in `downloads.json`; the audio, in the client's offline folder.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use gpui::{App, AppContext, Context, Entity, Global};
use serde::{Deserialize, Serialize};
use ytm_core::{Client, Track};

/// How many tracks download at once. Each one waits on yt-dlp for several seconds, so a little
/// parallelism helps, without hammering YouTube.
const CONCURRENT: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadedCollection {
    /// The playlist or album id.
    pub id: String,
    pub title: String,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadState {
    /// Tracks already on disk.
    pub done: usize,
    pub total: usize,
    /// Some of its tracks are still downloading or waiting to.
    pub active: bool,
}

pub struct Downloads {
    client: Client,
    path: PathBuf,
    collections: Vec<DownloadedCollection>,
    /// Tracks kept offline.
    downloaded: HashSet<String>,
    pending: VecDeque<String>,
    in_flight: HashSet<String>,
}

struct GlobalDownloads(Entity<Downloads>);

impl Global for GlobalDownloads {}

/// Loads the downloads list from `data_dir` and resumes anything left unfinished.
pub fn init(client: Client, data_dir: &Path, cx: &mut App) {
    let path = data_dir.join("downloads.json");
    let downloads = cx.new(|cx| Downloads::new(client, path, cx));
    cx.set_global(GlobalDownloads(downloads));
}

impl Downloads {
    /// The app's downloads; views observe it to show progress.
    pub fn global(cx: &App) -> Entity<Downloads> {
        cx.global::<GlobalDownloads>().0.clone()
    }

    fn new(client: Client, path: PathBuf, cx: &mut Context<Self>) -> Self {
        let collections: Vec<DownloadedCollection> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|json| {
                serde_json::from_str(&json)
                    .inspect_err(|err| log::warn!("ignoring {}: {err}", path.display()))
                    .ok()
            })
            .unwrap_or_default();
        let mut downloads = Downloads {
            client,
            path,
            collections,
            downloaded: HashSet::new(),
            pending: VecDeque::new(),
            in_flight: HashSet::new(),
        };
        let tracks: Vec<String> = downloads.all_tracks().collect();
        for id in tracks {
            if downloads.client.is_downloaded(&id) {
                downloads.downloaded.insert(id);
            } else {
                downloads.enqueue(id);
            }
        }
        downloads.pump(cx);
        downloads
    }

    /// Keeps a playlist or album offline. Downloading it again picks up tracks added since.
    pub fn download(
        &mut self,
        id: String,
        title: String,
        tracks: Vec<Track>,
        cx: &mut Context<Self>,
    ) {
        let missing: Vec<String> = tracks
            .iter()
            .map(|track| track.id.clone())
            .filter(|track_id| !self.downloaded.contains(track_id))
            .collect();
        let collection = DownloadedCollection { id, title, tracks };
        match self
            .collections
            .iter_mut()
            .find(|kept| kept.id == collection.id)
        {
            Some(kept) => *kept = collection,
            None => self.collections.push(collection),
        }
        for track_id in missing {
            self.enqueue(track_id);
        }
        self.save();
        self.pump(cx);
        cx.notify();
    }

    /// Stops keeping a playlist or album offline. Tracks another download also has stay.
    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(index) = self.collections.iter().position(|kept| kept.id == id) else {
            return;
        };
        let removed = self.collections.remove(index);
        let still_kept: HashSet<String> = self.all_tracks().collect();
        for track in removed.tracks {
            if still_kept.contains(&track.id) {
                continue;
            }
            self.pending.retain(|pending| *pending != track.id);
            if self.downloaded.remove(&track.id) {
                let remove = self.client.remove_download(track.id.clone());
                cx.background_executor()
                    .spawn(async move {
                        if let Err(err) = remove.await {
                            log::warn!("could not remove a download: {err:#}");
                        }
                    })
                    .detach();
            }
        }
        self.save();
        cx.notify();
    }

    pub fn state(&self, id: &str) -> Option<DownloadState> {
        let collection = self.collections.iter().find(|kept| kept.id == id)?;
        let done = collection
            .tracks
            .iter()
            .filter(|track| self.downloaded.contains(&track.id))
            .count();
        let active = collection
            .tracks
            .iter()
            .any(|track| self.pending.contains(&track.id) || self.in_flight.contains(&track.id));
        Some(DownloadState {
            done,
            total: collection.tracks.len(),
            active,
        })
    }

    pub fn is_downloaded(&self, track_id: &str) -> bool {
        self.downloaded.contains(track_id)
    }

    pub fn collections(&self) -> &[DownloadedCollection] {
        &self.collections
    }

    fn all_tracks(&self) -> impl Iterator<Item = String> + '_ {
        self.collections
            .iter()
            .flat_map(|collection| collection.tracks.iter().map(|track| track.id.clone()))
    }

    fn enqueue(&mut self, track_id: String) {
        if !self.pending.contains(&track_id) && !self.in_flight.contains(&track_id) {
            self.pending.push_back(track_id);
        }
    }

    /// Starts downloads until `CONCURRENT` run at once.
    fn pump(&mut self, cx: &mut Context<Self>) {
        while self.in_flight.len() < CONCURRENT {
            let Some(track_id) = self.pending.pop_front() else {
                return;
            };
            self.in_flight.insert(track_id.clone());
            let download = self.client.download(track_id.clone());
            cx.spawn(async move |this, cx| {
                let result = download.await;
                this.update(cx, |this, cx| {
                    this.in_flight.remove(&track_id);
                    match result {
                        // Removed while it downloaded: let it go again.
                        Ok(()) if !this.all_tracks().any(|kept| kept == track_id) => {
                            let remove = this.client.remove_download(track_id.clone());
                            cx.background_executor()
                                .spawn(async move { remove.await.ok() })
                                .detach();
                        }
                        Ok(()) => {
                            this.downloaded.insert(track_id);
                        }
                        // Not retried now; downloading the collection again tries it again.
                        Err(err) => log::warn!("could not download {track_id}: {err:#}"),
                    }
                    this.pump(cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    fn save(&self) {
        let written = serde_json::to_string_pretty(&self.collections)
            .map_err(std::io::Error::other)
            .and_then(|json| {
                // Write aside and rename, so a crash mid-write never loses the list.
                let partial = self.path.with_extension("json.part");
                std::fs::write(&partial, json)?;
                std::fs::rename(&partial, &self.path)
            });
        if let Err(err) = written {
            log::warn!("could not save the downloads list: {err}");
        }
    }
}
