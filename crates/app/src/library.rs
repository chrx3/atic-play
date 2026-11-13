//! Biblioteca: what the account saved — Me gusta, listas, álbumes, artistas and historial.
//!
//! The data and the page live here, but the rows and the cards are the ones from
//! [`crate::page`]: the library is another way of walking the same catalog, not another look.
//!
//! Every list loads on its own, so opening a tab never waits for the other four.

use std::sync::Arc;

use gpui::{
    AnyElement, AppContext as _, ClickEvent, Context, Entity, IntoElement, ParentElement,
    SharedString, Styled, Window, div, px, prelude::FluentBuilder as _,
};
use gpui_m3::{
    Scheme, Segment, SegmentedButtons,
    theme::{
        alpha,
        type_scale::{BODY_MEDIUM, HEADLINE_SMALL, TypeScaleExt},
    },
};

use crate::icons::icon;
use crate::model::{Load, Model, ViewKind};
use crate::page::{Page, error_block, heading, loading_block};
use crate::source::{AlbumCard, ArtistCard, PlaylistCard, Track};

/// The tabs, in the order the segmented buttons show them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibTab {
    Liked = 0,
    Playlists = 1,
    Albums = 2,
    Artists = 3,
    History = 4,
}

impl LibTab {
    pub const ALL: [LibTab; 5] = [
        LibTab::Liked,
        LibTab::Playlists,
        LibTab::Albums,
        LibTab::Artists,
        LibTab::History,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LibTab::Liked => "Me gusta",
            LibTab::Playlists => "Listas",
            LibTab::Albums => "Álbumes",
            LibTab::Artists => "Artistas",
            LibTab::History => "Historial",
        }
    }

    pub fn from_index(index: usize) -> LibTab {
        Self::ALL.get(index).copied().unwrap_or(LibTab::Liked)
    }

    /// `library:listas`, used by `ATIC_PLAY_OPEN=library:<tab>`.
    pub fn from_name(name: &str) -> Option<LibTab> {
        let name = name.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|tab| {
            tab.label().to_ascii_lowercase() == name
                || match tab {
                    LibTab::Liked => matches!(name.as_str(), "liked" | "me-gusta" | "megusta"),
                    LibTab::Playlists => matches!(name.as_str(), "lists" | "playlists" | "listas"),
                    LibTab::Albums => matches!(name.as_str(), "albums" | "albumes" | "álbumes"),
                    LibTab::Artists => matches!(name.as_str(), "artists" | "artistas"),
                    LibTab::History => matches!(name.as_str(), "history" | "historial"),
                }
        })
    }
}

impl Default for LibTab {
    fn default() -> Self {
        LibTab::Liked
    }
}

/// Everything the library page needs, loaded lazily and kept.
pub struct Library {
    pub tab: LibTab,
    pub liked: Load<Arc<Vec<Track>>>,
    pub playlists: Load<Arc<Vec<PlaylistCard>>>,
    pub albums: Load<Arc<Vec<AlbumCard>>>,
    pub artists: Load<Arc<Vec<ArtistCard>>>,
    pub history: Load<Arc<Vec<Track>>>,
    /// One counter per tab, so five loads in flight do not invalidate each other.
    generations: [u64; 5],
}

impl Default for Library {
    fn default() -> Self {
        Self {
            tab: LibTab::Liked,
            liked: Load::Idle,
            playlists: Load::Idle,
            albums: Load::Idle,
            artists: Load::Idle,
            history: Load::Idle,
            generations: [0; 5],
        }
    }
}

impl Library {
    /// The state behind one tab, without its contents.
    fn state(&self, tab: LibTab) -> LoadState<'_> {
        match tab {
            LibTab::Liked => state_of(&self.liked),
            LibTab::Playlists => state_of(&self.playlists),
            LibTab::Albums => state_of(&self.albums),
            LibTab::Artists => state_of(&self.artists),
            LibTab::History => state_of(&self.history),
        }
    }
}

/// [`Load`] without the value, so the five lists can be asked in one match.
#[derive(Clone, Copy)]
enum LoadState<'a> {
    Idle,
    Loading,
    Ready,
    Failed(&'a str),
}

