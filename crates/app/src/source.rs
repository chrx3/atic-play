//! Where the music comes from.
//!
//! [`MusicSource`] is the seam between the UI and the network. [`YtMusicSource`] talks to
//! [`play_ytmusic`]; [`FakeSource`] is a built-in catalog so the app can be opened, clicked and
//! themed while the client or the network is not ready (`ATIC_PLAY_SOURCE=fake`).
//!
//! Every call is blocking, and every call is made on a background thread: never from `render`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

/// One song the app can play.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub duration: Option<Duration>,
    /// Largest useful cover, before it is downloaded.
    pub thumbnail: Option<String>,
}

impl Track {
    /// Seconds; 0 when the length is unknown.
    pub fn secs(&self) -> f32 {
        self.duration.map_or(0., |d| d.as_secs_f32())
    }

    /// "Artist · Album".
    pub fn subtitle(&self) -> String {
        match &self.album {
            Some(album) if !album.is_empty() => format!("{} · {}", self.artist, album),
            _ => self.artist.clone(),
        }
    }
}

/// An album as a card or a page header.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlbumCard {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub year: Option<u32>,
    pub thumbnail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArtistCard {
    pub id: String,
    pub name: String,
    pub thumbnail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaylistCard {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub thumbnail: Option<String>,
}

/// Anything the shelves can hold.
#[derive(Clone, Debug, PartialEq)]
pub enum Card {
    Song(Track),
    Album(AlbumCard),
    Artist(ArtistCard),
    Playlist(PlaylistCard),
}

impl Card {
    pub fn title(&self) -> &str {
        match self {
            Card::Song(t) => &t.title,
            Card::Album(a) => &a.title,
            Card::Artist(a) => &a.name,
            Card::Playlist(p) => &p.title,
        }
    }

    pub fn subtitle(&self) -> String {
        match self {
            Card::Song(t) => t.artist.clone(),
            Card::Album(a) => a.artist.clone(),
            Card::Artist(_) => "Artista".into(),
            Card::Playlist(p) => p.author.clone().unwrap_or_else(|| "Lista".into()),
        }
    }

    pub fn thumbnail(&self) -> Option<&str> {
        match self {
            Card::Song(t) => t.thumbnail.as_deref(),
            Card::Album(a) => a.thumbnail.as_deref(),
            Card::Artist(a) => a.thumbnail.as_deref(),
            Card::Playlist(p) => p.thumbnail.as_deref(),
        }
    }

    /// Stable key for the placeholder art and element ids.
    pub fn key(&self) -> String {
        match self {
            Card::Song(t) => t.id.clone(),
            Card::Album(a) => a.id.clone(),
            Card::Artist(a) => a.id.clone(),
            Card::Playlist(p) => p.id.clone(),
        }
    }
}

/// A titled row of the home page.
#[derive(Clone, Debug, PartialEq)]
pub struct Shelf {
    pub title: String,
    pub cards: Vec<Card>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchResults {
    pub songs: Vec<Track>,
    pub albums: Vec<AlbumCard>,
    pub artists: Vec<ArtistCard>,
    pub playlists: Vec<PlaylistCard>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlbumDetail {
    pub album: AlbumCard,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtistDetail {
    pub artist: ArtistCard,
    pub top_songs: Vec<Track>,
    pub albums: Vec<AlbumCard>,
}

/// The signed-in account, as the UI shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountInfo {
    pub name: String,
    pub handle: Option<String>,
    pub photo: Option<String>,
}

/// What a call answers when it needed an account and the session was gone. The app watches for
/// it to go back to the anonymous client and offer the login again.
pub const SIGNED_OUT: &str = "signed-out";

/// Whether an error is the one above.
pub fn is_signed_out(message: &str) -> bool {
    message.contains(SIGNED_OUT)
}

/// A playable stream, ready for [`play_audio`].
#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub content_length: Option<u64>,
    /// Extra gain in dB for volume normalization.
    pub gain_db: f32,
    /// e.g. `audio/mp4; codecs="mp4a.40.2"`, for the logs.
    pub mime: Option<String>,
    /// Bits per second, for the logs.
    pub bitrate: Option<u32>,
}

/// Everything the app asks the network for. Calls block; the app calls them in the background.
pub trait MusicSource: Send + Sync + 'static {
    fn home(&self) -> Result<Vec<Shelf>, String>;
    fn search(&self, query: &str) -> Result<SearchResults, String>;
    fn suggestions(&self, query: &str) -> Result<Vec<String>, String>;
    fn album(&self, browse_id: &str) -> Result<AlbumDetail, String>;
    fn artist(&self, browse_id: &str) -> Result<ArtistDetail, String>;
    fn playlist(&self, playlist_id: &str) -> Result<Vec<Track>, String>;
    fn up_next(&self, video_id: &str) -> Result<Vec<Track>, String>;
    fn stream(&self, video_id: &str) -> Result<StreamInfo, String>;
    /// Where this source came from, for the UI.
    fn name(&self) -> &'static str;

    // ------------------------------------------------------------ the account
    //
    // The library and the likes need a session. A source without one answers with
    // [`SIGNED_OUT`], and the app keeps working as an anonymous client.

    /// Whether the source carries a session (it may still be expired).
    fn signed_in(&self) -> bool {
        false
    }

    /// Whether the keyring had a session that the client could not use, so the UI can offer the
    /// login again instead of pretending nothing was there.
    fn session_rejected(&self) -> bool {
        false
    }

    /// Name, handle and photo of the account.
    fn account(&self) -> Result<AccountInfo, String> {
        Err(SIGNED_OUT.into())
    }

    /// The songs marked "me gusta", newest first.
    fn liked_songs(&self) -> Result<Vec<Track>, String> {
        Err(SIGNED_OUT.into())
    }

    fn library_playlists(&self) -> Result<Vec<PlaylistCard>, String> {
        Err(SIGNED_OUT.into())
    }

    fn library_albums(&self) -> Result<Vec<AlbumCard>, String> {
        Err(SIGNED_OUT.into())
    }

    fn library_artists(&self) -> Result<Vec<ArtistCard>, String> {
        Err(SIGNED_OUT.into())
    }

    /// What the account played recently, newest first.
    fn history(&self) -> Result<Vec<Track>, String> {
        Err(SIGNED_OUT.into())
    }

    fn set_liked(&self, _video_id: &str, _liked: bool) -> Result<(), String> {
        Err(SIGNED_OUT.into())
    }

    fn add_to_playlist(&self, _playlist_id: &str, _video_id: &str) -> Result<(), String> {
        Err(SIGNED_OUT.into())
    }

    /// Creates a private playlist with `video_id` in it and answers its id.
    fn create_playlist(&self, _title: &str, _video_id: &str) -> Result<String, String> {
        Err(SIGNED_OUT.into())
    }
}

/// `ATIC_PLAY_SOURCE=fake|yt`; `yt` when it is not set.
///
/// The real source starts signed in when the keyring has a session. The app never sees the
/// cookies: they are read here, once, and only the client keeps them.
///
/// `ATIC_PLAY_ANONYMOUS=1` starts the real client without the session, for looking at the
/// signed-out UI without touching the account.
pub fn from_env() -> Arc<dyn MusicSource> {
    match std::env::var("ATIC_PLAY_SOURCE").as_deref() {
        Ok("fake") => Arc::new(FakeSource::new()),
        _ if std::env::var("ATIC_PLAY_ANONYMOUS").is_ok_and(|value| value == "1") => {
            log::info!("account: starting anonymous on purpose (ATIC_PLAY_ANONYMOUS)");
            Arc::new(YtMusicSource::new("es", "CL"))
        }
        _ => Arc::new(YtMusicSource::with_session("es", "CL")),
    }
}

/// A fresh client with whatever session the keyring has right now. Used after a login and after
/// a logout, so the app can swap its source without restarting.
pub fn signed_source(language: &str, region: &str) -> Arc<dyn MusicSource> {
    Arc::new(YtMusicSource::with_session(language, region))
}

// ---------------------------------------------------------------- YouTube Music

/// The real client, with panics from the not-yet-finished crate turned into errors.
pub struct YtMusicSource {
    client: Option<play_ytmusic::Client>,
    error: String,
    /// The keyring had a session, but it could not be used: the UI offers the login again.
    session_rejected: bool,
}

impl YtMusicSource {
    /// Anonymous: no session, only the public catalog.
    pub fn new(language: &str, region: &str) -> Self {
        Self::build(language, region, None)
    }

    /// Signed in with the session saved in the keyring, or anonymous when there is none.
    ///
    /// Reading the keyring is fast but blocking: this runs once, before the window opens.
    pub fn with_session(language: &str, region: &str) -> Self {
        let session = match play_account::load() {
            Ok(session) => session,
            Err(err) => {
                log::warn!("the account could not be read from the keyring: {err}");
                None
            }
        };
        match session {
            Some(session) if !session.cookies.trim().is_empty() => {
                log::info!("account: a session is saved");
                Self::build(language, region, Some(session.cookies))
            }
            _ => {
                log::info!("account: no session saved; starting anonymous");
                Self::build(language, region, None)
            }
        }
    }

    fn build(language: &str, region: &str, cookies: Option<String>) -> Self {
        let mut session_rejected = false;
        let mut failure = None;
        match cookies {
            Some(cookies) => {
                let started = catch_unwind(AssertUnwindSafe(|| {
                    play_ytmusic::Client::with_cookies(language, region, &cookies)
                }));
                match started {
                    Ok(Ok(client)) => {
                        log::info!("YouTube Music client signed in");
                        return Self {
                            client: Some(client),
                            error: String::new(),
                            session_rejected: false,
                        };
                    }
                    Ok(Err(err)) => {
                        // The cookies were there but the client cannot sign with them: the app
                        // keeps working as an anonymous client and asks for the login again.
                        log::warn!("the saved session cannot be used ({err}); going anonymous");
                        session_rejected = true;
                    }
                    Err(_) => {
                        failure = Some(
                            "el cliente de YouTube Music todavía no está disponible".to_string(),
                        )
                    }
                }
            }
            None => {}
        }
        if failure.is_none() {
            let started = catch_unwind(AssertUnwindSafe(|| {
                play_ytmusic::Client::new(language, region)
            }));
            match started {
                Ok(Ok(client)) => {
                    return Self {
                        client: Some(client),
                        error: String::new(),
                        session_rejected,
                    };
                }
                Ok(Err(err)) => failure = Some(format!("no se pudo iniciar YouTube Music: {err}")),
                Err(_) => {
                    failure =
                        Some("el cliente de YouTube Music todavía no está disponible".to_string())
                }
            }
        }
        let error = failure.unwrap_or_default();
        log::warn!("YouTube Music client could not start: {error}");
        Self {
            client: None,
            error,
            session_rejected,
        }
    }

    /// Turns the client's errors into the strings the UI shows, keeping [`SIGNED_OUT`] apart.
    fn describe(err: play_ytmusic::Error) -> String {
        match err {
            play_ytmusic::Error::SignedOut => SIGNED_OUT.to_string(),
            other => other.to_string(),
        }
    }

    fn call<T>(
        &self,
        f: impl FnOnce(&play_ytmusic::Client) -> play_ytmusic::Result<T>,
    ) -> Result<T, String> {
        let Some(client) = &self.client else {
            return Err(self.error.clone());
        };
        match catch_unwind(AssertUnwindSafe(|| f(client))) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(Self::describe(err)),
            Err(_) => Err("esa función de YouTube Music todavía no está disponible".into()),
        }
    }
}

