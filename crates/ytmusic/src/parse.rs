//! Reading YouTube Music JSON.
//!
//! The responses are huge and every field is optional, so everything here is tolerant: a
//! missing field skips one item, it never panics. Only [`crate::Error::Parse`] is returned
//! when a whole page is unusable.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::{
    Account, Album, AlbumPage, AlbumRef, Artist, ArtistPage, ArtistRef, Error, Item, Playlist,
    Result, SearchResults, Shelf, Song, Stream, Thumbnail,
};

const PAGE_ALBUM: &str = "MUSIC_PAGE_TYPE_ALBUM";
const PAGE_ARTIST: &str = "MUSIC_PAGE_TYPE_ARTIST";
const PAGE_PLAYLIST: &str = "MUSIC_PAGE_TYPE_PLAYLIST";
const PAGE_USER_CHANNEL: &str = "MUSIC_PAGE_TYPE_USER_CHANNEL";
const MAX_DEPTH: usize = 64;

// ---------------------------------------------------------------------------------------------
// Small JSON helpers
// ---------------------------------------------------------------------------------------------

/// Walks a fixed path of object keys.
pub(crate) fn at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    path.iter().try_fold(value, |node, key| node.get(*key))
}

/// Reads a text object: `{"runs":[{"text":"…"}]}`, `{"simpleText":"…"}` or a bare string.
pub(crate) fn text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(raw) = value.as_str() {
        let raw = raw.trim();
        return (!raw.is_empty()).then(|| raw.to_string());
    }
    if let Some(runs) = value.get("runs").and_then(Value::as_array) {
        let mut joined = String::new();
        for run in runs {
            if let Some(part) = run.get("text").and_then(Value::as_str) {
                joined.push_str(part);
            }
        }
        let joined = joined.trim();
        if !joined.is_empty() {
            return Some(joined.to_string());
        }
    }
    if let Some(raw) = value.get("simpleText").and_then(Value::as_str) {
        let raw = raw.trim();
        if !raw.is_empty() {
            return Some(raw.to_string());
        }
    }
    if let Some(raw) = value.get("text").and_then(Value::as_str) {
        let raw = raw.trim();
        if !raw.is_empty() {
            return Some(raw.to_string());
        }
    }
    None
}

/// A piece of text plus the page it points to, when it is clickable.
#[derive(Clone, Debug)]
pub(crate) struct Run {
    pub(crate) text: String,
    pub(crate) nav: Option<Nav>,
}

/// Where a clickable text goes.
#[derive(Clone, Debug)]
pub(crate) enum Nav {
    Browse {
        id: String,
        page_type: Option<String>,
    },
    Watch {
        video_id: String,
    },
}

fn run_list(value: Option<&Value>) -> Vec<Run> {
    let Some(value) = value else {
        return Vec::new();
    };
    if let Some(runs) = value.get("runs").and_then(Value::as_array) {
        return runs
            .iter()
            .filter_map(|run| {
                let text = run.get("text").and_then(Value::as_str)?;
                Some(Run {
                    text: text.to_string(),
                    nav: nav_of(run),
                })
            })
            .collect();
    }
    match text(Some(value)) {
        Some(text) => vec![Run {
            text,
            nav: nav_of(value),
        }],
        None => Vec::new(),
    }
}

fn join_runs(runs: &[Run]) -> String {
    let mut joined = String::new();
    for run in runs {
        joined.push_str(&run.text);
    }
    joined.trim().to_string()
}

/// Finds the endpoint a renderer, text or run points to.
pub(crate) fn nav_of(value: &Value) -> Option<Nav> {
    for key in ["navigationEndpoint", "onTap"] {
        if let Some(endpoint) = value.get(key)
            && let Some(nav) = endpoint_nav(endpoint)
        {
            return Some(nav);
        }
    }
    endpoint_nav(value)
}

fn endpoint_nav(endpoint: &Value) -> Option<Nav> {
    if let Some(browse) = endpoint.get("browseEndpoint") {
        let id = browse.get("browseId").and_then(Value::as_str)?;
        return Some(Nav::Browse {
            id: id.to_string(),
            page_type: page_type(browse),
        });
    }
    if let Some(watch) = endpoint.get("watchEndpoint") {
        let video_id = watch.get("videoId").and_then(Value::as_str)?;
        return Some(Nav::Watch {
            video_id: video_id.to_string(),
        });
    }
    None
}

fn page_type(browse: &Value) -> Option<String> {
    at(
        browse,
        &[
            "browseEndpointContextSupportedConfigs",
            "browseEndpointContextMusicConfig",
            "pageType",
        ],
    )
    .and_then(Value::as_str)
    .map(str::to_string)
}

fn browse_id(nav: &Option<Nav>) -> Option<String> {
    match nav {
        Some(Nav::Browse { id, .. }) => Some(id.clone()),
        _ => None,
    }
}

fn watch_id(nav: &Option<Nav>) -> Option<String> {
    match nav {
        Some(Nav::Watch { video_id }) => Some(video_id.clone()),
        _ => None,
    }
}

