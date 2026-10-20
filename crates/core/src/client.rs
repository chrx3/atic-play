use std::collections::HashMap;
use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use rustypipe::client::{ClientType, RustyPipe, RustyPipeQuery};
use rustypipe::model::paginator::Paginator;
use rustypipe::model::traits::FromYtItem;
use rustypipe::model::{
    AlbumItem, ArtistId, ArtistItem, MusicItem, MusicPlaylistItem, Thumbnail, TrackItem, VideoItem,
};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::runtime::Runtime;
use tokio::sync::{OnceCell, Semaphore};

use crate::audio_cache::AudioCache;
use crate::loudness::normalization_gain;

/// YouTube serves full-length downloads slowly; ranged requests of this size are not throttled.
const CHUNK_SIZE: u64 = 10 * 1024 * 1024;

/// M4A/AAC formats only, because that is what rodio decodes: 141 (256 kbps, Premium), then 140.
/// How long yt-dlp may take to resolve a track before it is killed.
const YT_DLP_TIMEOUT: Duration = Duration::from_secs(60);
/// Stream URLs last about six hours; one resolved ahead is used for well under that.
const RESOLUTION_LIFETIME: Duration = Duration::from_secs(60 * 60);
/// yt-dlp runs at once for tracks resolved ahead (each takes a second or more of CPU).
const MAX_WARM_UPS: usize = 2;
/// Attempts per chunk before a download gives up.
const CHUNK_ATTEMPTS: u32 = 3;
const AUDIO_FORMAT: &str = "141/140/bestaudio[ext=m4a]";

/// Enough for nearly any playlist; Liked Music can hold thousands, fetched 100 per request.
const MAX_PLAYLIST_TRACKS: usize = 500;
/// Saved playlists, albums and artists read for the library.
const MAX_LIBRARY_ITEMS: usize = 500;

/// A song as the UI shows it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artists: String,
    pub album: Option<String>,
    pub duration_secs: Option<u32>,
    pub cover_url: Option<String>,
    /// For "go to album" and "go to artist". Missing in queues saved by older versions.
    #[serde(default)]
    pub album_id: Option<String>,
    #[serde(default)]
    pub artist_id: Option<String>,
}