impl MusicSource for YtMusicSource {
    fn home(&self) -> Result<Vec<Shelf>, String> {
        let shelves = self.call(|c| c.home())?;
        Ok(shelves
            .into_iter()
            .map(|shelf| Shelf {
                title: shelf.title,
                cards: shelf.items.into_iter().map(card_from_item).collect(),
            })
            .filter(|shelf| !shelf.cards.is_empty())
            .collect())
    }

    fn search(&self, query: &str) -> Result<SearchResults, String> {
        let results = self.call(|c| c.search(query))?;
        Ok(SearchResults {
            songs: results.songs.into_iter().map(track_from_song).collect(),
            albums: results.albums.into_iter().map(album_from_album).collect(),
            artists: results.artists.into_iter().map(artist_from_artist).collect(),
            playlists: results
                .playlists
                .into_iter()
                .map(playlist_from_playlist)
                .collect(),
        })
    }

    fn suggestions(&self, query: &str) -> Result<Vec<String>, String> {
        self.call(|c| c.search_suggestions(query))
    }

    fn album(&self, browse_id: &str) -> Result<AlbumDetail, String> {
        let page = self.call(|c| c.album(browse_id))?;
        let album = album_from_album(page.album);
        // The track list often comes without art: the album's cover is the right stand-in.
        let tracks = page
            .tracks
            .into_iter()
            .map(track_from_song)
            .map(|mut track| {
                if track.thumbnail.is_none() {
                    track.thumbnail = album.thumbnail.clone();
                }
                if track.album.is_none() {
                    track.album = Some(album.title.clone());
                    track.album_id = Some(album.id.clone());
                }
                track
            })
            .collect();
        Ok(AlbumDetail { album, tracks })
    }