/// Image sizes of a renderer, smallest first (the server already sends them in order).
pub(crate) fn thumbnails(value: &Value) -> Vec<Thumbnail> {
    const PATHS: &[&[&str]] = &[
        &[
            "thumbnailRenderer",
            "musicThumbnailRenderer",
            "thumbnail",
            "thumbnails",
        ],
        &[
            "thumbnail",
            "musicThumbnailRenderer",
            "thumbnail",
            "thumbnails",
        ],
        &["musicThumbnailRenderer", "thumbnail", "thumbnails"],
        &["thumbnail", "thumbnails"],
        &["thumbnails"],
    ];
    for path in PATHS {
        if let Some(items) = at(value, path).and_then(Value::as_array) {
            let mapped: Vec<Thumbnail> = items.iter().filter_map(thumbnail).collect();
            if !mapped.is_empty() {
                return mapped;
            }
        }
    }
    Vec::new()
}

fn thumbnail(value: &Value) -> Option<Thumbnail> {
    let url = value.get("url").and_then(Value::as_str)?.trim();
    if url.is_empty() {
        return None;
    }
    Some(Thumbnail {
        url: url.to_string(),
        width: value.get("width").and_then(Value::as_u64).unwrap_or(0) as u32,
        height: value.get("height").and_then(Value::as_u64).unwrap_or(0) as u32,
    })
}

/// Reads a number that may come as a JSON number or as a string (`"5983881"`).
pub(crate) fn number(value: &Value) -> Option<u64> {
    if let Some(n) = value.as_u64() {
        return Some(n);
    }
    if let Some(n) = value.as_f64() {
        return (n >= 0.0).then_some(n as u64);
    }
    value.as_str()?.trim().parse().ok()
}

/// `"4:36"` or `"1:02:03"`.
pub(crate) fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let mut numbers = Vec::with_capacity(parts.len());
    for part in parts {
        if part.is_empty() || part.len() > 4 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        numbers.push(part.parse::<u64>().ok()?);
    }
    let (hours, minutes, seconds) = match numbers.as_slice() {
        [m, s] => (0, *m, *s),
        [h, m, s] => (*h, *m, *s),
        _ => return None,
    };
    if minutes > 59 || seconds > 59 {
        return None;
    }
    Some(Duration::from_secs(hours * 3600 + minutes * 60 + seconds))
}

fn year(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.len() != 4 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value = text.parse::<u32>().ok()?;
    (1900..=2100).contains(&value).then_some(value)
}

fn year_from_runs(runs: &[Run]) -> Option<u32> {
    runs.iter().find_map(|run| year(&run.text))
}

fn is_explicit(value: &Value) -> bool {
    ["badges", "subtitleBadges"].iter().any(|key| {
        value
            .get(*key)
            .is_some_and(|badges| has_icon(badges, "MUSIC_EXPLICIT_BADGE"))
    })
}

fn has_icon(value: &Value, wanted: &str) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, child)| {
            (key == "iconType" && child.as_str() == Some(wanted)) || has_icon(child, wanted)
        }),
        Value::Array(items) => items.iter().any(|child| has_icon(child, wanted)),
        _ => false,
    }
}

/// Collects every object stored under `key`, in document order.
pub(crate) fn collect<'a>(root: &'a Value, key: &str) -> Vec<&'a Value> {
    fn walk<'a>(value: &'a Value, key: &str, out: &mut Vec<&'a Value>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match value {
            Value::Object(map) => {
                if let Some(child) = map.get(key)
                    && child.is_object()
                {
                    out.push(child);
                }
                for child in map.values() {
                    walk(child, key, out, depth + 1);
                }
            }
            Value::Array(items) => {
                for child in items {
                    walk(child, key, out, depth + 1);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    walk(root, key, &mut out, 0);
    out
}

/// The page asks the visitor to sign in instead of showing content.
pub(crate) fn needs_sign_in(root: &Value) -> bool {
    fn walk(value: &Value, depth: usize) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        match value {
            Value::Object(map) => {
                map.contains_key("signInEndpoint")
                    || map.values().any(|child| walk(child, depth + 1))
            }
            Value::Array(items) => items.iter().any(|child| walk(child, depth + 1)),
            _ => false,
        }
    }

    walk(root, 0)
}

/// Token of the next page, in either shape the API uses.
pub(crate) fn continuation(root: &Value) -> Option<String> {
    for command in collect(root, "continuationCommand") {
        if let Some(token) = command
            .get("token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
        {
            return Some(token.to_string());
        }
    }
    for next in collect(root, "nextContinuationData") {
        if let Some(token) = next
            .get("continuation")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
        {
            return Some(token.to_string());
        }
    }
    None
}

/// Every section of every `sectionListRenderer` in the page, in document order.
fn sections(root: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    for list in collect(root, "sectionListRenderer") {
        if let Some(items) = list.get("contents").and_then(Value::as_array) {
            out.extend(items.iter());
        }
    }
    out
}

fn find_video_id(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(id) = map.get("videoId").and_then(Value::as_str)
                && !id.is_empty()
            {
                return Some(id.to_string());
            }
            map.values().find_map(find_video_id)
        }
        Value::Array(items) => items.iter().find_map(find_video_id),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Artists and metadata columns
// ---------------------------------------------------------------------------------------------

const TYPE_WORDS: &[&str] = &[
    "canción",
    "cancion",
    "song",
    "vídeo",
    "video",
    "álbum",
    "album",
    "artista",
    "artist",
    "lista de reproducción",
    "playlist",
    "single",
    "ep",
    "sencillo",
    "mixtape",
];

fn is_type_word(text: &str) -> bool {
    let needle = text.trim().to_lowercase();
    TYPE_WORDS.contains(&needle.as_str())
}

/// Drops a leading `Canción •` / `Álbum •` marker.
fn strip_type_prefix(text: &str) -> &str {
    if let Some((first, rest)) = text.split_once(" • ")
        && is_type_word(first)
    {
        return rest.trim();
    }
    text
}

fn split_artists(names: &str) -> Vec<ArtistRef> {
    let cleaned = strip_type_prefix(names);
    let cleaned = cleaned.split(" • ").next().unwrap_or(cleaned);
    cleaned
        .split(',')
        .flat_map(|part| part.split(" y "))
        .flat_map(|part| part.split(" and "))
        .map(str::trim)
        .filter(|part| {
            !part.is_empty()
                && !is_type_word(part)
                && parse_duration(part).is_none()
                && year(part).is_none()
        })
        .map(|name| ArtistRef {
            name: name.to_string(),
            id: None,
        })
        .collect()
}

fn artists_from_runs(runs: &[Run]) -> Vec<ArtistRef> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for run in runs {
        if let Some(Nav::Browse { id, page_type }) = &run.nav
            && page_type.as_deref() == Some(PAGE_ARTIST)
            && seen.insert(run.text.clone())
        {
            out.push(ArtistRef {
                name: run.text.trim().to_string(),
                id: Some(id.clone()),
            });
        }
    }
    out
}