impl<'a> LoadState<'a> {
    fn is_idle(self) -> bool {
        matches!(self, LoadState::Idle)
    }

    fn is_settled(self) -> bool {
        !matches!(self, LoadState::Idle | LoadState::Loading)
    }

    fn message(self) -> Option<&'a str> {
        match self {
            LoadState::Failed(error) => Some(error),
            _ => None,
        }
    }
}

fn state_of<T>(load: &Load<T>) -> LoadState<'_> {
    match load {
        Load::Idle => LoadState::Idle,
        Load::Loading => LoadState::Loading,
        Load::Ready(_) => LoadState::Ready,
        Load::Failed(error) => LoadState::Failed(error.as_ref()),
    }
}

/// What one background load came back with.
enum Loaded {
    Liked(Vec<Track>),
    Playlists(Vec<PlaylistCard>),
    Albums(Vec<AlbumCard>),
    Artists(Vec<ArtistCard>),
    History(Vec<Track>),
}

impl Model {
    /// Opens the library on `tab`, starting the loads it still needs.
    pub fn open_library(&mut self, tab: LibTab, cx: &mut Context<Self>) {
        self.library.tab = tab;
        if self.view.kind() != ViewKind::Library {
            self.push_view(crate::model::View::Library, cx);
        }
        self.library_load(tab, cx);
        cx.notify();
    }