    fn artist(&self, browse_id: &str) -> Result<ArtistDetail, String> {
        let page = self.call(|c| c.artist(browse_id))?;
        let artist = artist_from_artist(page.artist);
        let top_songs = page
            .top_songs
            .into_iter()
            .map(track_from_song)
            .map(|mut track| {
                if track.thumbnail.is_none() {
                    track.thumbnail = artist.thumbnail.clone();
                }
                track
            })
            .collect();
        Ok(ArtistDetail {
            artist,
            top_songs,
            albums: page.albums.into_iter().map(album_from_album).collect(),
        })
    }

    fn playlist(&self, playlist_id: &str) -> Result<Vec<Track>, String> {
        let songs = self.call(|c| c.playlist(playlist_id))?;
        Ok(songs.into_iter().map(track_from_song).collect())
    }

    fn up_next(&self, video_id: &str) -> Result<Vec<Track>, String> {
        let songs = self.call(|c| c.up_next(video_id))?;
        Ok(songs.into_iter().map(track_from_song).collect())
    }

    fn stream(&self, video_id: &str) -> Result<StreamInfo, String> {
        let stream = self.call(|c| c.stream(video_id))?;
        Ok(StreamInfo {
            url: stream.url,
            headers: stream.headers,
            content_length: stream.content_length,
            gain_db: stream.loudness_db.unwrap_or(0.),
            mime: Some(stream.mime_type),
            bitrate: (stream.bitrate > 0).then_some(stream.bitrate),
        })
    }