fn artists_from_text(runs: &[Run]) -> Vec<ArtistRef> {
    if runs.is_empty() {
        return Vec::new();
    }
    split_artists(&join_runs(runs))
}

fn album_from_runs(runs: &[Run]) -> Option<AlbumRef> {
    runs.iter().find_map(|run| match &run.nav {
        Some(Nav::Browse { id, page_type }) if page_type.as_deref() == Some(PAGE_ALBUM) => {
            Some(AlbumRef {
                title: run.text.trim().to_string(),
                id: id.clone(),
            })
        }
        _ => None,
    })
}

#[derive(Default)]
struct Meta {
    artists: Vec<ArtistRef>,
    album: Option<AlbumRef>,
    duration: Option<Duration>,
}

fn meta_from_columns(columns: &[Vec<Run>], fixed: &[Vec<Run>]) -> Meta {
    let mut meta = Meta::default();
    let mut seen = HashSet::new();

    for run in columns.iter().flatten() {
        if let Some(duration) = parse_duration(&run.text) {
            if meta.duration.is_none() {
                meta.duration = Some(duration);
            }
            continue;
        }
        if let Some(Nav::Browse { id, page_type }) = &run.nav {
            match page_type.as_deref() {
                Some(PAGE_ARTIST) if seen.insert(run.text.clone()) => {
                    meta.artists.push(ArtistRef {
                        name: run.text.trim().to_string(),
                        id: Some(id.clone()),
                    });
                }
                Some(PAGE_ALBUM) if meta.album.is_none() => {
                    meta.album = Some(AlbumRef {
                        title: run.text.trim().to_string(),
                        id: id.clone(),
                    });
                }
                _ => {}
            }
        }
    }

    if meta.artists.is_empty()
        && let Some(first) = columns.first()
    {
        meta.artists = artists_from_text(first);
    }
    if meta.duration.is_none() {
        meta.duration = fixed
            .iter()
            .flatten()
            .find_map(|run| parse_duration(&run.text));
    }
    meta
}

fn flex_columns(item: &Value) -> Vec<Vec<Run>> {
    let Some(columns) = item.get("flexColumns").and_then(Value::as_array) else {
        return Vec::new();
    };
    columns
        .iter()
        .map(|column| {
            let text = at(
                column,
                &["musicResponsiveListItemFlexColumnRenderer", "text"],
            );
            run_list(text)
        })
        .collect()
}

fn fixed_columns(item: &Value) -> Vec<Vec<Run>> {
    let Some(columns) = item.get("fixedColumns").and_then(Value::as_array) else {
        return Vec::new();
    };
    columns
        .iter()
        .map(|column| {
            let text = at(
                column,
                &["musicResponsiveListItemFixedColumnRenderer", "text"],
            );
            run_list(text)
        })
        .collect()
}

fn list_title_node(item: &Value) -> Option<&Value> {
    item.get("flexColumns")?
        .get(0)?
        .get("musicResponsiveListItemFlexColumnRenderer")?
        .get("text")
}

// ---------------------------------------------------------------------------------------------
// Songs
// ---------------------------------------------------------------------------------------------