    pub fn library_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = LibTab::from_index(index);
        self.library.tab = tab;
        self.library_load(tab, cx);
        cx.notify();
    }

    /// Starts the load of `tab` when it has never run.
    fn library_load(&mut self, tab: LibTab, cx: &mut Context<Self>) {
        if !self.source.signed_in() {
            return;
        }
        if self.library.state(tab).is_idle() {
            self.library_request(tab, cx);
        }
    }

    /// Loads every list once, so the tabs and the hearts are ready before they are opened.
    pub fn library_load_all(&mut self, cx: &mut Context<Self>) {
        if !self.source.signed_in() {
            return;
        }
        for tab in LibTab::ALL {
            if self.library.state(tab).is_idle() {
                self.library_request(tab, cx);
            }
        }
    }

    /// Re-runs the load of one tab (the retry button).
    pub fn library_retry(&mut self, tab: LibTab, cx: &mut Context<Self>) {
        self.library_request(tab, cx);
    }

    fn library_request(&mut self, tab: LibTab, cx: &mut Context<Self>) {
        let slot = tab as usize;
        self.library.generations[slot] += 1;
        let generation = self.library.generations[slot];
        let source = self.source.clone();
        let label = tab.label();
        match tab {
            LibTab::Liked => self.library.liked = Load::Loading,
            LibTab::Playlists => self.library.playlists = Load::Loading,
            LibTab::Albums => self.library.albums = Load::Loading,
            LibTab::Artists => self.library.artists = Load::Loading,
            LibTab::History => self.library.history = Load::Loading,
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            // One list per task; the four others are already on their way by now.
            let result = cx
                .background_spawn(async move {
                    let result = match tab {
                        LibTab::Liked => source.liked_songs().map(Loaded::Liked),
                        LibTab::Playlists => source.library_playlists().map(Loaded::Playlists),
                        LibTab::Albums => source.library_albums().map(Loaded::Albums),
                        LibTab::Artists => source.library_artists().map(Loaded::Artists),
                        LibTab::History => source.history().map(Loaded::History),
                    };
                    match &result {
                        Ok(_) => log::info!("library «{label}»: loaded"),
                        Err(err) => log::warn!("library «{label}» failed: {err}"),
                    }
                    result
                })
                .await;
            this.update(cx, |model, cx| {
                if model.library.generations[slot] != generation {
                    return;
                }
                let error = match result {
                    Ok(Loaded::Liked(list)) => {
                        model.library.liked = Load::Ready(Arc::new(list));
                        None
                    }
                    Ok(Loaded::Playlists(list)) => {
                        model.library.playlists = Load::Ready(Arc::new(list));
                        None
                    }
                    Ok(Loaded::Albums(list)) => {
                        model.library.albums = Load::Ready(Arc::new(list));
                        None
                    }
                    Ok(Loaded::Artists(list)) => {
                        model.library.artists = Load::Ready(Arc::new(list));
                        None
                    }
                    Ok(Loaded::History(list)) => {
                        model.library.history = Load::Ready(Arc::new(list));
                        None
                    }
                    Err(err) => {
                        let shared: SharedString = err.clone().into();
                        match tab {
                            LibTab::Liked => model.library.liked = Load::Failed(shared),
                            LibTab::Playlists => model.library.playlists = Load::Failed(shared),
                            LibTab::Albums => model.library.albums = Load::Failed(shared),
                            LibTab::Artists => model.library.artists = Load::Failed(shared),
                            LibTab::History => model.library.history = Load::Failed(shared),
                        }
                        Some(err)
                    }
                };
                if let Some(error) = error {
                    model.session_error(&error, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Forgets every list: after a logout, or when the session expired.
    pub fn library_clear(&mut self) {
        self.library = Library::default();
    }

    /// The first error among the lists, if any failed.
    pub fn library_error(&self) -> Option<&str> {
        LibTab::ALL
            .into_iter()
            .find_map(|tab| self.library.state(tab).message())
    }

    /// Whether the list behind the open tab finished loading (or gave up).
    pub fn library_settled(&self) -> bool {
        self.library.state(self.library.tab).is_settled()
    }

    /// Whether `video_id` is in Me gusta, according to the list already loaded.
    pub fn is_liked(&self, video_id: &str) -> bool {
        self.library
            .liked
            .ready()
            .is_some_and(|list| list.iter().any(|track| track.id == video_id))
    }

    /// Marks the current song as liked (or not), the way the heart does.
    pub fn toggle_like(&mut self, track: Track, cx: &mut Context<Self>) {
        if !self.account.signed_in {
            self.say("Inicia sesión para dar «Me gusta»", cx);
            return;
        }
        let was_liked = self.is_liked(&track.id);
        let liked = !was_liked;
        self.apply_like(&track, liked);
        cx.notify();
        let source = self.source.clone();
        let id = track.id.clone();
        let label = track.title.clone();
        cx.spawn(async move |this, cx| {
            let request_id = id.clone();
            let result = cx
                .background_spawn(async move { source.set_liked(&request_id, liked) })
                .await;
            this.update(cx, |model, cx| {
                match result {
                    Ok(()) => log::info!("like {id} -> {liked}"),
                    Err(err) => {
                        log::warn!("like {id} failed: {err}");
                        // Optimistic: put it back the way it was.
                        model.apply_like(&track, was_liked);
                        if !model.session_error(&err, cx) {
                            model.say(
                                if liked {
                                    format!("No se pudo dar «Me gusta» a «{label}»")
                                } else {
                                    format!("No se pudo quitar «Me gusta» a «{label}»")
                                },
                                cx,
                            );
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The cache half of [`Model::toggle_like`]: no network, no waiting.
    fn apply_like(&mut self, track: &Track, liked: bool) {
        let Some(list) = self.library.liked.ready().map(|list| (**list).clone()) else {
            return;
        };
        let mut list = list;
        let present = list.iter().position(|t| t.id == track.id);
        match (liked, present) {
            (true, None) => list.insert(0, track.clone()),
            (false, Some(index)) => {
                list.remove(index);
            }
            _ => return,
        }
        self.library.liked = Load::Ready(Arc::new(list));
    }

    /// The heart of the current song, wherever it is drawn.
    pub fn toggle_like_current(&mut self, cx: &mut Context<Self>) {
        match self.current.clone() {
            Some(track) => self.toggle_like(track, cx),
            None => self.say("Todavía no hay nada sonando", cx),
        }
    }

    /// Adds `track` at the end of one of the account's playlists.
    pub fn add_to_playlist(&mut self, playlist_id: String, track: Track, cx: &mut Context<Self>) {
        let source = self.source.clone();
        let id = track.id.clone();
        let title = track.title.clone();
        self.say("Agregando a la lista…", cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { source.add_to_playlist(&playlist_id, &id) })
                .await;
            this.update(cx, |model, cx| {
                match result {
                    Ok(()) => {
                        model.dialogs.close(cx);
                        model.say(format!("«{title}» quedó en la lista"), cx);
                    }
                    Err(err) => {
                        log::warn!("add to playlist failed: {err}");
                        model.say("No se pudo agregar a la lista", cx);
                        model.session_error(&err, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Creates a private playlist with `track` in it, then reloads the lists.
    pub fn create_playlist_with(&mut self, title: String, track: Track, cx: &mut Context<Self>) {
        let title = title.trim().to_string();
        if title.is_empty() {
            self.say("La lista necesita un nombre", cx);
            return;
        }
        let source = self.source.clone();
        let id = track.id.clone();
        let label = title.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { source.create_playlist(&title, &id) })
                .await;
            this.update(cx, |model, cx| {
                match result {
                    Ok(_playlist_id) => {
                        model.dialogs.close(cx);
                        model.library_request(LibTab::Playlists, cx);
                        model.say(format!("Lista «{label}» creada"), cx);
                    }
                    Err(err) => {
                        log::warn!("create playlist failed: {err}");
                        model.say("No se pudo crear la lista", cx);
                        model.session_error(&err, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

// ---------------------------------------------------------------- the page

/// The body of the Biblioteca page. The rows and the cards come from [`Page`]: this only picks
/// which ones to show and what to do when a tab has nothing.
pub fn body(
    model: &Entity<Model>,
    page: &mut Page,
    s: &Scheme,
    cx: &mut Context<Page>,
) -> AnyElement {
    let (tab, dark, signed_in, titles) = {
        let model = model.read(cx);
        (
            model.library.tab,
            model.dark,
            model.account.signed_in,
            counts(&model.library),
        )
    };
    let s = *s;

    if !signed_in {
        return div()
            .flex()
            .flex_col()
            .px(px(20.))
            .pb(px(40.))
            .child(heading(&s, "Biblioteca", "Solo con una cuenta"))
            .child(hint(
                &s,
                "Inicia sesión para tener tus Me gusta, tus listas, tus álbumes y tu historial.",
            ))
            .into_any_element();
    }

    let tabs: Vec<Segment> = LibTab::ALL
        .into_iter()
        .zip(titles)
        .map(|(tab, count)| {
            Segment::label(if count.is_empty() {
                tab.label().to_string()
            } else {
                format!("{} · {count}", tab.label())
            })
        })
        .collect();
    let entity = model.clone();
    let switch = SegmentedButtons::new(
        "library-tabs",
        tabs,
        tab as usize,
    )
    .on_change(move |index, _, cx| {
        entity.update(cx, |model, cx| model.library_tab(index, cx));
    });

    let mut column = div()
        .flex()
        .flex_col()
        .px(px(20.))
        .pb(px(44.))
        .child(heading(&s, "Biblioteca", "Lo que guardó tu cuenta"))
        .child(switch)
        .child(div().h(px(18.)));

    let retry = {
        let entity = model.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut gpui::App| {
            entity.update(cx, |model, cx| model.library_retry(tab, cx));
        }
    };

    match tab {
        LibTab::Liked => {
            let state = model.read(cx).library.liked.clone();
            column = column.child(match state {
                Load::Idle | Load::Loading => loading_block(&s, "Cargando tus Me gusta…"),
                Load::Failed(err) => {
                    let retry = retry.clone();
                    error_block(&s, &err, "No se pudieron cargar tus Me gusta", retry)
                }
                Load::Ready(list) if list.is_empty() => hint(
                    &s,
                    "Todavía no le diste «Me gusta» a nada. El corazón de cada canción lo guarda aquí.",
                ),
                Load::Ready(list) => page.song_list(&s, &list, true, dark, cx),
            });
        }
        LibTab::Playlists => {
            let state = model.read(cx).library.playlists.clone();
            column = column.child(match state {
                Load::Idle | Load::Loading => loading_block(&s, "Cargando tus listas…"),
                Load::Failed(err) => {
                    let retry = retry.clone();
                    error_block(&s, &err, "No se pudieron cargar tus listas", retry)
                }
                Load::Ready(list) if list.is_empty() => hint(
                    &s,
                    "No tienes listas todavía. Puedes crear una desde el menú de cualquier canción.",
                ),
                Load::Ready(list) => page.playlist_grid(&s, &list, dark, cx),
            });
        }
        LibTab::Albums => {
            let state = model.read(cx).library.albums.clone();
            column = column.child(match state {
                Load::Idle | Load::Loading => loading_block(&s, "Cargando tus álbumes…"),
                Load::Failed(err) => {
                    let retry = retry.clone();
                    error_block(&s, &err, "No se pudieron cargar tus álbumes", retry)
                }
                Load::Ready(list) if list.is_empty() => {
                    hint(&s, "Todavía no guardaste ningún álbum.")
                }
                Load::Ready(list) => page.album_grid(&s, &list, dark, cx),
            });
        }
        LibTab::Artists => {
            let state = model.read(cx).library.artists.clone();
            column = column.child(match state {
                Load::Idle | Load::Loading => loading_block(&s, "Cargando tus artistas…"),
                Load::Failed(err) => {
                    let retry = retry.clone();
                    error_block(&s, &err, "No se pudieron cargar tus artistas", retry)
                }
                Load::Ready(list) if list.is_empty() => {
                    hint(&s, "Todavía no sigues a ningún artista.")
                }
                Load::Ready(list) => page.artist_grid(&s, &list, dark, cx),
            });
        }
        LibTab::History => {
            let state = model.read(cx).library.history.clone();
            column = column.child(match state {
                Load::Idle | Load::Loading => loading_block(&s, "Cargando tu historial…"),
                Load::Failed(err) => {
                    let retry = retry.clone();
                    error_block(&s, &err, "No se pudo cargar tu historial", retry)
                }
                Load::Ready(list) if list.is_empty() => {
                    hint(&s, "Todavía no hay nada en tu historial.")
                }
                Load::Ready(list) => page.song_list(&s, &list, true, dark, cx),
            });
        }
    }
    column.into_any_element()
}

/// How many items each tab holds, as text for the segmented buttons; empty while loading.
fn counts(library: &Library) -> [String; 5] {
    let liked = library.liked.ready().map(|list| list.len());
    let playlists = library.playlists.ready().map(|list| list.len());
    let albums = library.albums.ready().map(|list| list.len());
    let artists = library.artists.ready().map(|list| list.len());
    let history = library.history.ready().map(|list| list.len());
    [liked, playlists, albums, artists, history]
        .map(|count| count.map_or_else(String::new, |count| count.to_string()))
}

fn hint(s: &Scheme, text: &str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .py(px(64.))
        .w_full()
        .child(
            div()
                .type_emphasized(HEADLINE_SMALL)
                .text_color(s.on_surface)
                .child("Nada por aquí todavía"),
        )
        .child(
            div()
                .max_w(px(460.))
                .type_style(BODY_MEDIUM)
                .text_color(alpha(s.on_surface_variant, 0.9))
                .text_center()
                .child(text.to_string()),
        )
        .into_any_element()
}

/// The "Me gusta" heart, in the place each view needs it.
pub fn heart(
    model: &Entity<Model>,
    track: Option<&Track>,
    id: impl Into<SharedString>,
    size: f32,
    cx: &mut gpui::App,
) -> AnyElement {
    let (signed_in, liked) = {
        let model = model.read(cx);
        (
            model.account.signed_in,
            track.is_some_and(|track| model.is_liked(&track.id)),
        )
    };
    let track = track.cloned();
    let entity = model.clone();
    gpui_m3::IconButton::new(id.into(), icon(if liked { "heart-filled" } else { "heart" }))
        .size(px(size))
        .tooltip(if !signed_in {
            "Inicia sesión para dar Me gusta"
        } else if liked {
            "Quitar de Me gusta"
        } else {
            "Me gusta"
        })
        .when(liked, |button| button.tonal())
        .when(!signed_in, |button| button.disabled(true))
        .on_click(move |_, _, cx| {
            if let Some(track) = track.clone() {
                entity.update(cx, |model, cx| model.toggle_like(track, cx));
            }
        })
        .into_any_element()
}
