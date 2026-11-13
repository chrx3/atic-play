//! The app's single source of truth: what is loaded, what plays, and where the user is.
//!
//! Views read it and call its methods; they never talk to the network or the audio engine
//! themselves. Everything slow happens in a spawned task that comes back with `this.update`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext as _, Context, Entity, Hsla, SharedString, hsla};
use gpui_m3::{Scheme, SchemeVariant, animate_scheme};

use crate::audio::Engine;
use crate::covers::CoverCache;
use crate::dev;
use crate::source::{
    AlbumCard, AlbumDetail, ArtistCard, ArtistDetail, Card, MusicSource, PlaylistCard,
    SearchResults, Shelf, StreamInfo, Track,
};

/// Playback position, apart from [`Model`] so its ticks redraw only the progress bars.
#[derive(Default)]
pub struct Clock {
    pub position: f32,
    pub duration: f32,
}

/// A value that arrives later, with the three states the UI has to tell apart.
#[derive(Clone, Debug)]
pub enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(SharedString),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Load::Loading)
    }

    pub fn is_idle(&self) -> bool {
        matches!(self, Load::Idle)
    }
}

#[derive(Clone)]
pub enum LyricsState {
    Idle,
    Loading,
    None,
    Ready(Arc<play_lyrics::Lyrics>),
    Failed(SharedString),
}

/// Which page the main column shows.
pub enum View {
    Home,
    Search,
    Album {
        id: String,
        load: Load<Arc<AlbumDetail>>,
    },
    Artist {
        id: String,
        load: Load<Arc<ArtistDetail>>,
    },
    Playlist {
        id: String,
        title: String,
        load: Load<Arc<Vec<Track>>>,
    },
    Library,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    Home,
    Search,
    Album,
    Artist,
    Playlist,
    Library,
}