    fn name(&self) -> &'static str {
        "YouTube Music"
    }

    fn signed_in(&self) -> bool {
        self.client.as_ref().is_some_and(|client| client.is_signed_in())
    }

    fn session_rejected(&self) -> bool {
        self.session_rejected
    }

    fn account(&self) -> Result<AccountInfo, String> {
        let account = self.call(|c| c.account())?;
        Ok(AccountInfo {
            name: account.name,
            handle: account.handle,
            photo: best_thumbnail(&account.photo),
        })
    }

    fn liked_songs(&self) -> Result<Vec<Track>, String> {
        let songs = self.call(|c| c.liked_songs())?;
        Ok(songs.into_iter().map(track_from_song).collect())
    }

    fn library_playlists(&self) -> Result<Vec<PlaylistCard>, String> {
        let playlists = self.call(|c| c.library_playlists())?;
        Ok(playlists.into_iter().map(playlist_from_playlist).collect())
    }

    fn library_albums(&self) -> Result<Vec<AlbumCard>, String> {
        let albums = self.call(|c| c.library_albums())?;
        Ok(albums.into_iter().map(album_from_album).collect())
    }

    fn library_artists(&self) -> Result<Vec<ArtistCard>, String> {
        let artists = self.call(|c| c.library_artists())?;
        Ok(artists.into_iter().map(artist_from_artist).collect())
    }

    fn history(&self) -> Result<Vec<Track>, String> {
        let songs = self.call(|c| c.history())?;
        Ok(songs.into_iter().map(track_from_song).collect())
    }

    fn set_liked(&self, video_id: &str, liked: bool) -> Result<(), String> {
        self.call(|c| c.set_liked(video_id, liked))
    }

    fn add_to_playlist(&self, playlist_id: &str, video_id: &str) -> Result<(), String> {
        self.call(|c| c.add_to_playlist(playlist_id, video_id))
    }

    fn create_playlist(&self, title: &str, video_id: &str) -> Result<String, String> {
        let ids = [video_id.to_string()];
        self.call(|c| c.create_playlist(title, &ids))
    }
}

