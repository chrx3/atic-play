//! Downloaded tracks kept on disk, so replaying one skips yt-dlp and the download.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;

pub(crate) struct AudioCache {
    dir: PathBuf,
    max_bytes: u64,
}

impl AudioCache {
    pub fn new(dir: PathBuf, max_bytes: u64) -> Self {
        AudioCache { dir, max_bytes }
    }

    /// The cached audio of a track, if any. Reading it marks it as recently used.
    pub async fn read(&self, track_id: &str) -> Option<Vec<u8>> {
        let path = self.path(track_id)?;
        let bytes = tokio::fs::read(&path).await.ok()?;
        // The modification time doubles as "last played", which is what pruning goes by.
        if let Ok(file) = File::options().write(true).open(&path) {
            file.set_modified(SystemTime::now()).ok();
        }
        Some(bytes)
    }

    /// Stores a track, then drops the least recently played ones beyond the size limit.
    pub async fn write(&self, track_id: &str, bytes: &[u8]) -> Result<()> {
        let Some(path) = self.path(track_id) else {
            return Ok(());
        };
        tokio::fs::create_dir_all(&self.dir).await?;
        // Write aside and rename, so a crash never leaves a truncated file to be played.
        let partial = path.with_extension("part");
        tokio::fs::write(&partial, bytes).await?;
        tokio::fs::rename(&partial, &path).await?;

        let dir = self.dir.clone();
        let max_bytes = self.max_bytes;
        tokio::task::spawn_blocking(move || prune(&dir, max_bytes)).await??;
        Ok(())
    }

    /// Track ids are YouTube video ids; anything else could point outside the cache.
    fn path(&self, track_id: &str) -> Option<PathBuf> {
        let valid = !track_id.is_empty()
            && track_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        valid.then(|| self.dir.join(format!("{track_id}.m4a")))
    }
}

/// Deletes the least recently played tracks until the cache fits in `max_bytes`.
fn prune(dir: &Path, max_bytes: u64) -> Result<()> {
    let mut files: Vec<(SystemTime, u64, PathBuf)> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "m4a"))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            Some((metadata.modified().ok()?, metadata.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, size, path) in files {
        if total <= max_bytes {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ytm-core-test-{name}-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rejects_ids_that_could_leave_the_cache() {
        let cache = AudioCache::new(PathBuf::from("cache"), 0);
        assert!(cache.path("dQw4w9WgXcQ").is_some());
        assert!(cache.path("a-b_c").is_some());
        assert!(cache.path("../secrets").is_none());
        assert!(cache.path("a/b").is_none());
        assert!(cache.path("").is_none());
    }

    #[test]
    fn prune_drops_the_least_recently_played_first() {
        let dir = temp_dir("prune");
        let now = SystemTime::now();
        for (name, age_secs) in [("old", 300), ("middle", 200), ("new", 100)] {
            let path = dir.join(format!("{name}.m4a"));
            fs::write(&path, [0u8; 10]).unwrap();
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(now - Duration::from_secs(age_secs))
                .unwrap();
        }
        fs::write(dir.join("notes.txt"), [0u8; 100]).unwrap();

        prune(&dir, 20).unwrap();

        assert!(!dir.join("old.m4a").exists());
        assert!(dir.join("middle.m4a").exists());
        assert!(dir.join("new.m4a").exists());
        // Only cached tracks count and get removed.
        assert!(dir.join("notes.txt").exists());
        fs::remove_dir_all(&dir).ok();
    }
}