/// Reads one `musicResponsiveListItemRenderer` as a song, if it is one.
pub(crate) fn song_from_list_item(item: &Value) -> Option<Song> {
    let columns = flex_columns(item);
    let title_runs = columns.first()?;
    let title = join_runs(title_runs);
    if title.is_empty() {
        return None;
    }

    let video_id = title_runs
        .iter()
        .find_map(|run| watch_id(&run.nav))
        .or_else(|| {
            at(
                item,
                &[
                    "overlay",
                    "musicItemThumbnailOverlayRenderer",
                    "content",
                    "musicPlayButtonRenderer",
                    "playNavigationEndpoint",
                ],
            )
            .and_then(|endpoint| watch_id(&nav_of(endpoint)))
        })
        .or_else(|| watch_id(&nav_of(item)))
        .or_else(|| {
            at(item, &["playlistItemData", "videoId"])
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| find_video_id(item))?;

    let meta = meta_from_columns(columns.get(1..).unwrap_or(&[]), &fixed_columns(item));
    let mut duration = meta.duration;
    if duration.is_none() {
        duration = text(item.get("lengthText")).and_then(|text| parse_duration(&text));
    }

    Some(Song {
        video_id,
        title,
        artists: meta.artists,
        album: meta.album,
        duration,
        thumbnails: thumbnails(item),
        explicit: is_explicit(item),
    })
}

fn song_from_two_row(item: &Value) -> Option<Song> {
    let title = text(item.get("title"))?;
    let subtitle = run_list(item.get("subtitle"));
    let nav = nav_of(item);
    let video_id = watch_id(&nav)?;
    let mut artists = artists_from_runs(&subtitle);
    if artists.is_empty() {
        artists = artists_from_text(&subtitle);
    }
    Some(Song {
        video_id,
        title,
        artists,
        album: album_from_runs(&subtitle),
        duration: subtitle.iter().find_map(|run| parse_duration(&run.text)),
        thumbnails: thumbnails(item),
        explicit: is_explicit(item),
    })
}

/// Reads one `playlistPanelVideoRenderer` (the queue) as a song.
fn song_from_queue_item(item: &Value) -> Option<Song> {
    let title = text(item.get("title"))?;
    let video_id = item
        .get("videoId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| watch_id(&nav_of(item)))
        .or_else(|| find_video_id(item))?;

    let mut byline = run_list(item.get("longBylineText"));
    if byline.is_empty() {
        byline = run_list(item.get("shortBylineText"));
    }
    let mut artists = artists_from_runs(&byline);
    if artists.is_empty() {
        artists = artists_from_text(&byline);
    }

    let duration = text(item.get("lengthText")).and_then(|text| parse_duration(&text));

    Some(Song {
        video_id,
        title,
        artists,
        album: album_from_runs(&byline),
        duration,
        thumbnails: thumbnails(item),
        explicit: is_explicit(item),
    })
}

// ---------------------------------------------------------------------------------------------
// Albums, artists and playlists as list items
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Song,
    Album,
    Artist,
    Playlist,
    Unknown,
}

fn kind_from(value: &Value, subtitle: &[Run]) -> Kind {
    match nav_of(value) {
        Some(Nav::Watch { .. }) => return Kind::Song,
        Some(Nav::Browse { id, page_type }) => {
            match page_type.as_deref() {
                Some(PAGE_ALBUM) => return Kind::Album,
                Some(PAGE_ARTIST) => return Kind::Artist,
                Some(PAGE_PLAYLIST) => return Kind::Playlist,
                Some("MUSIC_PAGE_TYPE_VIDEO") | Some("MUSIC_PAGE_TYPE_TRACK") => {
                    return Kind::Song;
                }
                _ => {}
            }
            if id.starts_with("MPREb_") || id.starts_with("MPAD") {
                return Kind::Album;
            }
            if id.starts_with("UC") {
                return Kind::Artist;
            }
            if id.starts_with("VL") {
                return Kind::Playlist;
            }
        }
        _ => {}
    }

    if let Some(word) = subtitle.iter().find_map(|run| type_word_kind(&run.text)) {
        return word;
    }

    if find_video_id(value).is_some() {
        return Kind::Song;
    }
    Kind::Unknown
}

fn type_word_kind(text: &str) -> Option<Kind> {
    let word = text.trim().to_lowercase();
    match word.as_str() {
        "canción" | "cancion" | "song" | "vídeo" | "video" => Some(Kind::Song),
        "álbum" | "album" | "single" | "ep" | "sencillo" | "mixtape" => Some(Kind::Album),
        "artista" | "artist" => Some(Kind::Artist),
        "lista de reproducción" | "playlist" => Some(Kind::Playlist),
        _ => None,
    }
}

/// Playlist ids arrive as `VL<id>` on browse endpoints.
fn strip_playlist_prefix(id: &str) -> String {
    id.strip_prefix("VL").unwrap_or(id).to_string()
}

/// Browse id of a list or two-row item: its own endpoint, or the one on its title run.
fn item_browse_id(item: &Value) -> Option<String> {
    if let Some(id) = browse_id(&nav_of(item)) {
        return Some(id);
    }
    let title = item.get("title").or_else(|| list_title_node(item));
    run_list(title)
        .into_iter()
        .find_map(|run| browse_id(&run.nav))
}

fn album_from_list_item(item: &Value) -> Option<Album> {
    let title = text(list_title_node(item))?;
    let id = item_browse_id(item)?;
    let columns = flex_columns(item);
    let subtitle = columns.get(1).map(Vec::as_slice).unwrap_or(&[]);
    let mut artists = artists_from_runs(subtitle);
    if artists.is_empty() {
        artists = artists_from_text(subtitle);
    }
    Some(Album {
        browse_id: id,
        title,
        artists,
        year: year_from_runs(subtitle),
        thumbnails: thumbnails(item),
    })
}

fn artist_from_list_item(item: &Value) -> Option<Artist> {
    let name = text(list_title_node(item))?;
    let browse_id = item_browse_id(item)?;
    Some(Artist {
        browse_id,
        name,
        thumbnails: thumbnails(item),
    })
}

/// Owner of a playlist: the run linked to a channel page, or the segment after the type word.
fn playlist_author(subtitle: &[Run]) -> Option<String> {
    subtitle
        .iter()
        .find_map(|run| match &run.nav {
            Some(Nav::Browse { page_type, .. })
                if page_type.as_deref() == Some(PAGE_USER_CHANNEL) =>
            {
                Some(run.text.trim().to_string())
            }
            _ => None,
        })
        .or_else(|| {
            let joined = join_runs(subtitle);
            let mut parts = joined.split(" • ").map(str::trim);
            if parts.next().is_some_and(is_type_word) {
                parts
                    .next()
                    .filter(|part| !part.is_empty())
                    .map(str::to_string)
            } else {
                None
            }
        })
        .filter(|author| !author.is_empty())
}

fn playlist_from_list_item(item: &Value) -> Option<Playlist> {
    let title = text(list_title_node(item))?;
    let id = strip_playlist_prefix(&item_browse_id(item)?);
    let columns = flex_columns(item);
    let subtitle = columns.get(1).map(Vec::as_slice).unwrap_or(&[]);

    Some(Playlist {
        id,
        title,
        author: playlist_author(subtitle),
        thumbnails: thumbnails(item),
    })
}

fn item_from_list_item(item: &Value) -> Option<Item> {
    let columns = flex_columns(item);
    let subtitle = columns.get(1).map(Vec::as_slice).unwrap_or(&[]);
    match kind_from(item, subtitle) {
        Kind::Song => song_from_list_item(item).map(Item::Song),
        Kind::Album => album_from_list_item(item).map(Item::Album),
        Kind::Artist => artist_from_list_item(item).map(Item::Artist),
        Kind::Playlist => playlist_from_list_item(item).map(Item::Playlist),
        Kind::Unknown => None,
    }
}

fn item_from_two_row(item: &Value) -> Option<Item> {
    let title = text(item.get("title"))?;
    let subtitle = run_list(item.get("subtitle"));
    let nav = nav_of(item);

    match kind_from(item, &subtitle) {
        Kind::Album => {
            let browse_id = item_browse_id(item)?;
            let mut artists = artists_from_runs(&subtitle);
            if artists.is_empty() {
                artists = artists_from_text(&subtitle);
            }
            Some(Item::Album(Album {
                browse_id,
                title,
                artists,
                year: year_from_runs(&subtitle),
                thumbnails: thumbnails(item),
            }))
        }
        Kind::Artist => {
            let browse_id = item_browse_id(item)?;
            Some(Item::Artist(Artist {
                browse_id,
                name: title,
                thumbnails: thumbnails(item),
            }))
        }
        Kind::Playlist => {
            let id = strip_playlist_prefix(&item_browse_id(item)?);
            Some(Item::Playlist(Playlist {
                id,
                title,
                author: playlist_author(&subtitle),
                thumbnails: thumbnails(item),
            }))
        }
        Kind::Song => {
            let video_id = watch_id(&nav)?;
            let mut artists = artists_from_runs(&subtitle);
            if artists.is_empty() {
                artists = artists_from_text(&subtitle);
            }
            Some(Item::Song(Song {
                video_id,
                title,
                artists,
                album: album_from_runs(&subtitle),
                duration: subtitle.iter().find_map(|run| parse_duration(&run.text)),
                thumbnails: thumbnails(item),
                explicit: is_explicit(item),
            }))
        }
        Kind::Unknown => None,
    }
}

fn item_from_contents(entry: &Value) -> Option<Item> {
    if let Some(two_row) = entry.get("musicTwoRowItemRenderer") {
        return item_from_two_row(two_row);
    }
    if let Some(list_item) = entry.get("musicResponsiveListItemRenderer") {
        return item_from_list_item(list_item);
    }
    None
}

/// Entries of every shelf-like renderer of a page, in document order.
fn shelf_entries(root: &Value) -> Vec<&Value> {
    fn walk<'a>(value: &'a Value, out: &mut Vec<&'a Value>, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        match value {
            Value::Object(map) => {
                for (key, field) in [
                    ("musicShelfRenderer", "contents"),
                    ("musicPlaylistShelfRenderer", "contents"),
                    ("musicCarouselShelfRenderer", "contents"),
                    ("gridRenderer", "items"),
                    ("itemSectionRenderer", "contents"),
                ] {
                    if let Some(entries) = map
                        .get(key)
                        .and_then(|renderer| renderer.get(field))
                        .and_then(Value::as_array)
                    {
                        out.extend(entries.iter());
                    }
                }
                for child in map.values() {
                    walk(child, out, depth + 1);
                }
            }
            Value::Array(items) => {
                for child in items {
                    walk(child, out, depth + 1);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    walk(root, &mut out, 0);
    out
}

fn item_key(item: &Item) -> String {
    match item {
        Item::Song(song) => format!("song:{}", song.video_id),
        Item::Album(album) => format!("album:{}", album.browse_id),
        Item::Artist(artist) => format!("artist:{}", artist.browse_id),
        Item::Playlist(playlist) => format!("playlist:{}", playlist.id),
    }
}

/// Every song, album, artist and playlist a browse page lists, without duplicates.
pub(crate) fn page_items(root: &Value) -> Vec<Item> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();

    for entry in shelf_entries(root) {
        push_item(&mut items, &mut seen, item_from_contents(entry));
    }

    // Some pages wrap the items in renderers we do not know about: take the inner ones.
    if items.is_empty() {
        for item in collect(root, "musicTwoRowItemRenderer") {
            push_item(&mut items, &mut seen, item_from_two_row(item));
        }
        for item in collect(root, "musicResponsiveListItemRenderer") {
            push_item(&mut items, &mut seen, item_from_list_item(item));
        }
    }

    items
}

fn push_item(items: &mut Vec<Item>, seen: &mut HashSet<String>, item: Option<Item>) {
    if let Some(item) = item
        && seen.insert(item_key(&item))
    {
        items.push(item);
    }
}

pub(crate) fn playlists_in_page(root: &Value) -> Vec<Playlist> {
    page_items(root)
        .into_iter()
        .filter_map(|item| match item {
            Item::Playlist(playlist) => Some(playlist),
            _ => None,
        })
        .collect()
}

pub(crate) fn albums_in_page(root: &Value) -> Vec<Album> {
    page_items(root)
        .into_iter()
        .filter_map(|item| match item {
            Item::Album(album) => Some(album),
            _ => None,
        })
        .collect()
}

pub(crate) fn artists_in_page(root: &Value) -> Vec<Artist> {
    page_items(root)
        .into_iter()
        .filter_map(|item| match item {
            Item::Artist(artist) => Some(artist),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct SearchAcc {
    results: SearchResults,
    songs: HashSet<String>,
    albums: HashSet<String>,
    artists: HashSet<String>,
    playlists: HashSet<String>,
}

impl SearchAcc {
    fn push(&mut self, item: Item) {
        match item {
            Item::Song(song) => {
                if self.songs.insert(song.video_id.clone()) {
                    self.results.songs.push(song);
                }
            }
            Item::Album(album) => {
                if self.albums.insert(album.browse_id.clone()) {
                    self.results.albums.push(album);
                }
            }
            Item::Artist(artist) => {
                if self.artists.insert(artist.browse_id.clone()) {
                    self.results.artists.push(artist);
                }
            }
            Item::Playlist(playlist) => {
                if self.playlists.insert(playlist.id.clone()) {
                    self.results.playlists.push(playlist);
                }
            }
        }
    }

    fn push_contents(&mut self, entries: &[Value]) {
        for entry in entries {
            if let Some(item) = item_from_contents(entry) {
                self.push(item);
            }
        }
    }

    /// The top result is a `musicCardShelfRenderer`: the entity it points to plus songs.
    fn push_card(&mut self, card: &Value) {
        let subtitle = run_list(card.get("subtitle"));
        if let (Some(title), Some(nav)) = (text(card.get("title")), nav_of(card)) {
            let browse = match &nav {
                Nav::Browse { id, .. } => Some(id.clone()),
                _ => None,
            };
            let item = match kind_from(card, &subtitle) {
                Kind::Album => browse.map(|browse_id| {
                    Item::Album(Album {
                        browse_id,
                        title: title.clone(),
                        artists: artists_from_runs(&subtitle),
                        year: year_from_runs(&subtitle),
                        thumbnails: thumbnails(card),
                    })
                }),
                Kind::Artist => browse.map(|browse_id| {
                    Item::Artist(Artist {
                        browse_id,
                        name: title.clone(),
                        thumbnails: thumbnails(card),
                    })
                }),
                Kind::Playlist => browse.map(|id| {
                    Item::Playlist(Playlist {
                        id: strip_playlist_prefix(&id),
                        title: title.clone(),
                        author: playlist_author(&subtitle),
                        thumbnails: thumbnails(card),
                    })
                }),
                Kind::Song => watch_id(&Some(nav)).map(|video_id| {
                    Item::Song(Song {
                        video_id,
                        title: title.clone(),
                        artists: artists_from_runs(&subtitle),
                        album: album_from_runs(&subtitle),
                        duration: subtitle.iter().find_map(|run| parse_duration(&run.text)),
                        thumbnails: thumbnails(card),
                        explicit: is_explicit(card),
                    })
                }),
                Kind::Unknown => None,
            };
            if let Some(item) = item {
                self.push(item);
            }
        }

        if let Some(contents) = card.get("contents").and_then(Value::as_array) {
            self.push_contents(contents);
        }
    }
}

pub(crate) fn search_results(root: &Value) -> SearchResults {
    let mut acc = SearchAcc::default();
    for section in sections(root) {
        if let Some(card) = section.get("musicCardShelfRenderer") {
            acc.push_card(card);
        }
        if let Some(shelf) = section.get("musicShelfRenderer")
            && let Some(entries) = shelf.get("contents").and_then(Value::as_array)
        {
            acc.push_contents(entries);
        }
        if let Some(carousel) = section.get("musicCarouselShelfRenderer")
            && let Some(entries) = carousel.get("contents").and_then(Value::as_array)
        {
            acc.push_contents(entries);
        }
        if let Some(item_section) = section.get("itemSectionRenderer")
            && let Some(entries) = item_section.get("contents").and_then(Value::as_array)
        {
            acc.push_contents(entries);
        }
    }
    acc.results
}

pub(crate) fn suggestions(root: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for renderer in collect(root, "searchSuggestionRenderer") {
        if let Some(suggestion) = text(renderer.get("suggestion"))
            && seen.insert(suggestion.clone())
        {
            out.push(suggestion);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Home
// ---------------------------------------------------------------------------------------------

fn shelf_title(header: Option<&Value>) -> Option<String> {
    let header = header?;
    for key in [
        "musicCarouselShelfBasicHeaderRenderer",
        "musicCardShelfHeaderBasicRenderer",
        "musicShelfRenderer",
    ] {
        if let Some(renderer) = header.get(key)
            && let Some(title) = text(renderer.get("title"))
        {
            return Some(title);
        }
    }
    text(header.get("title"))
}

pub(crate) fn home_shelves(root: &Value) -> Vec<Shelf> {
    let mut out: Vec<Shelf> = Vec::new();
    for section in sections(root) {
        let (title, entries) = if let Some(carousel) = section.get("musicCarouselShelfRenderer") {
            (
                shelf_title(carousel.get("header")),
                carousel.get("contents").and_then(Value::as_array),
            )
        } else if let Some(shelf) = section.get("musicShelfRenderer") {
            (
                text(shelf.get("title")),
                shelf.get("contents").and_then(Value::as_array),
            )
        } else {
            continue;
        };

        let Some(title) = title else { continue };
        let Some(entries) = entries else { continue };
        let items: Vec<Item> = entries.iter().filter_map(item_from_contents).collect();
        if items.is_empty() || out.iter().any(|shelf| shelf.title == title) {
            continue;
        }
        out.push(Shelf { title, items });
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Album, artist and playlist pages
// ---------------------------------------------------------------------------------------------

/// Songs listed in the page's track shelves, in order. Falls back to every list item.
pub(crate) fn songs_in_page(root: &Value) -> Vec<Song> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();

    for key in ["musicShelfRenderer", "musicPlaylistShelfRenderer"] {
        for shelf in collect(root, key) {
            let Some(entries) = shelf.get("contents").and_then(Value::as_array) else {
                continue;
            };
            for entry in entries {
                if let Some(song) = entry
                    .get("musicResponsiveListItemRenderer")
                    .and_then(song_from_list_item)
                    && seen.insert(song.video_id.clone())
                {
                    out.push(song);
                }
            }
        }
    }

    if out.is_empty() {
        for item in collect(root, "musicResponsiveListItemRenderer") {
            if let Some(song) = song_from_list_item(item)
                && seen.insert(song.video_id.clone())
            {
                out.push(song);
            }
        }
    }
    if out.is_empty() {
        for item in collect(root, "musicTwoRowItemRenderer") {
            if let Some(song) = song_from_two_row(item)
                && seen.insert(song.video_id.clone())
            {
                out.push(song);
            }
        }
    }
    out
}

fn first_header<'a>(root: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| collect(root, key).into_iter().next())
}

pub(crate) fn album_page(root: &Value, browse_id: &str) -> Result<AlbumPage> {
    let header = first_header(
        root,
        &[
            "musicResponsiveHeaderRenderer",
            "musicDetailHeaderRenderer",
            "musicImmersiveHeaderRenderer",
        ],
    )
    .ok_or_else(|| Error::Parse("album page without a header".to_string()))?;

    let title = text(header.get("title"))
        .ok_or_else(|| Error::Parse("album header without a title".to_string()))?;
    let subtitle = run_list(header.get("subtitle"));
    let strapline = run_list(header.get("straplineTextOne"));

    let mut artists = artists_from_runs(&strapline);
    if artists.is_empty() {
        artists = artists_from_runs(&subtitle);
    }
    if artists.is_empty() {
        artists = artists_from_text(&strapline);
    }

    Ok(AlbumPage {
        album: Album {
            browse_id: browse_id.to_string(),
            title,
            artists,
            year: year_from_runs(&subtitle),
            thumbnails: thumbnails(header),
        },
        tracks: songs_in_page(root),
    })
}

pub(crate) fn artist_page(root: &Value, browse_id: &str) -> Result<ArtistPage> {
    let header = first_header(
        root,
        &[
            "musicImmersiveHeaderRenderer",
            "musicResponsiveHeaderRenderer",
            "musicVisualHeaderRenderer",
            "musicPageHeaderRenderer",
        ],
    )
    .ok_or_else(|| Error::Parse("artist page without a header".to_string()))?;

    let name = text(header.get("title"))
        .ok_or_else(|| Error::Parse("artist header without a name".to_string()))?;

    let mut albums = Vec::new();
    let mut seen = HashSet::new();
    for item in collect(root, "musicTwoRowItemRenderer") {
        if let Some(Item::Album(album)) = item_from_two_row(item)
            && seen.insert(album.browse_id.clone())
        {
            albums.push(album);
        }
    }
    for item in collect(root, "musicResponsiveListItemRenderer") {
        if let Some(Item::Album(album)) = item_from_list_item(item)
            && seen.insert(album.browse_id.clone())
        {
            albums.push(album);
        }
    }

    Ok(ArtistPage {
        artist: Artist {
            browse_id: browse_id.to_string(),
            name,
            thumbnails: thumbnails(header),
        },
        top_songs: songs_in_page(root),
        albums,
    })
}

/// The tracks of a playlist page. The playlist header is not part of the public API.
pub(crate) fn playlist_songs(root: &Value) -> Vec<Song> {
    songs_in_page(root)
}

// ---------------------------------------------------------------------------------------------
// Account and playlist edits
// ---------------------------------------------------------------------------------------------

/// Reads the signed-in account out of an `account/account_menu` answer.
pub(crate) fn account(root: &Value) -> Option<Account> {
    let header = collect(root, "activeAccountHeaderRenderer")
        .into_iter()
        .next()?;
    let name = text(header.get("accountName"))?;
    let handle = ["channelHandle", "accountByline", "email"]
        .iter()
        .find_map(|key| text(header.get(*key)));

    let mut photo = match header.get("accountPhoto") {
        Some(node) => thumbnails(node),
        None => Vec::new(),
    };
    if photo.is_empty() {
        photo = thumbnails(header);
    }

    Some(Account {
        name,
        handle,
        photo,
    })
}

/// The id of a playlist just created, without the `VL` prefix.
pub(crate) fn created_playlist_id(root: &Value) -> Result<String> {
    let id = root
        .get("playlistId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| Error::Parse("playlist/create did not return a playlistId".to_string()))?;
    Ok(strip_playlist_prefix(id))
}

/// Fails when `browse/edit_playlist` reports anything but success.
pub(crate) fn playlist_edit_status(root: &Value) -> Result<()> {
    if let Some(status) = root.get("status").and_then(Value::as_str)
        && !status.is_empty()
        && status != "STATUS_SUCCEEDED"
    {
        return Err(Error::Network(format!(
            "browse/edit_playlist answered {status}"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Queue ("up next")
// ---------------------------------------------------------------------------------------------

pub(crate) fn queue_songs(root: &Value) -> Vec<Song> {
    let mut songs = Vec::new();
    let mut seen = HashSet::new();
    for item in collect(root, "playlistPanelVideoRenderer") {
        if let Some(song) = song_from_queue_item(item)
            && seen.insert(song.video_id.clone())
        {
            songs.push(song);
        }
    }
    songs
}

// ---------------------------------------------------------------------------------------------
// Streams
// ---------------------------------------------------------------------------------------------

fn is_audio_format(format: &Value) -> bool {
    let is_audio = format
        .get("mimeType")
        .and_then(Value::as_str)
        .is_some_and(|mime| mime.starts_with("audio/"));
    // Video tracks carry a width; the audio ones do not.
    is_audio && format.get("width").is_none()
}

fn is_auto_dubbed(format: &Value) -> bool {
    if let Some(track) = format.get("audioTrack") {
        if track.get("isAutoDubbed").and_then(Value::as_bool) == Some(true) {
            return true;
        }
        if track.get("audioIsDefault").and_then(Value::as_bool) == Some(false) {
            return true;
        }
        let label = text(track.get("displayName")).unwrap_or_default();
        if label.to_lowercase().contains("auto") {
            return true;
        }
    }
    if let Some(xtags) = format.get("xtags").and_then(Value::as_str) {
        // Alternate audio tracks are tagged `acont=<id>`.
        if xtags.contains("acont=") {
            return true;
        }
    }
    false
}

fn mime_of(format: &Value) -> &str {
    format.get("mimeType").and_then(Value::as_str).unwrap_or("")
}

fn bitrate_of(format: &Value) -> u64 {
    format.get("bitrate").and_then(number).unwrap_or(0)
}

/// Reads one value out of a query string without pulling in a URL crate.
pub(crate) fn query_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let query = url.split_once('?')?.1;
    for pair in query.split('&') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if name == key {
            return Some(value);
        }
    }
    None
}

/// Picks the best audio stream out of a `player` response.
///
/// The error is a human-readable reason; the caller tries the next client (or reports it as
/// [`Error::Unplayable`]).
pub(crate) fn stream_from_player(
    root: &Value,
    user_agent: &str,
) -> std::result::Result<Stream, String> {
    let status = at(root, &["playabilityStatus", "status"])
        .and_then(Value::as_str)
        .unwrap_or("");
    if status != "OK" {
        let reason = at(root, &["playabilityStatus", "reason"])
            .and_then(Value::as_str)
            .unwrap_or("");
        return Err(if reason.is_empty() {
            format!("the player said {status}")
        } else {
            reason.to_string()
        });
    }

    let streaming = root
        .get("streamingData")
        .ok_or_else(|| "the player returned no streaming data".to_string())?;

    let mut audio = Vec::new();
    for key in ["adaptiveFormats", "formats"] {
        if let Some(formats) = streaming.get(key).and_then(Value::as_array) {
            audio.extend(formats.iter().filter(|format| is_audio_format(format)));
        }
    }

    let with_url: Vec<&Value> = audio
        .iter()
        .copied()
        .filter(|format| format.get("url").and_then(Value::as_str).is_some())
        .collect();
    if with_url.is_empty() {
        return Err(if audio.is_empty() {
            "the player returned no audio track".to_string()
        } else {
            "every audio track is ciphered (signatureCipher), no direct URL".to_string()
        });
    }

    let original: Vec<&Value> = with_url
        .iter()
        .copied()
        .filter(|format| !is_auto_dubbed(format))
        .collect();
    let pool = if original.is_empty() {
        with_url
    } else {
        original
    };

    let best = pool
        .iter()
        .copied()
        .filter(|format| mime_of(format).starts_with("audio/mp4"))
        .max_by_key(|format| bitrate_of(format))
        .ok_or_else(|| "no AAC (audio/mp4) track; only Opus/WebM is available".to_string())?;

    let url = best
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| "the chosen track has no URL".to_string())?
        .to_string();

    let content_length = best
        .get("contentLength")
        .and_then(number)
        .or_else(|| query_param(&url, "clen").and_then(|raw| raw.parse::<u64>().ok()));

    let duration = at(root, &["videoDetails", "lengthSeconds"])
        .and_then(number)
        .map(Duration::from_secs)
        .or_else(|| {
            best.get("approxDurationMs")
                .and_then(number)
                .map(Duration::from_millis)
        });

    let loudness_db = at(root, &["playerConfig", "audioConfig", "loudnessDb"])
        .and_then(Value::as_f64)
        .map(|db| db as f32);

    let expires_in = streaming
        .get("expiresInSeconds")
        .and_then(number)
        .unwrap_or(6 * 60 * 60);

    Ok(Stream {
        url,
        headers: vec![("User-Agent".to_string(), user_agent.to_string())],
        mime_type: mime_of(best).to_string(),
        bitrate: bitrate_of(best) as u32,
        content_length,
        duration,
        loudness_db,
        expires_at: Instant::now() + Duration::from_secs(expires_in),
    })
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;