impl View {
    pub fn kind(&self) -> ViewKind {
        match self {
            View::Home => ViewKind::Home,
            View::Search => ViewKind::Search,
            View::Album { .. } => ViewKind::Album,
            View::Artist { .. } => ViewKind::Artist,
            View::Playlist { .. } => ViewKind::Playlist,
            View::Library => ViewKind::Library,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    All,
    One,
}

impl RepeatMode {
    pub fn next(self) -> Self {
        match self {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        }
    }
}

const MAX_BACK: usize = 24;

/// Where a page load should go, once the borrow of the view has ended.
enum Pending {
    Album(String),
    Artist(String),
    Playlist(String),
}

pub struct Model {
    pub source: Arc<dyn MusicSource>,
    pub covers: Entity<CoverCache>,
    pub clock: Entity<Clock>,
    pub audio: Engine,

    /// The account in the keyring, and the pages that need it.
    pub account: crate::account::AccountState,
    pub library: crate::library::Library,
    pub dialogs: crate::dialogs::Dialogs,

    pub home: Load<Arc<Vec<Shelf>>>,
    pub view: View,
    back: Vec<View>,
    nav_generation: u64,
    home_generation: u64,

    pub query: SharedString,
    pub search: Load<Arc<SearchResults>>,
    pub suggestions: Vec<String>,
    /// Whether the suggestion list is showing under the search bar. It is only up while the
    /// field has focus and the text changed since the last search; see [`Model::set_query`].
    pub suggestions_open: bool,
    pub search_tab: usize,
    search_generation: u64,
    last_search: SharedString,

    pub current: Option<Track>,
    pub queue: Vec<Track>,
    pub history: Vec<Track>,
    seed_queue: Vec<Track>,
    pub playing: bool,
    pub loading_audio: bool,
    pub audio_ready: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub volume: f32,
    playback_generation: u64,
    queue_generation: u64,

    pub lyrics: LyricsState,
    lyrics_generation: u64,

    pub queue_open: bool,
    pub now_open: bool,
    pub toast: Option<(SharedString, u64)>,
    pub dark: bool,
    seed: Hsla,
    random_state: u64,
}

impl Model {
    pub fn new(source: Arc<dyn MusicSource>, cx: &mut Context<Self>) -> Self {
        let covers = cx.new(|_| CoverCache::new());
        let clock = cx.new(|_| Clock::default());
        let dark = dev::dark();
        let mut model = Self {
            source,
            covers: covers.clone(),
            clock,
            audio: Engine::new(),
            account: crate::account::AccountState::default(),
            library: crate::library::Library::default(),
            dialogs: crate::dialogs::Dialogs::new(cx),
            home: Load::Loading,
            view: View::Home,
            back: Vec::new(),
            nav_generation: 0,
            home_generation: 0,
            query: SharedString::default(),
            search: Load::Idle,
            suggestions: Vec::new(),
            suggestions_open: false,
            search_tab: 0,
            search_generation: 0,
            last_search: SharedString::default(),
            current: None,
            queue: Vec::new(),
            history: Vec::new(),
            seed_queue: Vec::new(),
            playing: false,
            loading_audio: false,
            audio_ready: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            volume: 72.,
            playback_generation: 0,
            queue_generation: 0,
            lyrics: LyricsState::Idle,
            lyrics_generation: 0,
            queue_open: false,
            now_open: false,
            toast: None,
            dark,
            seed: hsla(0.62, 0.55, 0.5, 1.),
            random_state: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0x9e37_79b9, |d| d.as_nanos() as u64),
        };
        cx.observe(&covers, |model, covers, cx| {
            let Some(track) = model.current.clone() else {
                return;
            };
            let Some(seed) = covers.read(cx).seed(track.thumbnail.as_deref()) else {
                return;
            };
            if seed != model.seed {
                model.seed = seed;
                model.apply_theme(cx);
            }
        })
        .detach();
        model.refresh_home(cx);
        model.start_account(cx);
        model
    }

    pub fn scheme(&self) -> Scheme {
        Scheme::from_seed_variant(self.seed, self.dark, SchemeVariant::TonalSpot, 0.)
    }

    pub fn apply_theme(&self, cx: &mut App) {
        animate_scheme(self.scheme(), self.dark, cx);
    }

    pub fn set_dark(&mut self, dark: bool, cx: &mut Context<Self>) {
        self.dark = dark;
        self.apply_theme(cx);
        cx.notify();
    }

    /// A short message at the bottom of the window.
    pub fn say(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let generation = self.toast.as_ref().map_or(1, |(_, g)| g + 1);
        self.toast = Some((text.into(), generation));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(3400))
                .await;
            this.update(cx, |model, cx| {
                if model.toast.as_ref().is_some_and(|(_, g)| *g == generation) {
                    model.toast = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------ home

    pub fn refresh_home(&mut self, cx: &mut Context<Self>) {
        self.home_generation += 1;
        let generation = self.home_generation;
        self.home = Load::Loading;
        cx.notify();
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { source.home() }).await;
            this.update(cx, |model, cx| {
                if model.home_generation != generation {
                    return;
                }
                model.home = match result {
                    Ok(shelves) => {
                        log::info!("home: {} shelves", shelves.len());
                        Load::Ready(Arc::new(shelves))
                    }
                    Err(err) => {
                        log::warn!("home failed: {err}");
                        Load::Failed(err.into())
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The first song of the loaded shelves, for the play button before anything is chosen and
    /// for the `ATIC_PLAY_OPEN` states that start from a song.
    pub fn first_home_song(&self) -> Option<Track> {
        let shelves = self.home.ready()?;
        shelves.iter().find_map(|shelf| {
            shelf.cards.iter().find_map(|card| match card {
                Card::Song(track) => Some(track.clone()),
                _ => None,
            })
        })
    }

    /// The first album of the loaded shelves, in reading order.
    pub fn first_home_album(&self) -> Option<AlbumCard> {
        self.home.ready()?.iter().find_map(|shelf| {
            shelf.cards.iter().find_map(|card| match card {
                Card::Album(album) => Some(album.clone()),
                _ => None,
            })
        })
    }

    /// The first artist of the loaded shelves, in reading order.
    pub fn first_home_artist(&self) -> Option<ArtistCard> {
        self.home.ready()?.iter().find_map(|shelf| {
            shelf.cards.iter().find_map(|card| match card {
                Card::Artist(artist) => Some(artist.clone()),
                _ => None,
            })
        })
    }

    /// The first playlist of the loaded shelves, in reading order.
    pub fn first_home_playlist(&self) -> Option<PlaylistCard> {
        self.home.ready()?.iter().find_map(|shelf| {
            shelf.cards.iter().find_map(|card| match card {
                Card::Playlist(playlist) => Some(playlist.clone()),
                _ => None,
            })
        })
    }

    /// The first artist of the search results, for the artist page when the shelves have none.
    pub fn first_search_artist(&self) -> Option<ArtistCard> {
        self.search.ready()?.artists.first().cloned()
    }

    /// The first album of the search results, for the album page when the shelves have none.
    pub fn first_search_album(&self) -> Option<AlbumCard> {
        self.search.ready()?.albums.first().cloned()
    }

    /// Whether the shelves finished loading (or gave up): what `ATIC_PLAY_OPEN` waits for.
    pub fn home_settled(&self) -> bool {
        !matches!(self.home, Load::Idle | Load::Loading)
    }

    /// Whether the search results finished loading (or gave up).
    pub fn search_settled(&self) -> bool {
        !matches!(self.search, Load::Idle | Load::Loading)
    }

    /// Whether the album, artist or playlist page that is open finished loading (or gave up).
    pub fn view_settled(&self) -> bool {
        match &self.view {
            View::Album { load, .. } => !load.is_idle() && !load.is_loading(),
            View::Artist { load, .. } => !load.is_idle() && !load.is_loading(),
            View::Playlist { load, .. } => !load.is_idle() && !load.is_loading(),
            View::Home | View::Search | View::Library => true,
        }
    }

    // ------------------------------------------------------------ navigation

    pub(crate) fn push_view(&mut self, view: View, cx: &mut Context<Self>) {
        self.nav_generation += 1;
        self.suggestions_open = false;
        let mut previous = std::mem::replace(&mut self.view, View::Home);
        // A page that was still loading will be reloaded when it comes back.
        match &mut previous {
            View::Album { load, .. } if load.is_loading() => *load = Load::Idle,
            View::Artist { load, .. } if load.is_loading() => *load = Load::Idle,
            View::Playlist { load, .. } if load.is_loading() => *load = Load::Idle,
            _ => {}
        }
        self.back.push(previous);
        if self.back.len() > MAX_BACK {
            self.back.remove(0);
        }
        self.view = view;
        cx.notify();
    }

    pub fn go_home(&mut self, cx: &mut Context<Self>) {
        self.nav_generation += 1;
        self.back.clear();
        self.suggestions_open = false;
        self.view = View::Home;
        cx.notify();
    }

    pub fn go_search(&mut self, cx: &mut Context<Self>) {
        if self.view.kind() != ViewKind::Search {
            self.nav_generation += 1;
            self.view = View::Search;
        }
        cx.notify();
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        self.nav_generation += 1;
        self.suggestions_open = false;
        self.view = self.back.pop().unwrap_or(View::Home);
        cx.notify();
        self.ensure_loaded(cx);
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty() || self.view.kind() != ViewKind::Home
    }

    /// How many times the page has changed. Views use it to replay their entrance, and to know
    /// that a scroll or a measurement belongs to another page.
    pub fn nav_generation(&self) -> u64 {
        self.nav_generation
    }

    fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        let pending = match &self.view {
            View::Album { id, load } if load.is_idle() => Some(Pending::Album(id.clone())),
            View::Artist { id, load } if load.is_idle() => Some(Pending::Artist(id.clone())),
            View::Playlist { id, load, .. } if load.is_idle() => {
                Some(Pending::Playlist(id.clone()))
            }
            _ => None,
        };
        match pending {
            Some(Pending::Album(id)) => self.load_album(id, cx),
            Some(Pending::Artist(id)) => self.load_artist(id, cx),
            Some(Pending::Playlist(id)) => self.load_playlist(id, cx),
            None => {}
        }
    }

    pub fn open_album(&mut self, id: String, cx: &mut Context<Self>) {
        self.push_view(
            View::Album {
                id: id.clone(),
                load: Load::Loading,
            },
            cx,
        );
        self.load_album(id, cx);
    }

    pub fn open_artist(&mut self, id: String, cx: &mut Context<Self>) {
        self.push_view(
            View::Artist {
                id: id.clone(),
                load: Load::Loading,
            },
            cx,
        );
        self.load_artist(id, cx);
    }

    pub fn open_playlist(&mut self, id: String, title: String, cx: &mut Context<Self>) {
        self.push_view(
            View::Playlist {
                id: id.clone(),
                title,
                load: Load::Loading,
            },
            cx,
        );
        self.load_playlist(id, cx);
    }

    fn load_album(&mut self, id: String, cx: &mut Context<Self>) {
        let generation = self.nav_generation + 1;
        self.nav_generation = generation;
        if let View::Album { id: current, load } = &mut self.view {
            if current == &id {
                *load = Load::Loading;
            }
        }
        cx.notify();
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let label = id.clone();
            let result = cx.background_spawn(async move { source.album(&id) }).await;
            this.update(cx, |model, cx| {
                if model.nav_generation != generation {
                    return;
                }
                if let View::Album { load, .. } = &mut model.view {
                    *load = match result {
                        Ok(detail) => Load::Ready(Arc::new(detail)),
                        Err(err) => {
                            log::warn!("album «{label}» failed: {err}");
                            Load::Failed(err.into())
                        }
                    };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn load_artist(&mut self, id: String, cx: &mut Context<Self>) {
        let generation = self.nav_generation + 1;
        self.nav_generation = generation;
        if let View::Artist { id: current, load } = &mut self.view {
            if current == &id {
                *load = Load::Loading;
            }
        }
        cx.notify();
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let label = id.clone();
            let result = cx.background_spawn(async move { source.artist(&id) }).await;
            this.update(cx, |model, cx| {
                if model.nav_generation != generation {
                    return;
                }
                if let View::Artist { load, .. } = &mut model.view {
                    *load = match result {
                        Ok(detail) => Load::Ready(Arc::new(detail)),
                        Err(err) => {
                            log::warn!("artist «{label}» failed: {err}");
                            Load::Failed(err.into())
                        }
                    };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn load_playlist(&mut self, id: String, cx: &mut Context<Self>) {
        let generation = self.nav_generation + 1;
        self.nav_generation = generation;
        if let View::Playlist { id: current, load, .. } = &mut self.view {
            if current == &id {
                *load = Load::Loading;
            }
        }
        cx.notify();
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let label = id.clone();
            let result = cx.background_spawn(async move { source.playlist(&id) }).await;
            this.update(cx, |model, cx| {
                if model.nav_generation != generation {
                    return;
                }
                if let View::Playlist { load, .. } = &mut model.view {
                    *load = match result {
                        Ok(tracks) => Load::Ready(Arc::new(tracks)),
                        Err(err) => {
                            log::warn!("playlist «{label}» failed: {err}");
                            Load::Failed(err.into())
                        }
                    };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Re-opens the page that failed: same id, fresh request.
    pub fn retry_view(&mut self, cx: &mut Context<Self>) {
        let pending = match &self.view {
            View::Album { id, .. } => Some(Pending::Album(id.clone())),
            View::Artist { id, .. } => Some(Pending::Artist(id.clone())),
            View::Playlist { id, .. } => Some(Pending::Playlist(id.clone())),
            _ => None,
        };
        match pending {
            Some(Pending::Album(id)) => self.load_album(id, cx),
            Some(Pending::Artist(id)) => self.load_artist(id, cx),
            Some(Pending::Playlist(id)) => self.load_playlist(id, cx),
            None => self.refresh_home(cx),
        }
    }

    // ------------------------------------------------------------ search

    pub fn set_query(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.start_search(text.into(), Duration::from_millis(250), cx);
    }

    pub fn submit_query(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.start_search(text.into(), Duration::ZERO, cx);
    }

    /// Puts the suggestion list away without touching the text or the results.
    pub fn close_suggestions(&mut self, cx: &mut Context<Self>) {
        if self.suggestions_open {
            self.suggestions_open = false;
            cx.notify();
        }
    }

    fn start_search(
        &mut self,
        text: SharedString,
        delay: Duration,
        cx: &mut Context<Self>,
    ) {
        self.query = text;
        if self.view.kind() != ViewKind::Search {
            self.nav_generation += 1;
            self.view = View::Search;
            self.suggestions_open = false;
        }
        self.search_generation += 1;
        let generation = self.search_generation;
        let query = self.query.trim().to_string();
        self.suggestions.clear();
        // Typing shows the suggestions until the results of that search arrive; Enter always
        // closes them. Editing back to the text that was already searched does not reopen them.
        let changed = query != self.last_search;
        self.last_search = query.clone().into();
        self.suggestions_open = delay > Duration::ZERO && !query.is_empty() && changed;
        if query.is_empty() {
            self.search = Load::Idle;
            cx.notify();
            return;
        }
        self.search = Load::Loading;
        cx.notify();
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            if delay > Duration::ZERO {
                cx.background_executor().timer(delay).await;
            }
            let current = this
                .update(cx, |model, cx| {
                    if model.search_generation != generation {
                        return false;
                    }
                    model.spawn_suggestions(query.clone(), generation, cx);
                    true
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let search_query = query.clone();
            let results = cx
                .background_spawn(async move { source.search(&search_query) })
                .await;
            this.update(cx, |model, cx| {
                if model.search_generation != generation {
                    return;
                }
                model.search = match results {
                    Ok(results) => {
                        log::info!(
                            "search «{query}»: {} songs, {} albums, {} artists, {} playlists",
                            results.songs.len(),
                            results.albums.len(),
                            results.artists.len(),
                            results.playlists.len()
                        );
                        Load::Ready(Arc::new(results))
                    }
                    Err(err) => {
                        log::warn!("search «{query}» failed: {err}");
                        Load::Failed(err.into())
                    }
                };
                // The results take over: the suggestions were only there while waiting.
                model.suggestions_open = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Asks the source for suggestions and keeps them if nobody searched in the meantime.
    fn spawn_suggestions(&mut self, query: String, generation: u64, cx: &mut Context<Self>) {
        let source = self.source.clone();
        let label = query.clone();
        cx.spawn(async move |this, cx| {
            let list = cx
                .background_spawn(async move { source.suggestions(&query) })
                .await;
            this.update(cx, |model, cx| {
                if model.search_generation != generation {
                    return;
                }
                match list {
                    Ok(list) => model.suggestions = list,
                    Err(err) => log::warn!("suggestions «{label}» failed: {err}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Development hook (`ATIC_PLAY_OPEN=suggest`): types `text` and leaves its suggestions up,
    /// without running the search, so the panel can be looked at on purpose.
    pub fn preview_suggestions(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.query = text.into();
        if self.view.kind() != ViewKind::Search {
            self.nav_generation += 1;
            self.view = View::Search;
        }
        self.search_generation += 1;
        let generation = self.search_generation;
        let query = self.query.trim().to_string();
        self.search = Load::Idle;
        self.suggestions.clear();
        self.last_search = self.query.clone();
        self.suggestions_open = !query.is_empty();
        cx.notify();
        if !query.is_empty() {
            self.spawn_suggestions(query, generation, cx);
        }
    }

    pub fn set_search_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        self.search_tab = tab;
        cx.notify();
    }

    // ------------------------------------------------------------ playback

    pub fn current_id(&self) -> Option<&str> {
        self.current.as_ref().map(|track| track.id.as_str())
    }

    /// Plays one song and lets the radio (`up_next`) fill the queue after it.
    pub fn play_now(&mut self, track: Track, cx: &mut Context<Self>) {
        self.play_list(vec![track], 0, cx);
        self.refresh_queue(cx);
    }

    /// Plays `tracks[index]` and keeps the rest of the list as the queue.
    pub fn play_list(&mut self, mut tracks: Vec<Track>, index: usize, cx: &mut Context<Self>) {
        if tracks.is_empty() {
            return;
        }
        let index = index.min(tracks.len() - 1);
        let rest = tracks.split_off(index + 1);
        let current = tracks[index].clone();
        self.history.clear();
        self.seed_queue = tracks;
        self.queue = rest;
        self.start_track(current, cx);
    }

    pub fn play_shuffled(&mut self, tracks: Vec<Track>, cx: &mut Context<Self>) {
        if tracks.is_empty() {
            return;
        }
        let mut tracks = tracks;
        for i in (1..tracks.len()).rev() {
            let j = (self.next_random() % (i as u64 + 1)) as usize;
            tracks.swap(i, j);
        }
        self.shuffle = true;
        self.play_list(tracks, 0, cx);
    }

    fn next_random(&mut self) -> u64 {
        self.random_state = self
            .random_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.random_state >> 17
    }

    fn start_track(&mut self, track: Track, cx: &mut Context<Self>) {
        log::info!(
            "play {} «{}» — {}",
            track.id,
            track.title,
            track.artist
        );
        self.show_track(track.clone(), cx);
        let generation = self.playback_generation;
        self.playing = true;
        self.loading_audio = true;
        cx.notify();

        if !self.audio.available() {
            self.playing = false;
            self.loading_audio = false;
            let reason = self
                .audio
                .error()
                .unwrap_or("no hay una salida de audio disponible")
                .to_string();
            log::error!("audio engine is not available: {reason}");
            self.say(format!("Sin sonido: {reason}"), cx);
            return;
        }

        let source = self.source.clone();
        let id = track.id.clone();
        cx.spawn(async move |this, cx| {
            let stream = cx
                .background_spawn(async move {
                    let result = source.stream(&id);
                    match &result {
                        Ok(info) => log::info!(
                            "stream {id}: mime={} bitrate={}",
                            info.mime.as_deref().unwrap_or("?"),
                            info.bitrate
                                .map_or_else(|| "?".to_string(), |b| format!("{b} bps"))
                        ),
                        Err(err) => log::warn!("stream {id} failed: {err}"),
                    }
                    result
                })
                .await;
            this.update(cx, |model, cx| {
                if model.playback_generation != generation {
                    return;
                }
                model.loading_audio = false;
                match stream {
                    Ok(info) => model.load_stream(info, cx),
                    Err(err) => {
                        model.playing = false;
                        model.say(format!("No se pudo reproducir: {err}"), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Makes `track` the current one — title, clock, theme seed and lyrics — without any sound.
    /// It also cancels whatever the engine was loading for the previous song.
    fn show_track(&mut self, track: Track, cx: &mut Context<Self>) {
        self.playback_generation += 1;
        self.current = Some(track.clone());
        self.audio_ready = false;
        self.clock.update(cx, |clock, cx| {
            clock.position = 0.;
            clock.duration = track.secs();
            cx.notify();
        });
        self.seed = self.seed_for(&track, cx);
        self.apply_theme(cx);
        self.load_lyrics(cx);
        cx.notify();
    }

    /// Development hook (`ATIC_PLAY_OPEN=now|lyrics|queue`): loads `track` as the current song,
    /// paused, with the same radio queue a click would build. It never touches the audio engine,
    /// so nothing starts playing and the volume stays as it was.
    pub fn open_paused(&mut self, track: Track, cx: &mut Context<Self>) {
        log::info!(
            "open paused {} «{}» — {}",
            track.id,
            track.title,
            track.artist
        );
        self.playing = false;
        self.loading_audio = false;
        self.show_track(track, cx);
        self.refresh_queue(cx);
    }

    fn load_stream(&mut self, info: StreamInfo, cx: &mut Context<Self>) {
        if self.audio.load(&info) {
            self.audio_ready = true;
            self.audio.set_volume(self.volume / 100.);
            self.audio.play();
            self.playing = true;
        } else {
            self.playing = false;
            let reason = self
                .audio
                .error()
                .unwrap_or("el motor de audio no aceptó la canción")
                .to_string();
            log::error!("audio engine refused the stream: {reason}");
            self.say(format!("Sin sonido: {reason}"), cx);
        }
    }

    /// Asks the radio station that follows `current` for the next songs.
    fn refresh_queue(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.current.clone() else {
            return;
        };
        self.queue_generation += 1;
        let generation = self.queue_generation;
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let list = cx
                .background_spawn(async move { source.up_next(&track.id) })
                .await;
            this.update(cx, |model, cx| {
                if model.queue_generation != generation {
                    return;
                }
                let Ok(list) = list else {
                    return;
                };
                let current = model.current_id().map(str::to_string);
                let queue: Vec<Track> = list
                    .into_iter()
                    .filter(|track| Some(&track.id) != current.as_ref())
                    .collect();
                if queue.is_empty() {
                    return;
                }
                model.seed_queue = queue.clone();
                model.queue = queue;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.current.clone() else {
            match self.first_home_song() {
                Some(track) => self.play_now(track, cx),
                None => self.say("Todavía no hay nada para reproducir", cx),
            }
            return;
        };
        if self.playing {
            self.playing = false;
            self.audio.pause();
            cx.notify();
            return;
        }
        if !self.audio_ready {
            self.start_track(track, cx);
            return;
        }
        let (position, duration) = {
            let clock = self.clock.read(cx);
            (clock.position, clock.duration)
        };
        if duration > 0. && position >= duration - 0.5 {
            self.seek(0., cx);
        }
        self.playing = true;
        self.audio.play();
        cx.notify();
    }

    pub fn next(&mut self, cx: &mut Context<Self>) {
        if self.repeat == RepeatMode::One {
            self.restart(cx);
            return;
        }
        let next = if self.shuffle && !self.queue.is_empty() {
            let index = (self.next_random() as usize) % self.queue.len();
            Some(self.queue.remove(index))
        } else if !self.queue.is_empty() {
            Some(self.queue.remove(0))
        } else {
            None
        };
        if let Some(track) = next {
            if let Some(current) = self.current.clone() {
                self.history.push(current);
            }
            self.start_track(track, cx);
            return;
        }
        if self.repeat == RepeatMode::All {
            let current = self.current_id().map(str::to_string);
            let mut again: Vec<Track> = self
                .seed_queue
                .iter()
                .filter(|track| Some(&track.id) != current.as_ref())
                .cloned()
                .collect();
            if again.is_empty() {
                again = self.history.clone();
            }
            if let Some(track) = again.first().cloned() {
                again.remove(0);
                self.queue = again;
                if let Some(current) = self.current.clone() {
                    self.history.push(current);
                }
                self.start_track(track, cx);
                return;
            }
        }
        self.stop_playback(cx);
    }

    pub fn previous(&mut self, cx: &mut Context<Self>) {
        let position = self.clock.read(cx).position;
        if position > 3. {
            self.seek(0., cx);
            return;
        }
        if let Some(track) = self.history.pop() {
            if let Some(current) = self.current.clone() {
                self.queue.insert(0, current);
            }
            self.start_track(track, cx);
        } else {
            self.seek(0., cx);
        }
    }

    fn restart(&mut self, cx: &mut Context<Self>) {
        self.seek(0., cx);
        self.playing = true;
        self.audio.play();
        cx.notify();
    }

    fn stop_playback(&mut self, cx: &mut Context<Self>) {
        self.playing = false;
        self.loading_audio = false;
        self.audio.stop();
        cx.notify();
    }

    pub fn seek(&mut self, seconds: f32, cx: &mut Context<Self>) {
        let seconds = seconds.max(0.);
        self.audio.seek(Duration::from_secs_f32(seconds));
        self.clock.update(cx, |clock, cx| {
            clock.position = seconds;
            cx.notify();
        });
    }

    pub fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.volume = volume.clamp(0., 100.);
        self.audio.set_volume(self.volume / 100.);
        cx.notify();
    }

    pub fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        self.shuffle = !self.shuffle;
        cx.notify();
    }

    pub fn cycle_repeat(&mut self, cx: &mut Context<Self>) {
        self.repeat = self.repeat.next();
        cx.notify();
    }

    pub fn play_queue_item(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.queue.len() {
            return;
        }
        let track = self.queue.remove(index);
        if let Some(current) = self.current.clone() {
            self.history.push(current);
        }
        self.start_track(track, cx);
    }

    pub fn clear_queue(&mut self, cx: &mut Context<Self>) {
        self.queue.clear();
        cx.notify();
    }

    /// Called on a timer: drains the engine's events and moves the clock.
    pub fn tick(&mut self, cx: &mut Context<Self>) {
        if !self.audio.available() {
            return;
        }
        let mut state_changed = false;
        let mut ended = false;
        let mut duration = None;
        for event in self.audio.poll_events() {
            match event {
                play_audio::Event::State(play_audio::State::Playing) => {
                    self.playing = true;
                    self.loading_audio = false;
                    state_changed = true;
                }
                play_audio::Event::State(play_audio::State::Paused)
                | play_audio::Event::State(play_audio::State::Idle) => {
                    if self.playing {
                        self.playing = false;
                        state_changed = true;
                    }
                }
                play_audio::Event::State(play_audio::State::Loading) => {
                    if !self.loading_audio {
                        self.loading_audio = true;
                        state_changed = true;
                    }
                }
                play_audio::Event::State(play_audio::State::Ended) => ended = true,
                play_audio::Event::Duration(value) => duration = Some(value),
                play_audio::Event::Error(err) => {
                    log::error!("audio engine: {err}");
                    self.playing = false;
                    state_changed = true;
                    self.say(format!("Error de audio: {err}"), cx);
                }
            }
        }
        if self.current.is_none() {
            return;
        }
        let position = self.audio.position().as_secs_f32();
        let (last_position, last_duration) = {
            let clock = self.clock.read(cx);
            (clock.position, clock.duration)
        };
        let new_duration = duration.map_or(last_duration, |d| d.as_secs_f32());
        if (position - last_position).abs() > 0.004 || new_duration != last_duration {
            self.clock.update(cx, |clock, cx| {
                clock.position = position;
                clock.duration = new_duration;
                cx.notify();
            });
        }
        if state_changed {
            cx.notify();
        }
        if ended {
            self.next(cx);
        }
    }

    // ------------------------------------------------------------ lyrics

    fn load_lyrics(&mut self, cx: &mut Context<Self>) {
        self.lyrics_generation += 1;
        let generation = self.lyrics_generation;
        let Some(track) = self.current.clone() else {
            self.lyrics = LyricsState::Idle;
            return;
        };
        self.lyrics = LyricsState::Loading;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let query = play_lyrics::Query {
                        title: &track.title,
                        artist: &track.artist,
                        album: track.album.as_deref(),
                        duration: track.duration,
                    };
                    catch_unwind(AssertUnwindSafe(|| play_lyrics::fetch(&query)))
                })
                .await;
            this.update(cx, |model, cx| {
                if model.lyrics_generation != generation {
                    return;
                }
                model.lyrics = match result {
                    Ok(Ok(Some(lyrics))) => LyricsState::Ready(Arc::new(lyrics)),
                    Ok(Ok(None)) => LyricsState::None,
                    Ok(Err(err)) => LyricsState::Failed(err.to_string().into()),
                    Err(_) => LyricsState::Failed(
                        "la búsqueda de letras todavía no está disponible".into(),
                    ),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------ helpers

    fn seed_for(&self, track: &Track, cx: &App) -> Hsla {
        if let Some(seed) = self.covers.read(cx).seed(track.thumbnail.as_deref()) {
            return seed;
        }
        let mut hash: u32 = 2166136261;
        for byte in track.title.as_bytes().iter().chain(track.artist.as_bytes()) {
            hash ^= *byte as u32;
            hash = hash.wrapping_mul(16777619);
        }
        hsla((hash % 360) as f32 / 360., 0.5, 0.45, 1.)
    }

    /// Why there is no sound, when the engine could not start at all.
    pub fn audio_error(&self) -> Option<&str> {
        self.audio.error()
    }

    pub fn source_name(&self) -> &'static str {
        self.source.name()
    }
}

/// "Song · Artist · Album" lines and durations, shared by the views.
///
/// The page writes hours and minutes now ([`length_text`]); this stays for a view that only
/// wants whole minutes.
#[allow(dead_code)]
pub fn track_count_text(count: usize, total: Duration) -> String {
    let songs = match count {
        0 => "sin canciones".to_string(),
        1 => "1 canción".to_string(),
        n => format!("{n} canciones"),
    };
    let minutes = total.as_secs() / 60;
    if minutes == 0 {
        songs
    } else {
        format!("{songs} · {minutes} min")
    }
}

/// "4 h 7 min", "43 min" or "45 s" for a total length; nothing when it is unknown. Hours save
/// the reader from counting three-digit minutes.
pub fn length_text(total: Duration) -> String {
    let secs = total.as_secs();
    if secs == 0 {
        return String::new();
    }
    let (hours, minutes) = (secs / 3600, (secs % 3600) / 60);
    match (hours, minutes) {
        (0, 0) => format!("{secs} s"),
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

/// A click handler that updates the model.
pub fn click(
    model: &Entity<Model>,
    f: impl Fn(&mut Model, &mut Context<Model>) + 'static,
) -> impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut App) + 'static {
    let model = model.clone();
    move |_, _, cx| model.update(cx, |model, cx| f(model, cx))
}

/// A value-change handler that sets the volume (sliders send 0..=100).
pub fn change_volume(
    model: &Entity<Model>,
) -> impl Fn(f32, &mut gpui::Window, &mut App) + 'static {
    let model = model.clone();
    move |value, _, cx| model.update(cx, |model, cx| model.set_volume(value, cx))
}

/// The line being sung at `position`: the last one that already started.
pub fn line_at(lines: &[play_lyrics::Line], position: Duration) -> Option<usize> {
    if lines.is_empty() {
        return None;
    }
    match lines.binary_search_by(|line| line.start.cmp(&position)) {
        Ok(index) => Some(index),
        Err(0) => None,
        Err(index) => Some(index - 1),
    }
}

/// Total length of a list of tracks.
pub fn total_duration(tracks: &[Track]) -> Duration {
    tracks
        .iter()
        .filter_map(|track| track.duration)
        .sum::<Duration>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use play_lyrics::Line;

    fn line(secs: u64) -> Line {
        Line {
            start: Duration::from_secs(secs),
            text: format!("line {secs}"),
        }
    }

    #[test]
    fn line_at_finds_the_line_that_started_last() {
        let lines = vec![line(0), line(5), line(10)];
        assert_eq!(line_at(&lines, Duration::ZERO), Some(0));
        assert_eq!(line_at(&lines, Duration::from_secs(5)), Some(1));
        assert_eq!(line_at(&lines, Duration::from_secs(7)), Some(1));
        assert_eq!(line_at(&lines, Duration::from_secs(99)), Some(2));
        let later = vec![line(5)];
        assert_eq!(line_at(&later, Duration::from_secs(1)), None);
    }

    #[test]
    fn repeat_cycles() {
        assert_eq!(RepeatMode::Off.next(), RepeatMode::All);
        assert_eq!(RepeatMode::All.next(), RepeatMode::One);
        assert_eq!(RepeatMode::One.next(), RepeatMode::Off);
    }
}
