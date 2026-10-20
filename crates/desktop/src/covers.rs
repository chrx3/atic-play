use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::{Context, Hsla};
use ytm_core::Client;

use crate::playback::seed_color;

/// Cover and avatar files shared by every view. Views observe it to redraw when a download ends.
pub struct Covers {
    client: Client,
    ready: HashMap<String, PathBuf>,
    pending: HashSet<String>,
    /// The theme color of covers, for the page headers; `None` while it is worked out.
    seeds: HashMap<PathBuf, Option<Hsla>>,
}

impl Covers {
    pub fn new(client: Client) -> Self {
        Covers {
            client,
            ready: HashMap::new(),
            pending: HashSet::new(),
            seeds: HashMap::new(),
        }
    }

    /// The color a cover would theme the app with, worked out once in the background.
    pub fn seed(&mut self, cover: &Path, cx: &mut Context<Self>) -> Option<Hsla> {
        if let Some(seed) = self.seeds.get(cover) {
            return *seed;
        }
        self.seeds.insert(cover.to_path_buf(), None);
        let path = cover.to_path_buf();
        cx.spawn(async move |this, cx| {
            let seed_path = path.clone();
            let seed = cx
                .background_executor()
                .spawn(async move { seed_color(&seed_path) })
                .await;
            this.update(cx, |this, cx| {
                if seed.is_some() {
                    this.seeds.insert(path, seed);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        None
    }

    /// The cached file for `id` at `size_px`, starting the download on the first request.
    pub fn get(
        &mut self,
        id: &str,
        url: Option<&str>,
        size_px: u32,
        cx: &mut Context<Self>,
    ) -> Option<PathBuf> {
        let key = format!("{id}-{size_px}");
        if let Some(path) = self.ready.get(&key) {
            return Some(path.clone());
        }
        let url = url?;
        if !self.pending.insert(key.clone()) {
            return None;
        }

        let fetch = self
            .client
            .fetch_cover(id.to_string(), url.to_string(), size_px);
        cx.spawn(async move |this, cx| {
            let result = fetch.await;
            this.update(cx, |this, cx| match result {
                Ok(path) => {
                    this.pending.remove(&key);
                    this.ready.insert(key, path);
                    cx.notify();
                }
                // The key stays pending so a failed cover is not retried on every frame; the
                // views show a placeholder instead.
                Err(err) => log::warn!("cover {key}: {err:#}"),
            })
            .ok();
        })
        .detach();
        None
    }
}
