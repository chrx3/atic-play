use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use rustypipe::client::{RustyPipe, RustyPipeQuery};
use rustypipe::model::{
    AlbumItem, ArtistId, ArtistItem, MusicPlaylistItem, Thumbnail, TrackItem, VideoItem,
};
use tokio::process::Command;
use tokio::runtime::Runtime;

use crate::audio_cache::AudioCache;

/// YouTube serves full-length downloads slowly; ranged requests of this size are not throttled.
const CHUNK_SIZE: u64 = 10 * 1024 * 1024;

/// M4A/AAC formats only, because that is what rodio decodes: 141 (256 kbps, Premium), then 140.
const AUDIO_FORMAT: &str = "141/140/bestaudio[ext=m4a]";

/// Enough for nearly any playlist; Liked Music can hold thousands, fetched 100 per request.
const MAX_PLAYLIST_TRACKS: usize = 500;

/// A song as the UI shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artists: String,
    pub album: Option<String>,
    pub duration_secs: Option<u32>,
    pub cover_url: Option<String>,
}

impl From<TrackItem> for Track {
    fn from(item: TrackItem) -> Self {
        Track {
            id: item.id,
            title: item.name,
            artists: artist_names(&item.artists),
            album: item.album.map(|album| album.name),
            duration_secs: item.duration,
            cover_url: largest(&item.cover),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artists: String,
    pub year: Option<u16>,
    pub cover_url: Option<String>,
}

impl From<AlbumItem> for Album {
    fn from(item: AlbumItem) -> Self {
        Album {
            id: item.id,
            title: item.name,
            artists: artist_names(&item.artists),
            year: item.year,
            cover_url: largest(&item.cover),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub subscribers: Option<u64>,
    pub avatar_url: Option<String>,
}

impl From<ArtistItem> for Artist {
    fn from(item: ArtistItem) -> Self {
        Artist {
            id: item.id,
            name: item.name,
            subscribers: item.subscriber_count,
            avatar_url: largest(&item.avatar),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub owner: Option<String>,
    pub track_count: Option<u64>,
    pub cover_url: Option<String>,
}

impl From<MusicPlaylistItem> for Playlist {
    fn from(item: MusicPlaylistItem) -> Self {
        Playlist {
            id: item.id,
            title: item.name,
            owner: item.channel.map(|channel| channel.name),
            track_count: item.track_count,
            cover_url: largest(&item.thumbnail),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlaylistPage {
    pub playlist: Playlist,
    pub tracks: Vec<Track>,
}

/// What the signed-in user has saved, for the library and the sidebar.
#[derive(Debug, Clone, Default)]
pub struct Library {
    pub liked: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
}

#[derive(Debug, Clone, Default)]
pub struct HomeFeed {
    pub trending: Vec<Track>,
    pub top: Vec<Track>,
    pub artists: Vec<Artist>,
    pub new_albums: Vec<Album>,
}

async fn music_playlist_page(query: &RustyPipeQuery, playlist_id: &str) -> Result<PlaylistPage> {
    let mut playlist = query.music_playlist(playlist_id).await?;
    playlist
        .tracks
        .extend_limit(query, MAX_PLAYLIST_TRACKS)
        .await?;
    Ok(PlaylistPage {
        playlist: Playlist {
            id: playlist.id,
            title: playlist.name,
            owner: playlist.channel.map(|channel| channel.name),
            track_count: playlist.track_count,
            cover_url: largest(&playlist.thumbnail),
        },
        tracks: playlist.tracks.items.into_iter().map(Track::from).collect(),
    })
}

async fn youtube_playlist_page(query: &RustyPipeQuery, playlist_id: &str) -> Result<PlaylistPage> {
    let mut playlist = query.playlist(playlist_id).await?;
    playlist
        .videos
        .extend_limit(query, MAX_PLAYLIST_TRACKS)
        .await?;
    Ok(PlaylistPage {
        playlist: Playlist {
            id: playlist.id,
            title: playlist.name,
            owner: playlist.channel.map(|channel| channel.name),
            track_count: Some(playlist.video_count),
            cover_url: largest(&playlist.thumbnail),
        },
        tracks: playlist.videos.items.into_iter().map(Track::from).collect(),
    })
}

impl From<VideoItem> for Track {
    fn from(video: VideoItem) -> Self {
        // Songs on YouTube belong to "<artist> - Topic" channels.
        let artists = video
            .channel
            .map(|channel| {
                channel
                    .name
                    .strip_suffix(" - Topic")
                    .unwrap_or(&channel.name)
                    .to_string()
            })
            .unwrap_or_default();
        Track {
            id: video.id,
            title: video.name,
            artists,
            album: None,
            duration_secs: video.duration,
            cover_url: largest(&video.thumbnail),
        }
    }
}

async fn tracks_or_playlist(
    query: &RustyPipeQuery,
    tracks: Vec<TrackItem>,
    playlist_id: Option<String>,
) -> Vec<Track> {
    if !tracks.is_empty() {
        return tracks.into_iter().map(Track::from).collect();
    }
    let Some(playlist_id) = playlist_id else {
        return Vec::new();
    };
    match query.music_playlist(&playlist_id).await {
        Ok(playlist) => playlist.tracks.items.into_iter().map(Track::from).collect(),
        Err(err) => {
            log::warn!("chart playlist {playlist_id}: {err}");
            Vec::new()
        }
    }
}

fn artist_names(artists: &[ArtistId]) -> String {
    artists
        .iter()
        .map(|artist| artist.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn largest(thumbnails: &[Thumbnail]) -> Option<String> {
    thumbnails
        .iter()
        .max_by_key(|thumb| thumb.width)
        .map(|thumb| thumb.url.clone())
}

/// YouTube Music client. Requests run on an internal tokio runtime, so the returned futures can
/// be awaited from any executor (GPUI's included).
///
/// Search and metadata come from rustypipe. Stream URLs come from yt-dlp, which keeps up with
/// YouTube's signature and PO token changes far better than any embedded extractor.
///
/// Signing in uses the cookies of a YouTube Music session (a Netscape `cookies.txt`): rustypipe
/// reads them for the library, and yt-dlp for Premium audio.
#[derive(Clone)]
pub struct Client {
    runtime: Arc<Runtime>,
    pipe: RustyPipe,
    http: reqwest::Client,
    yt_dlp: OsString,
    /// The session cookies; they exist only while signed in.
    cookies_path: PathBuf,
    signed_in: Arc<AtomicBool>,
    audio_cache: Option<Arc<AudioCache>>,
}

impl Client {
    /// Keeps up to `max_bytes` of downloaded tracks in `dir`, dropping the least recently played.
    pub fn with_audio_cache(mut self, dir: impl Into<PathBuf>, max_bytes: u64) -> Self {
        self.audio_cache = Some(Arc::new(AudioCache::new(dir.into(), max_bytes)));
        self
    }

    /// `data_dir` keeps rustypipe's cache and the session; it is created if missing. Uses the
    /// yt-dlp binary named by the `YT_DLP` environment variable, or `yt-dlp` on PATH.
    pub fn new(data_dir: impl Into<PathBuf>) -> Result<Self> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("could not create {}", data_dir.display()))?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let pipe = RustyPipe::builder().storage_dir(&data_dir).build()?;
        Ok(Client {
            runtime: Arc::new(runtime),
            pipe,
            http: reqwest::Client::new(),
            yt_dlp: std::env::var_os("YT_DLP").unwrap_or_else(|| "yt-dlp".into()),
            cookies_path: data_dir.join("cookies.txt"),
            signed_in: Arc::default(),
            audio_cache: None,
        })
    }

    pub fn is_signed_in(&self) -> bool {
        self.signed_in.load(Ordering::Relaxed)
    }

    /// Signs in again with the session saved last time, if there is one and it still works.
    pub fn restore_session(&self) -> impl Future<Output = Result<bool>> + use<> {
        let client = self.clone();
        self.spawn(async move {
            let cookies = match tokio::fs::read_to_string(&client.cookies_path).await {
                Ok(cookies) => cookies,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(err) => return Err(err.into()),
            };
            client.authenticate(&cookies).await?;
            Ok(true)
        })
    }

    /// Signs in with the contents of a `cookies.txt` exported from a YouTube Music session.
    pub fn sign_in(&self, cookies: String) -> impl Future<Output = Result<()>> + use<> {
        let client = self.clone();
        self.spawn(async move {
            if !has_session_cookie(&cookies) {
                bail!(
                    "el archivo no tiene una sesión de YouTube; exporta las cookies con \
                     music.youtube.com abierto y la sesión iniciada"
                );
            }
            client.authenticate(&cookies).await?;
            tokio::fs::write(&client.cookies_path, &cookies)
                .await
                .context("no se pudo guardar la sesión")?;
            Ok(())
        })
    }

    pub fn sign_out(&self) -> impl Future<Output = Result<()>> + use<> {
        let client = self.clone();
        self.spawn(async move {
            client.signed_in.store(false, Ordering::Relaxed);
            client.pipe.user_auth_remove_cookie().await?;
            match tokio::fs::remove_file(&client.cookies_path).await {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
                _ => Ok(()),
            }
        })
    }

    async fn authenticate(&self, cookies: &str) -> Result<()> {
        self.pipe.user_auth_set_cookie_txt(cookies).await?;
        if let Err(err) = self.pipe.user_auth_check_cookie().await {
            self.pipe.user_auth_remove_cookie().await.ok();
            bail!("la sesión no es válida o ya expiró ({err})");
        }
        self.signed_in.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Liked songs, saved playlists, albums and artists. Requires a session.
    pub fn library(&self) -> impl Future<Output = Result<Library>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let query = pipe.query();
            let (liked, playlists, albums, artists) = tokio::join!(
                query.music_liked_tracks(),
                query.music_saved_playlists(),
                query.music_saved_albums(),
                query.music_saved_artists(),
            );
            Ok(Library {
                liked: liked?.tracks.items.into_iter().map(Track::from).collect(),
                playlists: playlists?.items.into_iter().map(Playlist::from).collect(),
                // Albums and artists are extras: show the rest if one of them fails.
                albums: albums
                    .inspect_err(|err| log::warn!("saved albums: {err}"))
                    .map(|albums| albums.items.into_iter().map(Album::from).collect())
                    .unwrap_or_default(),
                artists: artists
                    .inspect_err(|err| log::warn!("saved artists: {err}"))
                    .map(|artists| artists.items.into_iter().map(Artist::from).collect())
                    .unwrap_or_default(),
            })
        })
    }

    /// A playlist's header and its first `MAX_PLAYLIST_TRACKS` tracks. Private playlists (Liked
    /// Music included) need the session.
    pub fn playlist(
        &self,
        playlist_id: String,
    ) -> impl Future<Output = Result<PlaylistPage>> + use<> {
        let pipe = self.pipe.clone();
        let signed_in = self.is_signed_in();
        self.spawn(async move {
            let query = if signed_in {
                pipe.query().authenticated()
            } else {
                pipe.query()
            };
            match music_playlist_page(&query, &playlist_id).await {
                Ok(page) => Ok(page),
                // When rustypipe does not recognize a playlist header (as with the user's own
                // playlists before the vendored patch), it mistakes the playlist for an album.
                // Every YouTube Music playlist is also a YouTube playlist, whose page parses
                // separately, so fall back to it.
                Err(err) => {
                    log::warn!("music playlist {playlist_id}: {err}; reading it from YouTube");
                    youtube_playlist_page(&query, &playlist_id).await
                }
            }
        })
    }

    pub fn search_tracks(&self, query: String) -> impl Future<Output = Result<Vec<Track>>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let result = pipe.query().music_search_tracks(&query).await?;
            Ok(result.items.items.into_iter().map(Track::from).collect())
        })
    }

    pub fn search_albums(&self, query: String) -> impl Future<Output = Result<Vec<Album>>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let result = pipe.query().music_search_albums(&query).await?;
            Ok(result.items.items.into_iter().map(Album::from).collect())
        })
    }

    pub fn search_artists(
        &self,
        query: String,
    ) -> impl Future<Output = Result<Vec<Artist>>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let result = pipe.query().music_search_artists(&query).await?;
            Ok(result.items.items.into_iter().map(Artist::from).collect())
        })
    }

    pub fn album_tracks(
        &self,
        album_id: String,
    ) -> impl Future<Output = Result<Vec<Track>>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let album = pipe.query().music_album(&album_id).await?;
            // Album pages list tracks without covers; they all share the album's.
            let cover_url = largest(&album.cover);
            Ok(album
                .tracks
                .into_iter()
                .map(|item| {
                    let mut track = Track::from(item);
                    track.album.get_or_insert_with(|| album.name.clone());
                    if track.cover_url.is_none() {
                        track.cover_url = cover_url.clone();
                    }
                    track
                })
                .collect())
        })
    }

    pub fn artist_top_tracks(
        &self,
        artist_id: String,
    ) -> impl Future<Output = Result<Vec<Track>>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let artist = pipe.query().music_artist(&artist_id, false).await?;
            Ok(artist.tracks.into_iter().map(Track::from).collect())
        })
    }

    /// What the home page shows without an account: the charts of the listener's country (YouTube
    /// picks it from the IP) and new releases.
    pub fn home(&self) -> impl Future<Output = Result<HomeFeed>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let query = pipe.query();
            let (charts, new_albums) =
                tokio::join!(query.music_charts(None), query.music_new_albums());
            let charts = charts?;
            // rustypipe 0.11 no longer finds the chart tracks (nor their playlist ids) on the page,
            // but it still lists the chart playlists: "Trending 20" first, the "Top 100" last.
            let trending_id = charts
                .trending_playlist_id
                .or_else(|| charts.playlists.first().map(|playlist| playlist.id.clone()));
            let top_id = charts.top_playlist_id.or_else(|| {
                (charts.playlists.len() > 1)
                    .then(|| charts.playlists.last().map(|playlist| playlist.id.clone()))
                    .flatten()
            });
            let (trending, top) = tokio::join!(
                tracks_or_playlist(&query, charts.trending_tracks, trending_id),
                tracks_or_playlist(&query, charts.top_tracks, top_id),
            );
            Ok(HomeFeed {
                trending,
                top,
                artists: charts.artists.into_iter().map(Artist::from).collect(),
                // New releases are a bonus; the page is still useful without them.
                new_albums: new_albums
                    .inspect_err(|err| log::warn!("new albums: {err}"))
                    .unwrap_or_default()
                    .into_iter()
                    .map(Album::from)
                    .collect(),
            })
        })
    }

    /// Tracks YouTube Music would play after this one (its "radio").
    pub fn radio(&self, track_id: String) -> impl Future<Output = Result<Vec<Track>>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let radio = pipe.query().music_radio_track(&track_id).await?;
            Ok(radio.items.into_iter().map(Track::from).collect())
        })
    }

    /// Downloads the best M4A/AAC stream of a track.
    pub fn fetch_audio(&self, track_id: String) -> impl Future<Output = Result<Vec<u8>>> + use<> {
        let http = self.http.clone();
        let yt_dlp = self.yt_dlp.clone();
        let cookies = self.is_signed_in().then(|| self.cookies_path.clone());
        let cache = self.audio_cache.clone();
        self.spawn(async move {
            if let Some(cache) = &cache
                && let Some(bytes) = cache.read(&track_id).await
            {
                log::info!("track {track_id}: from the audio cache");
                return Ok(bytes);
            }
            let stream = resolve_stream(&yt_dlp, cookies.as_deref(), &track_id).await?;
            log::info!("track {track_id}: itag {}", stream.format_id);
            let bytes = download(&http, &stream).await?;
            if let Some(cache) = &cache
                && let Err(err) = cache.write(&track_id, &bytes).await
            {
                log::warn!("could not cache {track_id}: {err:#}");
            }
            Ok(bytes)
        })
    }

    /// Downloads a cover at `size_px` into the cache directory and returns its path. Covers are
    /// cached per `key` (a track, album or artist id) and size, so asking again is free.
    pub fn fetch_cover(
        &self,
        key: String,
        url: String,
        size_px: u32,
    ) -> impl Future<Output = Result<PathBuf>> + use<> {
        let http = self.http.clone();
        self.spawn(async move {
            let dir = std::env::temp_dir().join("ytm-player").join("covers");
            let path = dir.join(format!("{key}-{size_px}.jpg"));
            if tokio::fs::try_exists(&path).await? {
                return Ok(path);
            }
            let bytes = http
                .get(sized_cover_url(&url, size_px))
                .send()
                .await?
                .error_for_status()?
                .bytes()
                .await?;
            tokio::fs::create_dir_all(&dir).await?;
            tokio::fs::write(&path, &bytes).await?;
            Ok(path)
        })
    }

    fn spawn<T, F>(&self, task: F) -> impl Future<Output = Result<T>> + use<T, F>
    where
        T: Send + 'static,
        F: Future<Output = Result<T>> + Send + 'static,
    {
        let handle = self.runtime.spawn(task);
        async move { handle.await.context("YouTube Music task panicked")? }
    }
}