fn track_from_song(song: play_ytmusic::Song) -> Track {
    Track {
        id: song.video_id,
        title: song.title,
        artist: song
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        album: song.album.as_ref().map(|a| a.title.clone()),
        album_id: song.album.as_ref().map(|a| a.id.clone()),
        duration: song.duration,
        thumbnail: best_thumbnail(&song.thumbnails),
    }
}

fn album_from_album(album: play_ytmusic::Album) -> AlbumCard {
    AlbumCard {
        id: album.browse_id,
        title: album.title,
        artist: album
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        year: album.year,
        thumbnail: best_thumbnail(&album.thumbnails),
    }
}

fn artist_from_artist(artist: play_ytmusic::Artist) -> ArtistCard {
    ArtistCard {
        id: artist.browse_id,
        name: artist.name,
        thumbnail: best_thumbnail(&artist.thumbnails),
    }
}

fn playlist_from_playlist(playlist: play_ytmusic::Playlist) -> PlaylistCard {
    PlaylistCard {
        id: playlist.id,
        title: playlist.title,
        author: playlist.author,
        thumbnail: best_thumbnail(&playlist.thumbnails),
    }
}

fn card_from_item(item: play_ytmusic::Item) -> Card {
    match item {
        play_ytmusic::Item::Song(song) => Card::Song(track_from_song(song)),
        play_ytmusic::Item::Album(album) => Card::Album(album_from_album(album)),
        play_ytmusic::Item::Artist(artist) => Card::Artist(artist_from_artist(artist)),
        play_ytmusic::Item::Playlist(playlist) => Card::Playlist(playlist_from_playlist(playlist)),
    }
}

/// The biggest thumbnail that is still reasonable to decode, or the last one.
fn best_thumbnail(list: &[play_ytmusic::Thumbnail]) -> Option<String> {
    list.iter()
        .filter(|t| (200..=640).contains(&t.width))
        .next_back()
        .or_else(|| list.last())
        .map(|t| t.url.clone())
}

// ---------------------------------------------------------------- fake catalog

/// A small, made-up catalog: no files, no network, no audio.
pub struct FakeSource {
    albums: Vec<AlbumCard>,
    artists: Vec<ArtistCard>,
    tracks: Vec<Track>,
    playlists: Vec<PlaylistCard>,
}

const FAKE_ALBUMS: [(&str, &str, u32); 8] = [
    ("Midnight Lagoon", "Nova Reyes", 228),
    ("Golden Hour", "The Amber Coast", 32),
    ("Velvet Static", "Lumen", 186),
    ("Moss & Moonlight", "Fern Hollow", 140),
    ("Paper Planes", "Kite Season", 356),
    ("Deep Blue Radio", "Marina Sol", 262),
    ("Neon Garden", "Orchid Club", 330),
    ("Slow Orbit", "Halden", 208),
];

const FAKE_TRACKS: [(&str, usize, u32); 18] = [
    ("Glass Tide", 0, 222),
    ("Night Swim", 0, 245),
    ("Amber Road", 1, 206),
    ("Sunbleached", 1, 178),
    ("Static Bloom", 2, 272),
    ("Moth to Signal", 2, 227),
    ("Fern Song", 3, 213),
    ("Under the Canopy", 3, 302),
    ("Paper Planes", 4, 171),
    ("Kitefall", 4, 200),
    ("Sonar", 5, 235),
    ("Harbor Lights", 5, 260),
    ("Petal Pulse", 6, 192),
    ("After Hours", 6, 224),
    ("Perihelion", 7, 288),
    ("Drift", 7, 216),
    ("Low Lanterns", 0, 198),
    ("Last Light", 1, 251),
];

