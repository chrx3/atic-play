//! Tracks kept on disk: a size-limited cache of what was played, so replaying skips yt-dlp and the
//! download, and an offline folder of tracks the user downloaded, which is never pruned.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;

/// Next to each `<id>.m4a`, its normalization gain as text, so it is measured once.
const GAIN_EXTENSION: &str = "gain";

pub(crate) struct AudioCache {
    dir: PathBuf,
    offline_dir: PathBuf,
    max_bytes: u64,
}

impl AudioCache {
    pub fn new(dir: PathBuf, offline_dir: PathBuf, max_bytes: u64) -> Self {
        AudioCache {
            dir,
            offline_dir,
            max_bytes,
        }
    }

    /// A track's audio and normalization gain, from the offline folder or the cache. Reading it
    /// from the cache marks it as recently used.
    pub async fn read(&self, track_id: &str) -> Option<(Vec<u8>, Option<f32>)> {
        if let Some(path) = file_in(&self.offline_dir, track_id)
            && let Ok(bytes) = tokio::fs::read(&path).await
        {
            return Some((bytes, read_gain(&path).await));
        }
        let path = file_in(&self.dir, track_id)?;
        let bytes = tokio::fs::read(&path).await.ok()?;
        // The modification time doubles as "last played", which is what pruning goes by.
        if let Ok(file) = File::options().write(true).open(&path) {
            file.set_modified(SystemTime::now()).ok();
        }
        Some((bytes, read_gain(&path).await))
    }

    /// Stores a played track and its gain, then drops the least recently played ones beyond the
    /// size limit. Offline tracks are already kept, so they are not copied into the cache.
    pub async fn write(&self, track_id: &str, bytes: &[u8], gain: Option<f32>) -> Result<()> {
        if self.is_offline(track_id) {
            if let (Some(gain), Some(path)) = (gain, file_in(&self.offline_dir, track_id)) {
                write_gain(&path, gain).await?;
            }
            return Ok(());
        }
        let Some(path) = file_in(&self.dir, track_id) else {
            return Ok(());
        };
        write_track(&self.dir, &path, bytes, gain).await?;

        let dir = self.dir.clone();
        let max_bytes = self.max_bytes;
        tokio::task::spawn_blocking(move || prune(&dir, max_bytes)).await??;
        Ok(())
    }

    pub fn is_offline(&self, track_id: &str) -> bool {
        file_in(&self.offline_dir, track_id).is_some_and(|path| path.exists())
    }

    /// Keeps a track offline. One already in the cache is moved over; otherwise returns false and
    /// the caller downloads it with [`AudioCache::write_offline`].
    pub async fn keep_offline(&self, track_id: &str) -> Result<bool> {
        if self.is_offline(track_id) {
            return Ok(true);
        }
        let (Some(cached), Some(offline)) = (
            file_in(&self.dir, track_id),
            file_in(&self.offline_dir, track_id),
        ) else {
            return Ok(false);
        };
        if !tokio::fs::try_exists(&cached).await? {
            return Ok(false);
        }
        tokio::fs::create_dir_all(&self.offline_dir).await?;
        let gain = cached.with_extension(GAIN_EXTENSION);
        if tokio::fs::try_exists(&gain).await? {
            tokio::fs::rename(&gain, offline.with_extension(GAIN_EXTENSION)).await?;
        }
        tokio::fs::rename(&cached, &offline).await?;
        Ok(true)
    }

    pub async fn write_offline(
        &self,
        track_id: &str,
        bytes: &[u8],
        gain: Option<f32>,
    ) -> Result<()> {
        let Some(path) = file_in(&self.offline_dir, track_id) else {
            return Ok(());
        };
        write_track(&self.offline_dir, &path, bytes, gain).await
    }

    /// Stops keeping a track offline. Later plays download it again, into the cache.
    pub async fn remove_offline(&self, track_id: &str) -> Result<()> {
        let Some(path) = file_in(&self.offline_dir, track_id) else {
            return Ok(());
        };
        for file in [path.with_extension(GAIN_EXTENSION), path] {
            match tokio::fs::remove_file(&file).await {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
                _ => {}
            }
        }
        Ok(())
    }

    /// The size of the cache, without the offline tracks.
    pub async fn cache_bytes(&self) -> u64 {
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || tracks_in(&dir).iter().map(|(_, size, _)| size).sum())
            .await
            .unwrap_or(0)
    }

    /// Empties the cache; offline tracks stay.
    pub async fn clear(&self) -> Result<()> {
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || prune(&dir, 0)).await??;
        Ok(())
    }
}