/// YouTube Music covers (`lh3.googleusercontent.com/...=w60-h60-l90-rj`) can be requested at any
/// size by rewriting the parameters after `=`. Other URLs are left as they are.
fn sized_cover_url(url: &str, size_px: u32) -> String {
    match url.rsplit_once('=') {
        Some((base, params))
            if url.contains("googleusercontent.com") && params.starts_with('w') =>
        {
            format!("{base}=w{size_px}-h{size_px}-l90-rj")
        }
        _ => url.to_string(),
    }
}

struct ResolvedStream {
    format_id: String,
    url: String,
    user_agent: Option<String>,
    size: Option<u64>,
}

/// Whether a `cookies.txt` holds a signed-in YouTube session (the cookie YouTube signs API
/// requests with).
fn has_session_cookie(cookies: &str) -> bool {
    cookies.lines().any(|line| {
        let fields: Vec<&str> = line.split('\t').collect();
        fields.len() >= 7
            && fields[0]
                .trim_start_matches("#HttpOnly_")
                .ends_with("youtube.com")
            && matches!(fields[5], "SAPISID" | "__Secure-3PAPISID")
    })
}

async fn resolve_stream(
    yt_dlp: &OsString,
    cookies: Option<&Path>,
    track_id: &str,
) -> Result<ResolvedStream> {
    let mut command = Command::new(yt_dlp);
    if let Some(cookies) = cookies {
        // yt-dlp also writes refreshed cookies back to this file, which keeps the session alive.
        command.arg("--cookies").arg(cookies);
    }
    command
        // Signed in, yt-dlp uses web clients whose stream URLs need YouTube's JavaScript
        // challenge solved. It only looks for Deno by default; this also allows Node.
        .args(["--js-runtimes", "node"])
        .args(["--no-warnings", "--no-playlist", "-f", AUDIO_FORMAT])
        .args([
            "-O",
            "%(format_id)s",
            "-O",
            "%(filesize)s",
            "-O",
            "%(http_headers)j",
        ])
        .args(["-O", "%(url)s"])
        .arg(format!("https://music.youtube.com/watch?v={track_id}"))
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // Keep yt-dlp from flashing a console window over the GUI.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let output = command
        .output()
        .await
        .with_context(|| format!("could not run {}", yt_dlp.to_string_lossy()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8(output.stdout)?;
    let mut lines = stdout.lines();
    let mut next = |field: &str| {
        lines
            .next()
            .ok_or_else(|| anyhow!("yt-dlp printed no {field}"))
    };
    let format_id = next("format")?.to_string();
    let size = next("size")?.parse().ok();
    let headers: serde_json::Value = serde_json::from_str(next("headers")?).unwrap_or_default();
    let url = next("url")?.to_string();

    Ok(ResolvedStream {
        format_id,
        url,
        user_agent: headers["User-Agent"].as_str().map(str::to_string),
        size,
    })
}

async fn download(http: &reqwest::Client, stream: &ResolvedStream) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(stream.size.unwrap_or(0) as usize);
    loop {
        let start = bytes.len() as u64;
        let end = start + CHUNK_SIZE - 1;
        let mut request = http
            .get(&stream.url)
            .header(reqwest::header::RANGE, format!("bytes={start}-{end}"));
        if let Some(user_agent) = &stream.user_agent {
            request = request.header(reqwest::header::USER_AGENT, user_agent);
        }
        let chunk = request.send().await?.error_for_status()?.bytes().await?;
        bytes.extend_from_slice(&chunk);

        let done = match stream.size {
            Some(size) => bytes.len() as u64 >= size,
            // Without a known size, a short chunk means the end of the file.
            None => (chunk.len() as u64) < CHUNK_SIZE,
        };
        if done || chunk.is_empty() {
            return Ok(bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fake values: only the domain and cookie names matter here.
    const SIGNED_IN: &str = "# Netscape HTTP Cookie File\n\
        .youtube.com\tTRUE\t/\tTRUE\t1893456000\tPREF\tf6=40000000\n\
        .youtube.com\tTRUE\t/\tTRUE\t1893456000\t__Secure-3PAPISID\tfake-value\n";
    const SIGNED_OUT: &str = "# Netscape HTTP Cookie File\n\
        .youtube.com\tTRUE\t/\tTRUE\t1893456000\tPREF\tf6=40000000\n\
        .youtube.com\tTRUE\t/\tTRUE\t1893456000\tVISITOR_INFO1_LIVE\tfake-value\n";
    const OTHER_SITE: &str = ".example.com\tTRUE\t/\tTRUE\t1893456000\tSAPISID\tfake-value\n";

    #[test]
    fn session_cookie_needs_a_youtube_sapisid() {
        assert!(has_session_cookie(SIGNED_IN));
        assert!(!has_session_cookie(SIGNED_OUT));
        assert!(!has_session_cookie(OTHER_SITE));
        assert!(!has_session_cookie("not a cookie file"));
    }

    #[test]
    fn session_cookie_accepts_http_only_lines() {
        let http_only = "#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1893456000\tSAPISID\tfake-value\n";
        assert!(has_session_cookie(http_only));
    }

    #[test]
    fn cover_urls_are_resized() {
        assert_eq!(
            sized_cover_url("https://lh3.googleusercontent.com/abc=w60-h60-l90-rj", 544),
            "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj"
        );
        let video = "https://i.ytimg.com/vi/abc/hqdefault.jpg";
        assert_eq!(sized_cover_url(video, 544), video);
    }
}