impl From<TrackItem> for Track {
    fn from(item: TrackItem) -> Self {
        Track {
            id: item.id,
            title: item.name,
            artists: artist_names(&item.artists),
            album_id: item.album.as_ref().map(|album| album.id.clone()),
            album: item.album.map(|album| album.name),
            artist_id: item
                .artist_id
                .or_else(|| item.artists.iter().find_map(|artist| artist.id.clone())),
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

/// A downloaded track, ready to play.
#[derive(Debug, Clone)]
pub struct Audio {
    /// The M4A/AAC file.
    pub bytes: Vec<u8>,
    /// The volume factor that normalizes it, if it could be measured.
    pub gain: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct AlbumPage {
    pub album: Album,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone)]
pub struct ArtistPage {
    pub artist: Artist,
    pub description: Option<String>,
    pub top_tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub similar: Vec<Artist>,
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

/// What YouTube Music suggests while the user types a search.
#[derive(Debug, Clone, Default)]
pub struct SearchSuggestions {
    /// Completed search terms.
    pub terms: Vec<String>,
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
}

/// One page of a list and, when there is more, how to ask for the next one with
/// [`Client::more`].
pub struct Page<T> {
    pub items: Vec<T>,
    pub more: Option<More<T>>,
}

type NextPage<T> =
    dyn Fn(RustyPipe) -> Pin<Box<dyn Future<Output = Result<Page<T>>> + Send>> + Send + Sync;

/// The rest of a list.
pub struct More<T> {
    next: Arc<NextPage<T>>,
}

impl<T> Clone for More<T> {
    fn clone(&self) -> Self {
        More {
            next: self.next.clone(),
        }
    }
}

impl<T> std::fmt::Debug for More<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("More")
    }
}

/// A page from a rustypipe paginator, converting its items.
fn page_from<I, T>(mut paginator: Paginator<I>) -> Page<T>
where
    I: FromYtItem + Clone + Send + Sync + 'static,
    T: From<I> + Send + 'static,
{
    let items = std::mem::take(&mut paginator.items)
        .into_iter()
        .map(T::from)
        .collect();
    let more = paginator.ctoken.is_some().then(|| More {
        next: Arc::new(move |pipe: RustyPipe| {
            let paginator = paginator.clone();
            let next: Pin<Box<dyn Future<Output = Result<Page<T>>> + Send>> =
                Box::pin(async move {
                    Ok(match paginator.next(pipe.query()).await? {
                        Some(next) => page_from(next),
                        None => Page {
                            items: Vec::new(),
                            more: None,
                        },
                    })
                });
            next
        }) as Arc<NextPage<T>>,
    });
    Page { items, more }
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
            album_id: None,
            artist_id: None,
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
    resolver: Resolver,
    /// The session cookies; they exist only while signed in.
    cookies_path: PathBuf,
    signed_in: Arc<AtomicBool>,
    audio_cache: Option<Arc<AudioCache>>,
}

impl Client {
    /// Keeps up to `max_bytes` of downloaded tracks in `dir`, dropping the least recently played.
    /// Downloaded tracks the user keeps offline go in `offline_dir`, which is never pruned.
    pub fn with_audio_cache(
        mut self,
        dir: impl Into<PathBuf>,
        offline_dir: impl Into<PathBuf>,
        max_bytes: u64,
    ) -> Self {
        self.audio_cache = Some(Arc::new(AudioCache::new(
            dir.into(),
            offline_dir.into(),
            max_bytes,
        )));
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
            // No overall timeout: a track downloads in chunks for as long as it takes, but a
            // connection that stops sending fails instead of leaving the player loading forever.
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .read_timeout(Duration::from_secs(30))
                .build()?,
            resolver: Resolver::new(std::env::var_os("YT_DLP").unwrap_or_else(|| "yt-dlp".into())),
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
            // Each list comes in pages; read them all (up to a limit), so the library is
            // complete and older liked songs show as liked.
            let liked = async {
                let mut liked = query.music_liked_tracks().await?;
                if let Err(err) = liked.tracks.extend_limit(&query, MAX_PLAYLIST_TRACKS).await {
                    log::warn!("liked songs: no more pages: {err}");
                }
                Ok::<_, rustypipe::error::Error>(liked.tracks.items)
            };
            let (liked, playlists, albums, artists) = tokio::join!(
                liked,
                all_pages(&query, query.music_saved_playlists()),
                all_pages(&query, query.music_saved_albums()),
                all_pages(&query, query.music_saved_artists()),
            );
            Ok(Library {
                liked: liked?.into_iter().map(Track::from).collect(),
                playlists: playlists?.into_iter().map(Playlist::from).collect(),
                // Albums and artists are extras: show the rest if one of them fails.
                albums: albums
                    .inspect_err(|err| log::warn!("saved albums: {err}"))
                    .map(|albums| albums.into_iter().map(Album::from).collect())
                    .unwrap_or_default(),
                artists: artists
                    .inspect_err(|err| log::warn!("saved artists: {err}"))
                    .map(|artists| artists.into_iter().map(Artist::from).collect())
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

    pub fn search_tracks(
        &self,
        query: String,
    ) -> impl Future<Output = Result<Page<Track>>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let result = pipe.query().music_search_tracks(&query).await?;
            Ok(page_from(result.items))
        })
    }

    pub fn search_albums(
        &self,
        query: String,
    ) -> impl Future<Output = Result<Page<Album>>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let result = pipe.query().music_search_albums(&query).await?;
            Ok(page_from(result.items))
        })
    }

    pub fn search_artists(
        &self,
        query: String,
    ) -> impl Future<Output = Result<Page<Artist>>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let result = pipe.query().music_search_artists(&query).await?;
            Ok(page_from(result.items))
        })
    }

    /// The next page of a list.
    pub fn more<T: Send + 'static>(
        &self,
        more: More<T>,
    ) -> impl Future<Output = Result<Page<T>>> + use<T> {
        let pipe = self.pipe.clone();
        self.spawn(async move { (more.next)(pipe).await })
    }

    pub fn search_suggestions(
        &self,
        query: String,
    ) -> impl Future<Output = Result<SearchSuggestions>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let result = pipe.query().music_search_suggestion(&query).await?;
            let mut suggestions = SearchSuggestions {
                terms: result.terms,
                ..SearchSuggestions::default()
            };
            for item in result.items {
                match item {
                    MusicItem::Track(track) => suggestions.tracks.push(track.into()),
                    MusicItem::Album(album) => suggestions.albums.push(album.into()),
                    MusicItem::Artist(artist) => suggestions.artists.push(artist.into()),
                    MusicItem::Playlist(_) | MusicItem::User(_) => {}
                }
            }
            Ok(suggestions)
        })
    }

    pub fn album_tracks(
        &self,
        album_id: String,
    ) -> impl Future<Output = Result<Vec<Track>>> + use<> {
        let album = self.album(album_id);
        async move { Ok(album.await?.tracks) }
    }

    /// An album's header and tracks.
    pub fn album(&self, album_id: String) -> impl Future<Output = Result<AlbumPage>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let album = pipe.query().music_album(&album_id).await?;
            // Album pages list tracks without covers; they all share the album's.
            let cover_url = largest(&album.cover);
            let tracks = album
                .tracks
                .into_iter()
                .map(|item| {
                    let mut track = Track::from(item);
                    track.album.get_or_insert_with(|| album.name.clone());
                    track.album_id.get_or_insert_with(|| album.id.clone());
                    if track.cover_url.is_none() {
                        track.cover_url = cover_url.clone();
                    }
                    track
                })
                .collect();
            Ok(AlbumPage {
                album: Album {
                    id: album.id,
                    title: album.name,
                    artists: artist_names(&album.artists),
                    year: album.year,
                    cover_url,
                },
                tracks,
            })
        })
    }

    pub fn artist_top_tracks(
        &self,
        artist_id: String,
    ) -> impl Future<Output = Result<Vec<Track>>> + use<> {
        let artist = self.artist(artist_id);
        async move { Ok(artist.await?.top_tracks) }
    }

    /// An artist's page: top songs, albums and singles, and similar artists.
    pub fn artist(&self, artist_id: String) -> impl Future<Output = Result<ArtistPage>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let artist = pipe.query().music_artist(&artist_id, false).await?;
            Ok(ArtistPage {
                artist: Artist {
                    id: artist.id,
                    name: artist.name,
                    subscribers: artist.subscriber_count,
                    avatar_url: largest(&artist.header_image),
                },
                description: artist.description,
                top_tracks: artist.tracks.into_iter().map(Track::from).collect(),
                albums: artist.albums.into_iter().map(Album::from).collect(),
                similar: artist
                    .similar_artists
                    .into_iter()
                    .map(Artist::from)
                    .collect(),
            })
        })
    }

    /// What the signed-in user played recently, newest first and without repeats, for
    /// "Escuchar de nuevo". Requires a session.
    pub fn history(&self) -> impl Future<Output = Result<Vec<Track>>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let history = pipe.query().authenticated().music_history().await?;
            let mut seen = std::collections::HashSet::new();
            Ok(history
                .items
                .into_iter()
                .map(|entry| Track::from(entry.item))
                .filter(|track| seen.insert(track.id.clone()))
                .collect())
        })
    }

    /// Likes a track in the user's YouTube Music account, or takes the like back. Requires a
    /// session.
    pub fn set_liked(
        &self,
        track_id: String,
        liked: bool,
    ) -> impl Future<Output = Result<()>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let endpoint = if liked {
                "like/like"
            } else {
                "like/removelike"
            };
            let body = serde_json::json!({ "target": { "videoId": track_id } });
            pipe.query()
                .authenticated()
                .raw(ClientType::DesktopMusic, endpoint, &body)
                .await?;
            Ok(())
        })
    }

    /// The user's own playlists, the ones songs can be added to (Liked Music is left out: it
    /// changes by liking). Requires a session.
    pub fn editable_playlists(&self) -> impl Future<Output = Result<Vec<Playlist>>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            // YouTube Music's "Save to playlist" dialog lists them; it asks about a song, but
            // the list is the same for any.
            let body = serde_json::json!({ "videoIds": [ANY_VIDEO_ID] });
            let response = pipe
                .query()
                .authenticated()
                .raw(
                    ClientType::DesktopMusic,
                    "playlist/get_add_to_playlist",
                    &body,
                )
                .await?;
            parse_editable_playlists(&response)
        })
    }

    /// Adds songs to one of the user's playlists; ones already in it are skipped.
    pub fn add_to_playlist(
        &self,
        playlist_id: String,
        track_ids: Vec<String>,
    ) -> impl Future<Output = Result<()>> + use<> {
        let actions = track_ids
            .into_iter()
            .map(|id| {
                serde_json::json!({
                    "action": "ACTION_ADD_VIDEO",
                    "addedVideoId": id,
                    "dedupeOption": "DEDUPE_OPTION_SKIP",
                })
            })
            .collect();
        self.edit_playlist(playlist_id, actions)
    }

    /// Takes a song out of one of the user's playlists.
    pub fn remove_from_playlist(
        &self,
        playlist_id: String,
        track_id: String,
    ) -> impl Future<Output = Result<()>> + use<> {
        let action = serde_json::json!({
            "action": "ACTION_REMOVE_VIDEO_BY_VIDEO_ID",
            "removedVideoId": track_id,
        });
        self.edit_playlist(playlist_id, vec![action])
    }

    pub fn rename_playlist(
        &self,
        playlist_id: String,
        title: String,
    ) -> impl Future<Output = Result<()>> + use<> {
        let action = serde_json::json!({
            "action": "ACTION_SET_PLAYLIST_NAME",
            "playlistName": title,
        });
        self.edit_playlist(playlist_id, vec![action])
    }

    /// Creates a private playlist with these songs and returns its id.
    pub fn create_playlist(
        &self,
        title: String,
        track_ids: Vec<String>,
    ) -> impl Future<Output = Result<String>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            let body = serde_json::json!({
                "title": title,
                "description": "",
                "privacyStatus": "PRIVATE",
                "videoIds": track_ids,
            });
            let response = pipe
                .query()
                .authenticated()
                .raw(ClientType::DesktopMusic, "playlist/create", &body)
                .await?;
            let response: serde_json::Value = serde_json::from_str(&response)?;
            response["playlistId"]
                .as_str()
                .map(str::to_string)
                .context("YouTube Music did not return the new playlist")
        })
    }

    /// Deletes one of the user's playlists from their account. It cannot be undone.
    pub fn delete_playlist(&self, playlist_id: String) -> impl Future<Output = Result<()>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            ensure_editable(&playlist_id)?;
            let body = serde_json::json!({ "playlistId": playlist_id });
            pipe.query()
                .authenticated()
                .raw(ClientType::DesktopMusic, "playlist/delete", &body)
                .await?;
            Ok(())
        })
    }

    fn edit_playlist(
        &self,
        playlist_id: String,
        actions: Vec<serde_json::Value>,
    ) -> impl Future<Output = Result<()>> + use<> {
        let pipe = self.pipe.clone();
        self.spawn(async move {
            ensure_editable(&playlist_id)?;
            let body = serde_json::json!({ "playlistId": playlist_id, "actions": actions });
            let response = pipe
                .query()
                .authenticated()
                .raw(ClientType::DesktopMusic, "browse/edit_playlist", &body)
                .await?;
            let response: serde_json::Value = serde_json::from_str(&response)?;
            match response["status"].as_str() {
                None | Some("STATUS_SUCCEEDED") => Ok(()),
                Some(status) => bail!("YouTube Music refused the change ({status})"),
            }
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

    /// Downloads the best M4A/AAC stream of a track, with its normalization gain.
    pub fn fetch_audio(&self, track_id: String) -> impl Future<Output = Result<Audio>> + use<> {
        let http = self.http.clone();
        let resolver = self.resolver.clone();
        let cookies = self.is_signed_in().then(|| self.cookies_path.clone());
        let cache = self.audio_cache.clone();
        self.spawn(async move {
            let cached = match &cache {
                Some(cache) => cache.read(&track_id).await,
                None => None,
            };
            let (bytes, gain, fresh) = match cached {
                Some((bytes, Some(gain))) => {
                    log::info!("track {track_id}: from disk");
                    (bytes, Some(gain), false)
                }
                // Cached before normalization existed: measure it once now.
                Some((bytes, None)) => {
                    let gain = measure_gain(&bytes).await?;
                    (bytes, gain, false)
                }
                None => {
                    let (bytes, gain) =
                        fetch_fresh(&http, &resolver, cookies.as_deref(), &track_id).await?;
                    (bytes, gain, true)
                }
            };
            if let Some(cache) = &cache
                && (fresh || gain.is_some())
                && let Err(err) = cache.write(&track_id, &bytes, gain).await
            {
                log::warn!("could not cache {track_id}: {err:#}");
            }
            Ok(Audio { bytes, gain })
        })
    }

    /// Resolves a track's stream URL in the background, so playing it soon after starts sooner:
    /// yt-dlp takes seconds, and a play request joins a resolution already under way. Does
    /// nothing for tracks already on disk.
    pub fn warm_up(&self, track_id: String) {
        if self
            .audio_cache
            .as_ref()
            .is_some_and(|cache| cache.contains(&track_id))
        {
            return;
        }
        let resolver = self.resolver.clone();
        let cookies = self.is_signed_in().then(|| self.cookies_path.clone());
        self.runtime.spawn(async move {
            let Ok(_permit) = resolver.warm_ups.acquire().await else {
                return;
            };
            if let Err(err) = resolver.resolve(cookies.as_deref(), &track_id).await {
                log::debug!("warm-up of {track_id}: {err:#}");
            }
        });
    }

    /// Keeps a track on disk for offline listening: moved from the cache if it was played,
    /// downloaded otherwise.
    pub fn download(&self, track_id: String) -> impl Future<Output = Result<()>> + use<> {
        let http = self.http.clone();
        let resolver = self.resolver.clone();
        let cookies = self.is_signed_in().then(|| self.cookies_path.clone());
        let cache = self.audio_cache.clone();
        self.spawn(async move {
            let cache = cache.context("offline downloads need the audio cache")?;
            if cache.keep_offline(&track_id).await? {
                return Ok(());
            }
            let (bytes, gain) =
                fetch_fresh(&http, &resolver, cookies.as_deref(), &track_id).await?;
            cache.write_offline(&track_id, &bytes, gain).await
        })
    }

    /// Stops keeping a track offline.
    pub fn remove_download(&self, track_id: String) -> impl Future<Output = Result<()>> + use<> {
        let cache = self.audio_cache.clone();
        self.spawn(async move {
            match cache {
                Some(cache) => cache.remove_offline(&track_id).await,
                None => Ok(()),
            }
        })
    }

    pub fn is_downloaded(&self, track_id: &str) -> bool {
        self.audio_cache
            .as_ref()
            .is_some_and(|cache| cache.is_offline(track_id))
    }

    /// The size of the cache of played tracks, without the offline ones.
    pub fn audio_cache_bytes(&self) -> impl Future<Output = Result<u64>> + use<> {
        let cache = self.audio_cache.clone();
        self.spawn(async move {
            Ok(match cache {
                Some(cache) => cache.cache_bytes().await,
                None => 0,
            })
        })
    }

    /// Empties the cache of played tracks; offline tracks stay.
    pub fn clear_audio_cache(&self) -> impl Future<Output = Result<()>> + use<> {
        let cache = self.audio_cache.clone();
        self.spawn(async move {
            match cache {
                Some(cache) => cache.clear().await,
                None => Ok(()),
            }
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
            // The same cover is often requested twice at once (the player and the queue panel).
            // Each download writes its own file and renames it into place, so nobody ever reads
            // a half-written cover.
            static WRITES: AtomicU64 = AtomicU64::new(0);
            let partial = dir.join(format!(
                "{key}-{size_px}.{}-{}.part",
                std::process::id(),
                WRITES.fetch_add(1, Ordering::Relaxed)
            ));
            tokio::fs::write(&partial, &bytes).await?;
            if let Err(err) = tokio::fs::rename(&partial, &path).await {
                // Another download got there first; its file is just as good.
                tokio::fs::remove_file(&partial).await.ok();
                if !tokio::fs::try_exists(&path).await? {
                    return Err(err.into());
                }
            }
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

#[derive(Clone)]
struct ResolvedStream {
    format_id: String,
    url: String,
    user_agent: Option<String>,
    size: Option<u64>,
}

/// Stream URLs from yt-dlp, each resolved once and shared: a play request joins a warm-up
/// already under way instead of starting yt-dlp again.
#[derive(Clone)]
struct Resolver {
    yt_dlp: OsString,
    resolutions: Arc<Mutex<HashMap<String, Resolution>>>,
    warm_ups: Arc<Semaphore>,
}

struct Resolution {
    started: Instant,
    stream: Arc<OnceCell<ResolvedStream>>,
}

impl Resolver {
    fn new(yt_dlp: OsString) -> Self {
        Resolver {
            yt_dlp,
            resolutions: Arc::default(),
            warm_ups: Arc::new(Semaphore::new(MAX_WARM_UPS)),
        }
    }

    async fn resolve(&self, cookies: Option<&Path>, track_id: &str) -> Result<ResolvedStream> {
        let key = resolution_key(cookies, track_id);
        let stream = {
            let mut resolutions = self
                .resolutions
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            resolutions.retain(|_, resolution| resolution.started.elapsed() < RESOLUTION_LIFETIME);
            resolutions
                .entry(key)
                .or_insert_with(|| Resolution {
                    started: Instant::now(),
                    stream: Arc::default(),
                })
                .stream
                .clone()
        };
        // A failed resolution is not kept, so the next request tries again.
        let stream = stream
            .get_or_try_init(|| resolve_stream(&self.yt_dlp, cookies, track_id))
            .await?;
        Ok(stream.clone())
    }

    /// Drops a resolved URL that stopped working, so the next request resolves it again.
    fn forget(&self, cookies: Option<&Path>, track_id: &str) {
        let key = resolution_key(cookies, track_id);
        self.resolutions
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(&key);
    }
}

/// Signed in and signed out resolutions differ (Premium formats), so they are kept apart.
fn resolution_key(cookies: Option<&Path>, track_id: &str) -> String {
    format!("{}:{track_id}", cookies.is_some())
}

/// Resolves a track with yt-dlp, downloads it and measures its normalization gain.
async fn fetch_fresh(
    http: &reqwest::Client,
    resolver: &Resolver,
    cookies: Option<&Path>,
    track_id: &str,
) -> Result<(Vec<u8>, Option<f32>)> {
    let started = Instant::now();
    let stream = resolver.resolve(cookies, track_id).await?;
    log::info!(
        "track {track_id}: itag {} after {:.1}s",
        stream.format_id,
        started.elapsed().as_secs_f32()
    );
    let bytes = match download(http, &stream).await {
        Ok(bytes) => bytes,
        // A URL resolved ahead may have been refused (expired, or tied to another session):
        // resolve it once more.
        Err(err) if is_forbidden(&err) => {
            log::info!("track {track_id}: stream URL refused, resolving again");
            resolver.forget(cookies, track_id);
            let stream = resolver.resolve(cookies, track_id).await?;
            download(http, &stream).await?
        }
        Err(err) => {
            resolver.forget(cookies, track_id);
            return Err(err);
        }
    };
    let gain = measure_gain(&bytes).await?;
    // Each URL is downloaded once; replays come from the disk cache.
    resolver.forget(cookies, track_id);
    Ok((bytes, gain))
}

fn is_forbidden(err: &anyhow::Error) -> bool {
    err.downcast_ref::<reqwest::Error>()
        .and_then(reqwest::Error::status)
        .is_some_and(|status| status == reqwest::StatusCode::FORBIDDEN)
}

/// Decodes the whole track to measure it, off the async workers.
async fn measure_gain(bytes: &[u8]) -> Result<Option<f32>> {
    let audio = bytes.to_vec();
    Ok(tokio::task::spawn_blocking(move || normalization_gain(&audio)).await?)
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

    // `kill_on_drop` stops yt-dlp when the timeout drops its future.
    let output = tokio::time::timeout(YT_DLP_TIMEOUT, command.output())
        .await
        .map_err(|_| anyhow!("yt-dlp took more than {}s", YT_DLP_TIMEOUT.as_secs()))?
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
        let chunk_len = download_chunk(http, stream, start, end, &mut bytes).await?;

        let done = match stream.size {
            Some(size) => bytes.len() as u64 >= size,
            // Without a known size, a short chunk means the end of the file.
            None => (chunk_len as u64) < CHUNK_SIZE,
        };
        if done || chunk_len == 0 {
            return Ok(bytes);
        }
    }
}

/// Appends one range to `bytes` and returns its length, retrying network and server errors so one
/// bad chunk doesn't lose the whole track.
async fn download_chunk(
    http: &reqwest::Client,
    stream: &ResolvedStream,
    start: u64,
    end: u64,
    bytes: &mut Vec<u8>,
) -> Result<usize> {
    let mut attempt = 1;
    loop {
        let mut request = http
            .get(&stream.url)
            .header(reqwest::header::RANGE, format!("bytes={start}-{end}"));
        if let Some(user_agent) = &stream.user_agent {
            request = request.header(reqwest::header::USER_AGENT, user_agent);
        }
        let result = async { request.send().await?.error_for_status()?.bytes().await }.await;
        match result {
            Ok(chunk) => {
                bytes.extend_from_slice(&chunk);
                return Ok(chunk.len());
            }
            // A 4xx (an expired or forbidden URL) won't get better by asking again.
            Err(err) if err.status().is_some_and(|status| status.is_client_error()) => {
                return Err(err.into());
            }
            Err(err) if attempt < CHUNK_ATTEMPTS => {
                log::warn!("chunk at {start} failed (attempt {attempt}): {err}");
                tokio::time::sleep(Duration::from_millis(500 * u64::from(attempt))).await;
                attempt += 1;
            }
            Err(err) => return Err(err.into()),
        }
    }
}

/// Any public video, for requests that need one but whose answer does not depend on it.
const ANY_VIDEO_ID: &str = "jNQXAC9IVRw";

/// Lists YouTube keeps itself (Liked Music, Episodes for Later…) have short ids, not the `PL…`
/// of playlists users make; they must not be edited or deleted this way.
fn ensure_editable(playlist_id: &str) -> Result<()> {
    if playlist_id.starts_with("PL") {
        Ok(())
    } else {
        bail!("{playlist_id} is not a playlist of the user")
    }
}

/// The playlists in a "Save to playlist" dialog that songs can be added to and removed from.
fn parse_editable_playlists(response: &str) -> Result<Vec<Playlist>> {
    let response: serde_json::Value = serde_json::from_str(response)?;
    let options = response["contents"][0]["addToPlaylistRenderer"]["playlists"]
        .as_array()
        .context("YouTube Music sent no playlists")?;
    Ok(options
        .iter()
        .map(|option| &option["playlistAddToOptionRenderer"])
        .filter(|option| !option["removeFromPlaylistServiceEndpoint"].is_null())
        .filter_map(|option| {
            let id = option["playlistId"].as_str()?;
            ensure_editable(id).ok()?;
            let title = text_of(&option["title"])?;
            // "6 tracks", "1 canción"…: the first number.
            let track_count = text_of(&option["shortBylineText"]).and_then(|byline| {
                byline
                    .split_whitespace()
                    .next()
                    .and_then(|count| count.replace([',', '.'], "").parse().ok())
            });
            let cover_url =
                option["thumbnailRenderer"]["musicThumbnailRenderer"]["thumbnail"]["thumbnails"]
                    .as_array()
                    .and_then(|thumbnails| thumbnails.last())
                    .and_then(|thumbnail| thumbnail["url"].as_str())
                    .map(str::to_string);
            Some(Playlist {
                id: id.to_string(),
                title,
                owner: None,
                track_count,
                cover_url,
            })
        })
        .collect())
}

/// The text of a YouTube `simpleText` or `runs` object.
fn text_of(text: &serde_json::Value) -> Option<String> {
    if let Some(simple) = text["simpleText"].as_str() {
        return Some(simple.to_string());
    }
    let runs = text["runs"].as_array()?;
    Some(runs.iter().filter_map(|run| run["text"].as_str()).collect())
}

/// Every page of a list, up to `MAX_LIBRARY_ITEMS`; the first one at least.
async fn all_pages<T: FromYtItem>(
    query: &RustyPipeQuery,
    first: impl Future<Output = Result<Paginator<T>, rustypipe::error::Error>>,
) -> Result<Vec<T>, rustypipe::error::Error> {
    let mut paginator = first.await?;
    // YouTube sometimes hands out a continuation whose page is empty (the saved playlists do),
    // which rustypipe reports as an error: keep what came.
    if let Err(err) = paginator.extend_limit(query, MAX_LIBRARY_ITEMS).await {
        log::warn!(
            "library: no more pages after {}: {err}",
            paginator.items.len()
        );
    }
    Ok(paginator.items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editable_playlists_leave_out_liked_music() {
        let response = r#"{"contents":[{"addToPlaylistRenderer":{"playlists":[
            {"playlistAddToOptionRenderer":{"playlistId":"LM","title":{"simpleText":"Liked Music"},
                "addToPlaylistServiceEndpoint":{}}},
            {"playlistAddToOptionRenderer":{"playlistId":"PLabc","title":{"runs":[{"text":"aaa"}]},
                "shortBylineText":{"runs":[{"text":"6"},{"text":" tracks"}]},
                "addToPlaylistServiceEndpoint":{},"removeFromPlaylistServiceEndpoint":{},
                "thumbnailRenderer":{"musicThumbnailRenderer":{"thumbnail":{"thumbnails":[
                    {"url":"small"},{"url":"large"}]}}}}}
        ]}}]}"#;
        let playlists = parse_editable_playlists(response).unwrap();
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].id, "PLabc");
        assert_eq!(playlists[0].title, "aaa");
        assert_eq!(playlists[0].track_count, Some(6));
        assert_eq!(playlists[0].cover_url.as_deref(), Some("large"));
    }

    #[test]
    fn only_user_playlists_can_be_edited() {
        assert!(ensure_editable("PLze97acQIEXm_CL_L8Fp8o7idJU0tlPGq").is_ok());
        assert!(ensure_editable("LM").is_err());
        assert!(ensure_editable("SE").is_err());
    }

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