/// `<dir>/<id>.m4a`. Track ids are YouTube video ids; anything else could point outside `dir`.
fn file_in(dir: &Path, track_id: &str) -> Option<PathBuf> {
    let valid = !track_id.is_empty()
        && track_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    valid.then(|| dir.join(format!("{track_id}.m4a")))
}

async fn read_gain(track: &Path) -> Option<f32> {
    tokio::fs::read_to_string(track.with_extension(GAIN_EXTENSION))
        .await
        .ok()
        .and_then(|gain| gain.trim().parse().ok())
}

async fn write_gain(track: &Path, gain: f32) -> Result<()> {
    tokio::fs::write(track.with_extension(GAIN_EXTENSION), gain.to_string()).await?;
    Ok(())
}

async fn write_track(dir: &Path, path: &Path, bytes: &[u8], gain: Option<f32>) -> Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    if let Some(gain) = gain {
        write_gain(path, gain).await?;
    }
    // Write aside and rename, so a crash never leaves a truncated file to be played.
    let partial = path.with_extension("part");
    tokio::fs::write(&partial, bytes).await?;
    tokio::fs::rename(&partial, path).await?;
    Ok(())
}

/// The tracks in `dir` with their last-played time and size.
fn tracks_in(dir: &Path) -> Vec<(SystemTime, u64, PathBuf)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "m4a"))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            Some((metadata.modified().ok()?, metadata.len(), entry.path()))
        })
        .collect()
}

/// Deletes the least recently played tracks until the cache fits in `max_bytes`.
fn prune(dir: &Path, max_bytes: u64) -> Result<()> {
    let mut files = tracks_in(dir);
    let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
    files.sort_by_key(|(modified, _, _)| *modified);
    for (_, size, path) in files {
        if total <= max_bytes {
            break;
        }
        if fs::remove_file(&path).is_ok() {
            fs::remove_file(path.with_extension(GAIN_EXTENSION)).ok();
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

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn rejects_ids_that_could_leave_the_cache() {
        let dir = PathBuf::from("cache");
        assert!(file_in(&dir, "dQw4w9WgXcQ").is_some());
        assert!(file_in(&dir, "a-b_c").is_some());
        assert!(file_in(&dir, "../secrets").is_none());
        assert!(file_in(&dir, "a/b").is_none());
        assert!(file_in(&dir, "").is_none());
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
        fs::write(dir.join("old.gain"), "1.2").unwrap();
        fs::write(dir.join("notes.txt"), [0u8; 100]).unwrap();

        prune(&dir, 20).unwrap();

        assert!(!dir.join("old.m4a").exists());
        assert!(!dir.join("old.gain").exists());
        assert!(dir.join("middle.m4a").exists());
        assert!(dir.join("new.m4a").exists());
        // Only cached tracks count and get removed.
        assert!(dir.join("notes.txt").exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn offline_tracks_survive_pruning_and_clearing() {
        let root = temp_dir("offline");
        let cache = AudioCache::new(root.join("cache"), root.join("offline"), 0);
        block_on(async {
            cache
                .write_offline("kept", b"offline audio", Some(0.8))
                .await
                .unwrap();
            // A max of 0 prunes every cached track as soon as it is written.
            cache.write("played", b"cached audio", None).await.unwrap();
            cache.clear().await.unwrap();

            assert!(cache.is_offline("kept"));
            assert_eq!(
                cache.read("kept").await,
                Some((b"offline audio".to_vec(), Some(0.8)))
            );
            assert_eq!(cache.read("played").await, None);
        });
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn keeping_a_cached_track_offline_moves_it_with_its_gain() {
        let root = temp_dir("keep");
        let cache = AudioCache::new(root.join("cache"), root.join("offline"), u64::MAX);
        block_on(async {
            cache.write("song", b"audio", Some(1.1)).await.unwrap();
            assert!(cache.keep_offline("song").await.unwrap());
            assert!(!root.join("cache").join("song.m4a").exists());
            assert_eq!(
                cache.read("song").await,
                Some((b"audio".to_vec(), Some(1.1)))
            );
            // Nothing cached to move: the caller has to download it.
            assert!(!cache.keep_offline("other").await.unwrap());

            cache.remove_offline("song").await.unwrap();
            assert!(!cache.is_offline("song"));
            assert_eq!(cache.read("song").await, None);
        });
        fs::remove_dir_all(&root).ok();
    }
}