impl FakeSource {
    pub fn new() -> Self {
        let albums: Vec<AlbumCard> = FAKE_ALBUMS
            .iter()
            .enumerate()
            .map(|(i, (title, artist, _))| AlbumCard {
                id: format!("fake-album-{i}"),
                title: (*title).to_string(),
                artist: (*artist).to_string(),
                year: Some(2024 - (i as u32 % 4)),
                thumbnail: None,
            })
            .collect();
        let tracks: Vec<Track> = FAKE_TRACKS
            .iter()
            .enumerate()
            .map(|(i, (title, album, secs))| Track {
                id: format!("fake-track-{i}"),
                title: (*title).to_string(),
                artist: albums[*album].artist.clone(),
                album: Some(albums[*album].title.clone()),
                album_id: Some(albums[*album].id.clone()),
                duration: Some(Duration::from_secs(*secs as u64)),
                thumbnail: None,
            })
            .collect();
        let mut artists: Vec<ArtistCard> = Vec::new();
        for album in &albums {
            if artists.iter().any(|a| a.name == album.artist) {
                continue;
            }
            artists.push(ArtistCard {
                id: format!("fake-artist-{}", artists.len()),
                name: album.artist.clone(),
                thumbnail: None,
            });
        }
        let playlists = vec![
            PlaylistCard {
                id: "fake-playlist-late".into(),
                title: "Viaje de noche".into(),
                author: Some("Tú".into()),
                thumbnail: None,
            },
            PlaylistCard {
                id: "fake-playlist-sunday".into(),
                title: "Domingo tranquilo".into(),
                author: Some("Tú".into()),
                thumbnail: None,
            },
        ];
        Self {
            albums,
            artists,
            tracks,
            playlists,
        }
    }

    fn album_index(&self, id: &str) -> Option<usize> {
        self.albums.iter().position(|a| a.id == id)
    }
}

impl MusicSource for FakeSource {
    fn home(&self) -> Result<Vec<Shelf>, String> {
        let album_cards = |list: &[usize]| -> Vec<Card> {
            list.iter()
                .map(|&i| Card::Album(self.albums[i].clone()))
                .collect()
        };
        let all: Vec<usize> = (0..self.albums.len()).collect();
        let recent: Vec<usize> = all.iter().rev().copied().collect();
        let songs = |list: &[usize]| -> Vec<Card> {
            list.iter()
                .map(|&i| Card::Song(self.tracks[i].clone()))
                .collect()
        };
        Ok(vec![
            Shelf {
                title: "Hecho para ti".into(),
                cards: album_cards(&all),
            },
            Shelf {
                title: "Canciones que te pueden gustar".into(),
                cards: songs(&[0, 4, 7, 9, 12, 15, 2, 10, 17, 6]),
            },
            Shelf {
                title: "Lanzamientos".into(),
                cards: album_cards(&recent[..5]),
            },
            Shelf {
                title: "Artistas".into(),
                cards: self
                    .artists
                    .iter()
                    .map(|a| Card::Artist(a.clone()))
                    .collect(),
            },
            Shelf {
                title: "Tus listas".into(),
                cards: self
                    .playlists
                    .iter()
                    .map(|p| Card::Playlist(p.clone()))
                    .collect(),
            },
            Shelf {
                title: "Escuchado hace poco".into(),
                cards: songs(&[1, 3, 5, 8, 11, 13, 16]),
            },
        ])
    }

