//! A small YouTube Music client, written from scratch against the public web endpoints.
//!
//! Everything is blocking (`ureq`): call it from a background thread, never from the UI thread.
//! The public types below are the contract the app codes against; keep their names and fields.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

mod auth;
mod http;
mod parse;

/// Errors the app can tell apart.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Network(String),
    #[error("YouTube Music answered something we could not read: {0}")]
    Parse(String),
    #[error("this song cannot be played: {0}")]
    Unplayable(String),
    /// The call needed an account: there are no cookies, or they expired.
    #[error("the YouTube Music session is not signed in, or its cookies expired")]
    SignedOut,
}

pub type Result<T> = std::result::Result<T, Error>;

/// One size of an image; lists are ordered from smallest to largest.
#[derive(Clone, Debug, PartialEq)]
pub struct Thumbnail {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtistRef {
    pub name: String,
    /// Browse id (`UC…`), when the artist has a page.
    pub id: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlbumRef {
    pub title: String,
    /// Browse id (`MPREb_…`).
    pub id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Song {
    pub video_id: String,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub duration: Option<Duration>,
    pub thumbnails: Vec<Thumbnail>,
    pub explicit: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Album {
    pub browse_id: String,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub year: Option<u32>,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Artist {
    pub browse_id: String,
    pub name: String,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Playlist {
    /// Without the `VL` prefix.
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Song(Song),
    Album(Album),
    Artist(Artist),
    Playlist(Playlist),
}

/// The signed-in YouTube Music account.
#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub name: String,
    /// `@handle`, the email, or whatever byline the account menu sends.
    pub handle: Option<String>,
    pub photo: Vec<Thumbnail>,
}

/// A titled row of the home page ("Quick picks", "New releases"…).
#[derive(Clone, Debug, PartialEq)]
pub struct Shelf {
    pub title: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlbumPage {
    pub album: Album,
    pub tracks: Vec<Song>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtistPage {
    pub artist: Artist,
    pub top_songs: Vec<Song>,
    pub albums: Vec<Album>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchResults {
    pub songs: Vec<Song>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
}

/// A playable audio stream: open `url` with these `headers`, in byte ranges.
#[derive(Clone, Debug)]
pub struct Stream {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// e.g. `audio/mp4; codecs="mp4a.40.2"`.
    pub mime_type: String,
    pub bitrate: u32,
    /// Total size in bytes, when known.
    pub content_length: Option<u64>,
    pub duration: Option<Duration>,
    /// Loudness relative to the reference, for volume normalization.
    pub loudness_db: Option<f32>,
    pub expires_at: Instant,
}

/// Shared client: holds the HTTP agent, the session id and the language. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    http: Arc<http::Http>,
}

/// How many items the library lists are allowed to page through.
const LIBRARY_LIMIT: usize = 500;

impl Client {
    /// `language` like `"es"`, `region` like `"CL"`.
    ///
    /// Creates the anonymous session: it makes one request to get the `visitorData` YouTube
    /// Music needs. Empty values fall back to `"es"` / `"CL"`.
    pub fn new(language: &str, region: &str) -> Result<Self> {
        Ok(Self {
            http: Arc::new(http::Http::new(language, region, None)?),
        })
    }

    /// Same as [`Client::new`], signed in with these cookies (a `Cookie` header value).
    ///
    /// Empty cookies fall back to an anonymous client. Cookies without a `SAPISID`-like cookie
    /// cannot sign requests, so they are rejected with [`Error::SignedOut`].
    pub fn with_cookies(language: &str, region: &str, cookies: &str) -> Result<Self> {
        let auth = if cookies.trim().is_empty() {
            None
        } else {
            Some(auth::Auth::parse(cookies).ok_or(Error::SignedOut)?)
        };
        Ok(Self {
            http: Arc::new(http::Http::new(language, region, auth)?),
        })
    }

    /// True when the client was built with cookies. Calls may still return
    /// [`Error::SignedOut`] if they expired.
    pub fn is_signed_in(&self) -> bool {
        self.http.is_signed_in()
    }

    /// Name, handle and photo of the signed-in account.
    pub fn account(&self) -> Result<Account> {
        self.ensure_signed_in()?;
        let answer = self
            .http
            .post("account/account_menu", &http::WEB, Map::new())?;
        // The menu also comes back signed out, just without an account header.
        parse::account(&answer).ok_or(Error::SignedOut)
    }

    /// The songs marked "me gusta", newest first.
    pub fn liked_songs(&self) -> Result<Vec<Song>> {
        self.paged_library("FEmusic_liked_videos", parse::songs_in_page, |song| {
            song.video_id.clone()
        })
    }

    /// The playlists saved in the account's library.
    pub fn library_playlists(&self) -> Result<Vec<Playlist>> {
        self.paged_library(
            "FEmusic_liked_playlists",
            parse::playlists_in_page,
            |playlist| playlist.id.clone(),
        )
    }

    /// The albums saved in the account's library.
    pub fn library_albums(&self) -> Result<Vec<Album>> {
        self.paged_library("FEmusic_liked_albums", parse::albums_in_page, |album| {
            album.browse_id.clone()
        })
    }

    /// The artists of the account's library.
    ///
    /// YouTube Music has answered this page under more than one browse id over time
    /// (`FEmusic_library_corpus_track_artists` is the one `WEB_REMIX` uses today); the first one
    /// that answers with artists wins, so a renamed page does not empty the tab.
    pub fn library_artists(&self) -> Result<Vec<Artist>> {
        const PAGES: [&str; 3] = [
            "FEmusic_library_corpus_artists",
            "FEmusic_library_corpus_track_artists",
            "FEmusic_library_artists",
        ];
        let mut artists = Vec::new();
        for browse_id in PAGES {
            artists = self.paged_library(browse_id, parse::artists_in_page, |artist| {
                artist.browse_id.clone()
            })?;
            if !artists.is_empty() {
                break;
            }
        }
        Ok(artists)
    }

    /// What the account played recently, newest first.
    pub fn history(&self) -> Result<Vec<Song>> {
        self.paged_library("FEmusic_history", parse::songs_in_page, |song| {
            song.video_id.clone()
        })
    }

    /// Marks a song as liked (or removes the like).
    pub fn set_liked(&self, video_id: &str, liked: bool) -> Result<()> {
        self.ensure_signed_in()?;
        if video_id.trim().is_empty() {
            return Err(Error::Parse("set_liked needs a video id".to_string()));
        }
        let endpoint = if liked {
            "like/like"
        } else {
            "like/removelike"
        };
        let mut body = Map::new();
        body.insert("target".to_string(), target(video_id));
        self.http.post(endpoint, &http::WEB, body)?;
        Ok(())
    }

    /// Adds a song at the end of one of the account's playlists.
    pub fn add_to_playlist(&self, playlist_id: &str, video_id: &str) -> Result<()> {
        self.ensure_signed_in()?;
        let playlist_id = playlist_id.strip_prefix("VL").unwrap_or(playlist_id);
        if playlist_id.trim().is_empty() || video_id.trim().is_empty() {
            return Err(Error::Parse(
                "add_to_playlist needs a playlist id and a video id".to_string(),
            ));
        }

        let mut action = Map::new();
        action.insert("action".to_string(), Value::from("ACTION_ADD_VIDEO"));
        action.insert("addedVideoId".to_string(), Value::from(video_id));

        let mut body = Map::new();
        body.insert("playlistId".to_string(), Value::from(playlist_id));
        body.insert(
            "actions".to_string(),
            Value::Array(vec![Value::Object(action)]),
        );

        let answer = self.http.post("browse/edit_playlist", &http::WEB, body)?;
        parse::playlist_edit_status(&answer)
    }

    /// Creates a private playlist and returns its id (without the `VL` prefix).
    pub fn create_playlist(&self, title: &str, video_ids: &[String]) -> Result<String> {
        self.ensure_signed_in()?;
        let title = title.trim();
        if title.is_empty() {
            return Err(Error::Parse("a playlist needs a title".to_string()));
        }

        let mut body = Map::new();
        body.insert("title".to_string(), Value::from(title));
        body.insert("privacyStatus".to_string(), Value::from("PRIVATE"));
        body.insert(
            "videoIds".to_string(),
            Value::Array(
                video_ids
                    .iter()
                    .map(|id| Value::from(id.as_str()))
                    .collect(),
            ),
        );

        let answer = self.http.post("playlist/create", &http::WEB, body)?;
        parse::created_playlist_id(&answer)
    }

    /// Walks a library browse id (and its continuations) up to [`LIBRARY_LIMIT`] items.
    fn paged_library<T>(
        &self,
        browse_id: &str,
        extract: impl Fn(&Value) -> Vec<T>,
        key: impl Fn(&T) -> String,
    ) -> Result<Vec<T>> {
        self.ensure_signed_in()?;

        let mut items: Vec<T> = Vec::new();
        let mut seen_items = HashSet::new();
        let mut seen_tokens = HashSet::new();
        let mut pages = 0;

        let mut body = Map::new();
        body.insert("browseId".to_string(), Value::from(browse_id));
        let mut answer = self.http.post("browse", &http::WEB, body)?;

        loop {
            pages += 1;
            if pages > 60 {
                // Defensive cap: a misbehaving server must not page forever.
                return Ok(items);
            }

            let page = extract(&answer);
            if page.is_empty() && parse::needs_sign_in(&answer) {
                return Err(Error::SignedOut);
            }
            for item in page {
                if items.len() >= LIBRARY_LIMIT {
                    return Ok(items);
                }
                if seen_items.insert(key(&item)) {
                    items.push(item);
                }
            }

            let Some(token) = parse::continuation(&answer) else {
                return Ok(items);
            };
            if !seen_tokens.insert(token.clone()) {
                return Ok(items);
            }

            let mut body = Map::new();
            body.insert("continuation".to_string(), Value::from(token));
            answer = self.http.post("browse", &http::WEB, body)?;
        }
    }

    fn ensure_signed_in(&self) -> Result<()> {
        if self.is_signed_in() {
            Ok(())
        } else {
            Err(Error::SignedOut)
        }
    }

    pub fn search_suggestions(&self, input: &str) -> Result<Vec<String>> {
        if input.trim().is_empty() {
            return Ok(Vec::new());
        }
        let mut body = Map::new();
        body.insert("input".to_string(), Value::from(input));
        let answer = self
            .http
            .post("music/get_search_suggestions", &http::WEB, body)?;
        Ok(parse::suggestions(&answer))
    }

    pub fn search(&self, query: &str) -> Result<SearchResults> {
        let mut body = Map::new();
        body.insert("query".to_string(), Value::from(query));
        let answer = self.http.post("search", &http::WEB, body)?;
        Ok(parse::search_results(&answer))
    }

    pub fn home(&self) -> Result<Vec<Shelf>> {
        let mut body = Map::new();
        body.insert("browseId".to_string(), Value::from("FEmusic_home"));
        let answer = self.http.post("browse", &http::WEB, body)?;
        Ok(parse::home_shelves(&answer))
    }

    pub fn album(&self, browse_id: &str) -> Result<AlbumPage> {
        let mut body = Map::new();
        body.insert("browseId".to_string(), Value::from(browse_id));
        let answer = self.http.post("browse", &http::WEB, body)?;
        parse::album_page(&answer, browse_id)
    }

    pub fn artist(&self, browse_id: &str) -> Result<ArtistPage> {
        let mut body = Map::new();
        body.insert("browseId".to_string(), Value::from(browse_id));
        let answer = self.http.post("browse", &http::WEB, body)?;
        parse::artist_page(&answer, browse_id)
    }

    pub fn playlist(&self, playlist_id: &str) -> Result<Vec<Song>> {
        let browse_id = if playlist_id.starts_with("VL") {
            playlist_id.to_string()
        } else {
            format!("VL{playlist_id}")
        };
        let mut body = Map::new();
        body.insert("browseId".to_string(), Value::from(browse_id));
        let answer = self.http.post("browse", &http::WEB, body)?;
        Ok(parse::playlist_songs(&answer))
    }

    /// The radio / "up next" queue that follows a song (the song itself comes first).
    pub fn up_next(&self, video_id: &str) -> Result<Vec<Song>> {
        // Asking for the song's radio (`RDAMVM<video>`) makes the server list the whole queue;
        // with only a `videoId` it sends just the current song.
        let radio = format!("RDAMVM{video_id}");
        let mut body = Map::new();
        body.insert("videoId".to_string(), Value::from(video_id));
        body.insert("playlistId".to_string(), Value::from(radio));
        let answer = self.http.post("next", &http::WEB, body)?;
        let songs = parse::queue_songs(&answer);
        if !songs.is_empty() {
            return Ok(songs);
        }

        let mut body = Map::new();
        body.insert("videoId".to_string(), Value::from(video_id));
        let answer = self.http.post("next", &http::WEB, body)?;
        Ok(parse::queue_songs(&answer))
    }

    /// The best audio stream the `play-audio` crate can decode (AAC in MP4 for now).
    pub fn stream(&self, video_id: &str) -> Result<Stream> {
        let mut last_reason = None;
        let mut last_network_error = None;

        for client in http::PLAYER_CLIENTS {
            let mut body = Map::new();
            body.insert("videoId".to_string(), Value::from(video_id));
            body.insert("contentCheckOk".to_string(), Value::from(true));
            body.insert("racyCheckOk".to_string(), Value::from(true));

            let answer = match self.http.post("player", &client, body) {
                Ok(answer) => answer,
                Err(err) => {
                    log::debug!("player with {} failed: {err}", client.name);
                    last_network_error = Some(err);
                    continue;
                }
            };

            match parse::stream_from_player(&answer, client.user_agent) {
                Ok(stream) => {
                    log::debug!(
                        "stream for {video_id} via {} ({}, {} kbps)",
                        client.name,
                        stream.mime_type,
                        stream.bitrate / 1000
                    );
                    return Ok(stream);
                }
                Err(reason) => {
                    log::debug!(
                        "player with {} cannot play {video_id}: {reason}",
                        client.name
                    );
                    last_reason = Some(reason);
                }
            }
        }

        match (last_reason, last_network_error) {
            (Some(reason), _) => Err(Error::Unplayable(reason)),
            (None, Some(err)) => Err(err),
            (None, None) => Err(Error::Unplayable(
                "no player client could be used".to_string(),
            )),
        }
    }
}

/// `{"videoId": …}`, the target of like/unlike calls.
fn target(video_id: &str) -> Value {
    let mut target = Map::new();
    target.insert("videoId".to_string(), Value::from(video_id));
    Value::Object(target)
}