    fn search(&self, query: &str) -> Result<SearchResults, String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(SearchResults::default());
        }
        let has = |text: &str| text.to_lowercase().contains(&query);
        let mut results = SearchResults::default();
        results.songs = self
            .tracks
            .iter()
            .filter(|t| has(&t.title) || has(&t.artist) || t.album.as_deref().is_some_and(has))
            .cloned()
            .collect();
        results.albums = self
            .albums
            .iter()
            .filter(|a| has(&a.title) || has(&a.artist))
            .cloned()
            .collect();
        results.artists = self
            .artists
            .iter()
            .filter(|a| has(&a.name))
            .cloned()
            .collect();
        results.playlists = self
            .playlists
            .iter()
            .filter(|p| has(&p.title))
            .cloned()
            .collect();
        Ok(results)
    }

    fn suggestions(&self, query: &str) -> Result<Vec<String>, String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut out: Vec<String> = Vec::new();
        for track in &self.tracks {
            if out.len() >= 6 {
                break;
            }
            if track.title.to_lowercase().contains(&query) {
                out.push(track.title.clone());
            }
        }
        for album in &self.albums {
            if out.len() >= 6 {
                break;
            }
            if album.title.to_lowercase().contains(&query) {
                out.push(album.title.clone());
            }
        }
        Ok(out)
    }

    fn album(&self, browse_id: &str) -> Result<AlbumDetail, String> {
        let index = self
            .album_index(browse_id)
            .ok_or_else(|| "no encontré ese álbum".to_string())?;
        Ok(AlbumDetail {
            album: self.albums[index].clone(),
            tracks: self
                .tracks
                .iter()
                .filter(|t| t.album_id.as_deref() == Some(browse_id))
                .cloned()
                .collect(),
        })
    }

    fn artist(&self, browse_id: &str) -> Result<ArtistDetail, String> {
        let artist = self
            .artists
            .iter()
            .find(|a| a.id == browse_id)
            .cloned()
            .ok_or_else(|| "no encontré ese artista".to_string())?;
        Ok(ArtistDetail {
            top_songs: self
                .tracks
                .iter()
                .filter(|t| t.artist == artist.name)
                .cloned()
                .collect(),
            albums: self
                .albums
                .iter()
                .filter(|a| a.artist == artist.name)
                .cloned()
                .collect(),
            artist,
        })
    }

    fn playlist(&self, playlist_id: &str) -> Result<Vec<Track>, String> {
        let list: &[usize] = match playlist_id {
            "fake-playlist-late" => &[0, 2, 5, 11, 14, 8],
            "fake-playlist-sunday" => &[6, 3, 16, 12, 1],
            _ => return Err("no encontré esa lista".into()),
        };
        Ok(list.iter().map(|&i| self.tracks[i].clone()).collect())
    }

    fn up_next(&self, video_id: &str) -> Result<Vec<Track>, String> {
        let Some(index) = self.tracks.iter().position(|t| t.id == video_id) else {
            return Ok(Vec::new());
        };
        let current = &self.tracks[index];
        let mut list: Vec<Track> = vec![current.clone()];
        list.extend(
            self.tracks
                .iter()
                .enumerate()
                .filter(|(i, t)| *i != index && t.album_id == current.album_id)
                .map(|(_, t)| t.clone()),
        );
        list.extend(
            self.tracks
                .iter()
                .enumerate()
                .filter(|(i, t)| *i != index && t.album_id != current.album_id)
                .map(|(_, t)| t.clone()),
        );
        list.truncate(12);
        Ok(list)
    }

    fn stream(&self, _video_id: &str) -> Result<StreamInfo, String> {
        Err("el modo de prueba no tiene audio: usa ATIC_PLAY_SOURCE=yt".into())
    }

    fn name(&self) -> &'static str {
        "catálogo de prueba"
    }

    // The fake catalog pretends to be signed in, so the library can be looked at with no network
    // at all. Nothing is ever written: the calls only answer yes.

    fn signed_in(&self) -> bool {
        true
    }

    fn account(&self) -> Result<AccountInfo, String> {
        Ok(AccountInfo {
            name: "Cuenta de prueba".into(),
            handle: Some("@atic-play".into()),
            photo: None,
        })
    }

    fn liked_songs(&self) -> Result<Vec<Track>, String> {
        Ok(self.tracks.iter().rev().take(6).cloned().collect())
    }

    fn library_playlists(&self) -> Result<Vec<PlaylistCard>, String> {
        Ok(self.playlists.clone())
    }

    fn library_albums(&self) -> Result<Vec<AlbumCard>, String> {
        Ok(self.albums.clone())
    }

    fn library_artists(&self) -> Result<Vec<ArtistCard>, String> {
        Ok(self.artists.clone())
    }

    fn history(&self) -> Result<Vec<Track>, String> {
        Ok(self.tracks.iter().take(10).cloned().collect())
    }

    fn set_liked(&self, _video_id: &str, _liked: bool) -> Result<(), String> {
        Ok(())
    }

    fn add_to_playlist(&self, _playlist_id: &str, _video_id: &str) -> Result<(), String> {
        Ok(())
    }

    fn create_playlist(&self, title: &str, _video_id: &str) -> Result<String, String> {
        Ok(format!("fake-created-{}", title.len()))
    }
}

/// "m:ss".
pub fn clock_text(secs: f32) -> String {
    let secs = secs.max(0.) as u32;
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_catalog_is_consistent() {
        let source = FakeSource::new();
        let shelves = source.home().unwrap();
        assert!(!shelves.is_empty());
        for shelf in &shelves {
            assert!(!shelf.cards.is_empty(), "{}", shelf.title);
        }
        let album = source.album("fake-album-0").unwrap();
        assert!(!album.tracks.is_empty());
        assert!(album.tracks.iter().all(|t| t.album_id.as_deref() == Some("fake-album-0")));
    }

    #[test]
    fn fake_search_finds_titles_and_artists() {
        let source = FakeSource::new();
        let results = source.search("amber").unwrap();
        assert!(!results.songs.is_empty());
        assert!(!results.albums.is_empty());
        assert!(source.search("   ").unwrap().is_empty());
    }

    #[test]
    fn up_next_starts_with_the_song_and_stays_on_the_album_first() {
        let source = FakeSource::new();
        let list = source.up_next("fake-track-0").unwrap();
        assert_eq!(list[0].id, "fake-track-0");
        assert!(list.iter().take(3).all(|t| t.album_id.as_deref() == Some("fake-album-0")));
    }

    /// Manual probe of the signed-in library against the real account:
    /// `cargo test -p atic-play -- --ignored the_saved_session_reads_the_library --nocapture`.
    ///
    /// It only reads: it never likes, unlikes or edits a playlist, so it is safe to run against
    /// the user's own account.
    #[test]
    #[ignore = "hits the network and needs the session saved in the keyring"]
    fn the_saved_session_reads_the_library() {
        let session = play_account::load()
            .expect("the keyring could not be read")
            .expect("there is no saved session");
        let client = play_ytmusic::Client::with_cookies("es", "CL", &session.cookies)
            .expect("the cookies were rejected");
        assert!(client.is_signed_in());

        let account = client.account().expect("the account menu reads");
        println!("account: {} ({:?})", account.name, account.handle);
        assert!(!account.name.is_empty());

        let liked = client.liked_songs().expect("liked songs read");
        println!("liked: {}", liked.len());
        assert!(liked.iter().all(|song| !song.video_id.is_empty()));

        let playlists = client.library_playlists().expect("playlists read");
        println!("playlists: {}", playlists.len());
        assert!(playlists.iter().all(|playlist| !playlist.id.is_empty()));

        let albums = client.library_albums().expect("albums read");
        println!("albums: {}", albums.len());
        assert!(albums.iter().all(|album| !album.browse_id.is_empty()));

        let artists = client.library_artists().expect("artists read");
        println!("artists: {}", artists.len());
        assert!(
            artists.iter().all(|artist| artist.browse_id.starts_with("UC")),
            "a library artist is a channel, not something else the page holds"
        );
        assert!(artists.iter().all(|artist| !artist.name.trim().is_empty()));

        let history = client.history().expect("history reads");
        println!("history: {}", history.len());
        assert!(history.iter().all(|song| !song.video_id.is_empty()));
    }

    #[test]
    fn clock_text_pads_seconds() {
        assert_eq!(clock_text(0.), "0:00");
        assert_eq!(clock_text(65.9), "1:05");
        assert_eq!(clock_text(-3.), "0:00");
    }
}
