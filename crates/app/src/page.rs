//! The scrolling main column: home, search results, and the album, artist and playlist pages.
//!
//! Everything here is drawn from [`Model`] and [`CoverCache`]; nothing blocks. The view lives in
//! an `AnyView::cached` slot, so it redraws only when one of those two notifies.
//!
//! The column scrolls itself, with a smooth wheel, a thin overlay scrollbar that can be dragged,
//! and only the song rows near the viewport built for long lists.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, Context, Corners, ElementId, Entity,
    Focusable, FontWeight, ImageSource, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Render, ScrollDelta, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement, Styled, StyledImage, Subscription, Window, canvas,
    deferred, div, hsla, img, linear_color_stop, linear_gradient, px,
    prelude::FluentBuilder as _,
};
use gpui_m3::{
    Banner, Button, Carousel, CarouselItem, Elevation, Icon, IconButton, ListItem, LoadingIndicator,
    Scheme, Search, SearchBar, Segment, SegmentedButtons, TextFieldEvent, Theme, TopAppBar,
    TopAppBarSize, collapse_progress, collapse_range, interaction::{
        interactive, state_layer, use_interaction,
    }, motion::{
        self, MotionSettings, Spring, animate_color, animate_f32, entrance, key, presence, replay,
    }, search_field,
    theme::{
        alpha,
        type_scale::{
            BODY_MEDIUM, HEADLINE_SMALL, LABEL_MEDIUM, TITLE_LARGE, TypeScaleExt,
        },
    },
};

use crate::covers::{ArtSize, CoverCache};
use crate::icons::icon;
use crate::model::{Load, Model, View, ViewKind, length_text, total_duration};
use crate::motion::{CALM, CALM_SECS, calm_ease};
use crate::source::{AlbumCard, AlbumDetail, ArtistCard, Card, PlaylistCard, Shelf, Track, clock_text};

const PAD: f32 = 20.;
const TILE: f32 = 164.;
const COVER: f32 = 160.;
/// Air between two song rows: enough to tell them apart without a line that fights the hover.
const ROW_GAP: f32 = 2.;
/// A list longer than this draws only the rows near the viewport.
const VIRTUAL_FROM: usize = 28;
/// Rows left out of the count above and below the viewport, so a fast wheel never shows a gap.
const OVERSCAN: f32 = 3.;
/// The wheel settles in this fraction of the app's transition length (about a third of a second):
/// long enough to feel smooth, short enough that it never trails the fingers.
const SCROLL_SETTLE: f32 = 0.45;
/// One wheel notch, in pixels; a touchpad sends pixels and is applied straight.
const LINE_PX: f32 = 20.;

pub struct Page {
    model: Entity<Model>,
    covers: Entity<CoverCache>,
    search: Entity<Search>,
    /// Keeps the suggestion list in step with the search field's focus.
    focus_out: Option<Subscription>,
    /// Set while [`Page::preset_search`] fills the field, so the change it emits does not start a
    /// search of its own: the preset already decided what to do with the text.
    preset_typing: bool,
    /// Where the wheel wants the column, in content pixels.
    scroll: f32,
    /// What is drawn this frame: `scroll` once the wheel smoothing has caught up. The touchpad
    /// and the scrollbar thumb set both, so they follow the hand exactly.
    scroll_shown: f32,
    /// Motion clock at the previous frame, for the scroll smoothing.
    scroll_time: f32,
    scroll_generation: u64,
    /// Where the column was left in each view, so «Volver» lands where the user was.
    view_scroll: HashMap<PageKey, f32>,
    /// The view being drawn, and the navigation it belongs to.
    view_key: Option<PageKey>,
    nav_seen: u64,
    content_height: Rc<Cell<f32>>,
    area_height: Rc<Cell<f32>>,
    /// Top of the scrollable area in window coordinates, from the last paint.
    area_top: Rc<Cell<f32>>,
    /// Height of the content at the previous frame; a change means a different page is measured.
    last_content: f32,
    /// Height of a song row, measured once from a list drawn whole.
    row_h: Rc<Cell<f32>>,
    /// Last measured place of every long song list, for clipping it to the viewport.
    list_geom: Rc<RefCell<HashMap<&'static str, ListGeom>>>,
}

/// Identifies a view in [`Page::view_scroll`]. Search is one place whatever the query.
#[derive(Clone, PartialEq, Eq, Hash)]
enum PageKey {
    Home,
    Search,
    Library,
    Album(String),
    Artist(String),
    Playlist(String),
}

/// Where a song list sat in the window on the frame it was last measured.
#[derive(Clone, Copy, Default)]
struct ListGeom {
    generation: u64,
    /// Scroll shown when it was measured, so a scroll can be taken into account afterwards.
    shown: f32,
    /// Window y of the first row drawn, and its index in the list.
    y: f32,
    first: usize,
    /// How many rows the list had.
    rows: usize,
}

/// What a row should leave out because the page already says it.
#[derive(Clone, Copy, Default)]
struct RowHints<'a> {
    album: Option<&'a str>,
    artist: Option<&'a str>,
}

impl Page {
    pub fn new(model: Entity<Model>, covers: Entity<CoverCache>, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| {
            search_field(cx).placeholder("Busca canciones, álbumes o artistas")
        });
        cx.subscribe(&field, |page: &mut Self, _, event: &TextFieldEvent, cx| {
            match event {
                TextFieldEvent::Changed(text) => {
                    if page.preset_typing {
                        page.preset_typing = false;
                    } else {
                        page.model
                            .update(cx, |model, cx| model.set_query(text.clone(), cx));
                    }
                }
                TextFieldEvent::Submitted(text) => {
                    page.model
                        .update(cx, |model, cx| model.submit_query(text.clone(), cx));
                }
                TextFieldEvent::Cancelled => {
                    page.model.update(cx, |model, cx| model.close_suggestions(cx));
                }
            }
            cx.notify();
        })
        .detach();
        let search = cx.new(|cx| Search::new(field, cx));
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&covers, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            covers,
            search,
            focus_out: None,
            preset_typing: false,
            scroll: 0.,
            scroll_shown: 0.,
            scroll_time: 0.,
            scroll_generation: 0,
            view_scroll: HashMap::new(),
            view_key: None,
            nav_seen: 0,
            content_height: Rc::default(),
            area_height: Rc::default(),
            area_top: Rc::new(Cell::new(0.)),
            last_content: 0.,
            row_h: Rc::new(Cell::new(0.)),
            list_geom: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| model.go_search(cx));
        let field = self.search.read(cx).field().clone();
        field.read(cx).focus(window);
    }

    /// Development hook (`ATIC_PLAY_OPEN`): opens Buscar with `text` already in the field.
    ///
    /// With `suggestions` the suggestion panel is left up and the search is not run; without it
    /// the search runs right away, exactly as if Enter had been pressed.
    pub fn preset_search(&mut self, text: String, suggestions: bool, cx: &mut Context<Self>) {
        self.model.update(cx, |model, cx| model.go_search(cx));
        // Filling the field emits a change of its own; take it over so the preset, and not the
        // field, decides whether that text searches or only shows its suggestions.
        self.preset_typing = true;
        self.search
            .update(cx, |search, cx| search.set_text(text.clone(), cx));
        if suggestions {
            self.model
                .update(cx, |model, cx| model.preview_suggestions(text, cx));
        } else {
            self.model
                .update(cx, |model, cx| model.submit_query(text, cx));
        }
        cx.notify();
    }

    /// Back to the top, for when the user picks a rail destination.
    pub fn scroll_top(&mut self, cx: &mut Context<Self>) {
        self.scroll_generation += 1;
        self.scroll = 0.;
        self.scroll_shown = 0.;
        // The destination starts at the top from now on, even when the user comes back to it.
        let key = page_key(&self.model.read(cx).view);
        self.view_scroll.insert(key, 0.);
        cx.notify();
    }

    fn bar_expanded() -> f32 {
        f32::from(TopAppBarSize::Large.expanded_height(true))
    }

    fn max_scroll(&self) -> f32 {
        let expanded = Self::bar_expanded();
        let range = collapse_range(expanded);
        let progress = collapse_progress(self.scroll, expanded);
        let bar = expanded + (64. - expanded) * progress;
        let area_when_collapsed = self.area_height.get() + (bar - 64.);
        range + (self.content_height.get() - area_when_collapsed).max(0.)
    }

    fn cover(
        &mut self,
        url: Option<&str>,
        size: ArtSize,
        cx: &mut Context<Self>,
    ) -> Option<Arc<gpui::RenderImage>> {
        let peeked = { self.covers.read(cx).peek_size(url, size) };
        if peeked.is_some() {
            return peeked;
        }
        self.covers.update(cx, |covers, cx| {
            covers.image_size(url, size, cx);
        });
        None
    }

    /// Called when the scrollbar thumb is dragged: follows the hand, with no smoothing.
    fn scroll_to_progress(&mut self, progress: f32, max: f32, cx: &mut Context<Self>) {
        self.scroll = (progress * max).clamp(0., max);
        self.scroll_shown = self.scroll;
        cx.notify();
    }

    /// The rows of `slot` worth drawing this frame, from the geometry of the last paint.
    fn visible_rows(
        &self,
        slot: &'static str,
        count: usize,
        row_h: Option<f32>,
    ) -> std::ops::Range<usize> {
        let all = 0..count;
        if count <= VIRTUAL_FROM {
            return all;
        }
        let Some(row_h) = row_h else {
            return all;
        };
        let Some(geom) = self.list_geom.borrow().get(slot).copied() else {
            return all;
        };
        let (area_top, area_h) = (self.area_top.get(), self.area_height.get());
        if geom.generation != self.nav_seen || geom.rows != count || area_top < 1. || area_h <= 0. {
            return all;
        }
        // The rows move with the scroll: the last measured top, corrected by how far the column
        // moved since then. The measurement is of the first row drawn, not of row zero.
        let pitch = row_h + ROW_GAP;
        let top =
            geom.y + (geom.shown - self.scroll_shown) - geom.first as f32 * pitch;
        let margin = OVERSCAN * pitch;
        let first = ((area_top - margin - top) / pitch).floor().max(0.) as usize;
        let last = ((area_top + area_h + margin - top) / pitch).ceil().max(0.) as usize;
        let first = first.min(count);
        let last = last.clamp(first, count);
        if last - first >= count {
            all
        } else {
            first..last
        }
    }

    // -------------------------------------------------------- search bar

    fn search_block(&mut self, s: &Scheme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (suggestions, open) = {
            let model = self.model.read(cx);
            (model.suggestions.clone(), model.suggestions_open)
        };
        let search = self.search.clone();
        let model = self.model.clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (index, text) in suggestions.iter().take(6).enumerate() {
            let (search, model, text) = (search.clone(), model.clone(), text.clone());
            let picked = text.clone();
            rows.push(
                ListItem::new(("suggestion", index), text)
                    .leading(
                        Icon::new("search")
                            .size(px(18.))
                            .color(s.on_surface_variant),
                    )
                    .on_click(move |_, _, cx| {
                        // Choosing a suggestion fills the field and searches it right away.
                        search.update(cx, |search, cx| {
                            search.set_text(picked.clone(), cx)
                        });
                        model.update(cx, |model, cx| model.submit_query(picked.clone(), cx));
                    })
                    .into_any_element(),
            );
        }
        // A menu can be shorter than a panel: the same short spring brings it in and takes it
        // out, and the last rows stay while it leaves.
        let shown = presence(("page-suggest", 0usize), open && !rows.is_empty(), window, cx);
        let panel = shown.filter(|_| !rows.is_empty()).map(|p| {
            deferred(
                div()
                    .absolute()
                    .top(px(70. - 4. * (1. - p)))
                    .left_0()
                    .right_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p(px(8.))
                    .rounded(px(28.))
                    .bg(s.surface_container_high)
                    .opacity(p)
                    .shadow(Elevation::Level3.shadow(s))
                    .occlude()
                    .children(rows),
            )
            .with_priority(1)
        });
        div()
            .relative()
            .w_full()
            .max_w(px(720.))
            .pt(px(10.))
            // A press here belongs to the search: only clicks elsewhere put the list away.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(SearchBar::new("app-search-bar", &self.search).open_on_press(false))
            .children(panel)
            .into_any_element()
    }

    // -------------------------------------------------------- home

    fn home_body(&mut self, s: &Scheme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (home, dark, audio_error) = {
            let model = self.model.read(cx);
            (
                model.home.clone(),
                model.dark,
                model.audio_error().map(str::to_string),
            )
        };
        let mut column = div().flex().flex_col().px(px(PAD)).pb(px(PAD + 24.));
        column = column.child(self.search_block(s, window, cx));
        if let Some(error) = audio_error {
            log::warn!("audio: {error}");
            column = column.child(
                div().pt(px(8.)).child(
                    Banner::new("audio-banner")
                        .subtle()
                        .icon("alert")
                        .title("Sin audio")
                        .text("No se pudo abrir la salida de audio. Puedes explorar, pero no vas a escuchar nada."),
                ),
            );
        }
        match home {
            Load::Idle | Load::Loading => {
                // Shelves of the right shape, pulsing: the page keeps its layout while it loads.
                column = column.child(
                    div()
                        .pt(px(20.))
                        .text_size(px(13.5))
                        .text_color(s.on_surface_variant)
                        .child("Cargando tu música…"),
                );
                column = column
                    .child(skeleton_shelf(s, window, cx))
                    .child(skeleton_shelf(s, window, cx));
            }
            Load::Failed(err) => {
                let model = self.model.clone();
                column = column.child(error_block(
                    s,
                    &err,
                    "No se pudo cargar el inicio",
                    move |_, _, cx| model.update(cx, |model, cx| model.refresh_home(cx)),
                ));
            }
            Load::Ready(shelves) if shelves.is_empty() => {
                let model = self.model.clone();
                column = column.child(empty_block(
                    s,
                    "music",
                    "Nada por aquí todavía",
                    "YouTube Music no devolvió nada. Prueba de nuevo en un rato.",
                    "Reintentar",
                    move |_, _, cx| model.update(cx, |model, cx| model.refresh_home(cx)),
                ));
            }
            Load::Ready(shelves) => {
                for (index, shelf) in shelves.iter().enumerate() {
                    // The first shelves arrive one after the other, 110 ms apart.
                    let p = if index < 4 {
                        entrance(("shelf-in", index), CALM, index as f32 * 0.11, window, cx)
                    } else {
                        1.
                    };
                    column = column.child(
                        div()
                            .relative()
                            .top(px(8. * (1. - p)))
                            .opacity(p)
                            .child(heading(s, &shelf.title, ""))
                            .child(self.shelf(s, shelf, index, dark, window, cx)),
                    );
                }
                let mut songs: Vec<Track> = Vec::new();
                for shelf in shelves.iter() {
                    for card in &shelf.cards {
                        if let Card::Song(track) = card {
                            if !songs.iter().any(|t| t.id == track.id) {
                                songs.push(track.clone());
                            }
                        }
                    }
                }
                songs.truncate(10);
                if !songs.is_empty() {
                    column = column
                        .child(heading(s, "Canciones para ti", "Lo que encontró la radio"))
                        .child(self.song_list_in(
                            s,
                            &songs,
                            false,
                            "home",
                            RowHints::default(),
                            dark,
                            Some(window),
                            cx,
                        ));
                }
            }
        }
        column.into_any_element()
    }

    fn shelf(
        &mut self,
        s: &Scheme,
        shelf: &Shelf,
        index: usize,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cards = shelf.cards.clone();
        let scope = format!("shelf-{index}");
        let items: Vec<CarouselItem> = cards
            .iter()
            .map(|card| {
                CarouselItem::element(self.card_tile(s, card, dark, false, &scope, Some(window), cx))
                    .background(s.surface)
            })
            .collect();
        let model = self.model.clone();
        Carousel::uncontained(("shelf", index))
            .height(px(236.))
            .item_width(px(TILE + 20.))
            .items(items)
            .on_select(move |selected, _, cx| {
                if let Some(card) = cards.get(selected) {
                    open_card(&model, card, cx);
                }
            })
            .into_any_element()
    }

    fn card_tile(
        &mut self,
        s: &Scheme,
        card: &Card,
        dark: bool,
        interactive_card: bool,
        scope: &str,
        mut window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = ElementId::Name(SharedString::from(format!("card-{scope}-{}", card.key())));
        // Pages bring a window, so the cover can answer the pointer; the library draws its
        // grids through the same tile without one, and the card is then still.
        let state = window
            .as_deref_mut()
            .map(|window| use_interaction(&id, window, cx));
        let (hovered, pressed) = state.as_ref().map_or((false, false), |state| {
            let state = state.read(cx);
            (state.hovered, state.pressed)
        });
        let active = hovered || pressed;
        let round = matches!(card, Card::Artist(_));
        let size = if round { COVER - 16. } else { COVER };
        let base = if round { size / 2. } else { 16. };
        let radius = match window.as_deref_mut() {
            Some(window) => animate_f32(
                key(&id, "radius"),
                Spring::SPATIAL_FAST,
                if active { base + 8. } else { base },
                window,
                cx,
            ),
            None => base,
        };
        let scale = match window.as_deref_mut() {
            Some(window) => animate_f32(
                key(&id, "press"),
                Spring::SPATIAL_FAST,
                if pressed { 0.96 } else { 1. },
                window,
                cx,
            ),
            None => 1.,
        };
        let image = self.cover(card.thumbnail(), ArtSize::Large, cx);
        let shown = size * scale;
        let art = match window.as_deref_mut() {
            Some(window) => art_animated(
                &card.key(),
                image,
                shown,
                radius * scale,
                dark,
                window,
                cx,
            ),
            None => art(&card.key(), image, shown, radius * scale, dark),
        };
        // The hover layer scales with the art, so the whole cover reacts as one.
        let layer = state.as_ref().and_then(|state| {
            window.as_deref_mut().map(|window| {
                state_layer(
                    &id,
                    state,
                    Corners::all(px(radius * scale)),
                    s.on_surface,
                    window,
                    cx,
                )
                .into_any_element()
            })
        });
        let cover_box = div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(TILE))
            .h(px(size))
            .child(
                div()
                    .relative()
                    .size(px(shown))
                    .child(art)
                    .children(layer),
            );
        let title = card.title().to_string();
        let subtitle = card.subtitle();
        let mut tile = div()
            .id(id)
            .w(px(TILE))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(cover_box)
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight(500.))
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(s.on_surface_variant)
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(subtitle),
            );
        if let Some(state) = &state {
            tile = interactive(tile, state, true, false);
        }
        if interactive_card {
            let model = self.model.clone();
            let card = card.clone();
            tile.on_click(move |_, _, cx| open_card(&model, &card, cx))
                .into_any_element()
        } else {
            tile.into_any_element()
        }
    }

    // -------------------------------------------------------- song lists

    /// The rows of a list, as the library page asks for them: the same rows, without a window to
    /// fade the covers in.
    pub(crate) fn song_list(
        &mut self,
        s: &Scheme,
        tracks: &[Track],
        as_queue: bool,
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.song_list_in(
            s,
            tracks,
            as_queue,
            "library",
            RowHints::default(),
            dark,
            None,
            cx,
        )
    }

    fn song_list_in(
        &mut self,
        s: &Scheme,
        tracks: &[Track],
        as_queue: bool,
        slot: &'static str,
        hints: RowHints<'_>,
        dark: bool,
        mut window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (current, playing) = {
            let model = self.model.read(cx);
            (model.current_id().map(str::to_string), model.playing)
        };
        let shared = Arc::new(tracks.to_vec());
        let count = tracks.len();
        let row_h = {
            let measured = self.row_h.get();
            (measured > 1.).then_some(measured)
        };
        let range = self.visible_rows(slot, count, row_h);
        let mut rows = div().relative().flex().flex_col().gap(px(ROW_GAP));
        // The canvas sits on the rows themselves, so its bounds are the first row's top and the
        // height of all of them: that is where the next frame takes the geometry from.
        let geom = self.list_geom.clone();
        let probe_h = self.row_h.clone();
        let generation = self.nav_seen;
        let shown = self.scroll_shown;
        let whole = range.len() == count;
        let first_drawn = range.start;
        let probe = canvas(
            move |bounds, _, _| {
                let top = f32::from(bounds.origin.y);
                let height = f32::from(bounds.size.height);
                geom.borrow_mut().insert(
                    slot,
                    ListGeom {
                        generation,
                        shown,
                        y: top,
                        first: first_drawn,
                        rows: count,
                    },
                );
                if whole && probe_h.get() <= 1. && count >= VIRTUAL_FROM && height > 0. {
                    let row = (height - (count.saturating_sub(1)) as f32 * ROW_GAP) / count as f32;
                    if row > 8. && row < 200. {
                        probe_h.set(row);
                    }
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        rows = rows.child(probe);
        for index in range.clone() {
            rows = rows.child(self.song_row(
                s,
                &tracks[index],
                index,
                slot,
                as_queue,
                &shared,
                current.as_deref(),
                playing,
                hints,
                dark,
                window.as_deref_mut(),
                cx,
            ));
        }
        // Spacers keep the list exactly as tall as if every row were drawn, so the scrollbar and
        // the top bar collapse do not move when rows come in and out.
        let mut list = div()
            .flex()
            .flex_col()
            .p(px(8.))
            .rounded(px(28.))
            .bg(s.surface_container_low);
        if let Some(row_h) = row_h {
            if range.start > 0 {
                list = list.child(div().h(px(range.start as f32 * (row_h + ROW_GAP))));
            }
        }
        list = list.child(rows);
        if let Some(row_h) = row_h {
            if range.end < count {
                list = list.child(div().h(px((count - range.end) as f32 * (row_h + ROW_GAP))));
            }
        }
        list.into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn song_row(
        &mut self,
        s: &Scheme,
        track: &Track,
        index: usize,
        slot: &'static str,
        as_queue: bool,
        shared: &Arc<Vec<Track>>,
        current: Option<&str>,
        playing: bool,
        hints: RowHints<'_>,
        dark: bool,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let image = self.cover(track.thumbnail.as_deref(), ArtSize::Thumb, cx);
        let model = self.model.clone();
        let shared = shared.clone();
        let is_current = current == Some(track.id.as_str());
        let trailing = div()
            .flex()
            .items_center()
            .gap(px(14.))
            .when(is_current && playing, |d| {
                d.child(Icon::new(icon("waves")).size(px(18.)).color(s.primary))
            })
            .child(
                div()
                    .w(px(44.))
                    .text_size(px(12.5))
                    .text_color(s.on_surface_variant)
                    .text_right()
                    .child(if track.duration.is_some() {
                        clock_text(track.secs())
                    } else {
                        String::new()
                    }),
            );
        let art = match window {
            Some(window) => art_animated(
                &format!("{slot}-{index}-{}", track.id),
                image,
                44.,
                10.,
                dark,
                window,
                cx,
            ),
            None => art(
                &format!("{slot}-{index}-{}", track.id),
                image,
                44.,
                10.,
                dark,
            ),
        };
        ListItem::new((slot, index), track.title.clone())
            .subtitle(track_line(track, hints))
            .leading(art)
            .trailing(trailing)
            .on_click(move |_, _, cx| {
                model.update(cx, |model, cx| {
                    if as_queue {
                        model.play_list((*shared).clone(), index, cx);
                    } else {
                        model.play_now((*shared)[index].clone(), cx);
                    }
                })
            })
            .into_any_element()
    }

    // -------------------------------------------------------- search

    fn search_body(&mut self, s: &Scheme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (query, search, tab, dark) = {
            let model = self.model.read(cx);
            (
                model.query.clone(),
                model.search.clone(),
                model.search_tab,
                model.dark,
            )
        };
        let mut column = div()
            .flex()
            .flex_col()
            .px(px(PAD))
            .pb(px(PAD + 24.))
            .child(self.search_block(s, window, cx));
        if query.trim().is_empty() {
            return column
                .child(hint_block(
                    s,
                    "search",
                    "Busca en YouTube Music",
                    "Escribe el nombre de una canción, un álbum, un artista o una lista.",
                ))
                .into_any_element();
        }
        match search {
            Load::Idle | Load::Loading => {
                column = column.child(loading_block(s, "Buscando…"));
            }
            Load::Failed(err) => {
                let model = self.model.clone();
                let query2 = query.to_string();
                column = column.child(error_block(
                    s,
                    &err,
                    "No se pudo buscar",
                    move |_, _, cx| {
                        model.update(cx, |model, cx| model.submit_query(query2.clone(), cx))
                    },
                ));
            }
            Load::Ready(results) if results.is_empty() => {
                column = column.child(hint_block(
                    s,
                    "search",
                    "Sin resultados",
                    &format!(
                        "No encontré nada para «{}». Prueba con otras palabras.",
                        query.trim()
                    ),
                ));
            }
            Load::Ready(results) => {
                let counts = [
                    results.songs.len(),
                    results.albums.len(),
                    results.artists.len(),
                    results.playlists.len(),
                ];
                let labels = ["Canciones", "Álbumes", "Artistas", "Listas"];
                let segments: Vec<Segment> = labels
                    .into_iter()
                    .zip(counts)
                    .map(|(label, count)| Segment::label(format!("{label} · {count}")))
                    .collect();
                let model = self.model.clone();
                column = column.child(
                    div().pt(px(14.)).child(
                        SegmentedButtons::new("search-tabs", segments, tab.min(3)).on_change(
                            move |index, _, cx| {
                                model.update(cx, |model, cx| model.set_search_tab(index, cx))
                            },
                        ),
                    ),
                );
                column = column.child(div().h(px(16.)));
                match tab.min(3) {
                    0 => {
                        if results.songs.is_empty() {
                            column = column.child(no_results(s, "canciones"));
                        } else {
                            column = column.child(self.song_list_in(
                                s,
                                &results.songs,
                                false,
                                "search",
                                RowHints::default(),
                                dark,
                                Some(window),
                                cx,
                            ));
                        }
                    }
                    1 => {
                        if results.albums.is_empty() {
                            column = column.child(no_results(s, "álbumes"));
                        } else {
                            column = column.child(self.album_grid_in(
                                s,
                                &results.albums,
                                dark,
                                "search-albums",
                                Some(window),
                                cx,
                            ));
                        }
                    }
                    2 => {
                        if results.artists.is_empty() {
                            column = column.child(no_results(s, "artistas"));
                        } else {
                            column = column.child(self.artist_grid_in(
                                s,
                                &results.artists,
                                dark,
                                "search-artists",
                                Some(window),
                                cx,
                            ));
                        }
                    }
                    _ => {
                        if results.playlists.is_empty() {
                            column = column.child(no_results(s, "listas"));
                        } else {
                            column = column.child(self.playlist_grid_in(
                                s,
                                &results.playlists,
                                dark,
                                "search-lists",
                                Some(window),
                                cx,
                            ));
                        }
                    }
                }
            }
        }
        column.into_any_element()
    }

    // -------------------------------------------------------- album / artist / playlist

    fn album_body(
        &mut self,
        s: &Scheme,
        load: Load<Arc<AlbumDetail>>,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match load {
            Load::Idle | Load::Loading => loading_block(s, "Cargando el álbum…"),
            Load::Failed(err) => {
                let model = self.model.clone();
                error_block(s, &err, "No se pudo abrir el álbum", move |_, _, cx| {
                    model.update(cx, |model, cx| model.retry_view(cx))
                })
            }
            Load::Ready(detail) => {
                let image = self.cover(detail.album.thumbnail.as_deref(), ArtSize::Large, cx);
                let total = total_duration(&detail.tracks);
                let meta = match detail.album.year {
                    Some(year) => format!("{year} · {}", list_line(detail.tracks.len(), total)),
                    None => list_line(detail.tracks.len(), total),
                };
                let play = {
                    let model = self.model.clone();
                    let tracks = detail.tracks.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        model.update(cx, |model, cx| model.play_list(tracks.to_vec(), 0, cx))
                    }
                };
                let shuffle = {
                    let model = self.model.clone();
                    let tracks = detail.tracks.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        model.update(cx, |model, cx| model.play_shuffled(tracks.to_vec(), cx))
                    }
                };
                // The album's name is in the bar, where it stays when the bar collapses: the
                // header names the artist and the record, never the title again.
                let header = div()
                    .flex()
                    .items_end()
                    .gap(px(24.))
                    .pt(px(12.))
                    .child(art_animated(
                        &detail.album.id,
                        image,
                        208.,
                        24.,
                        dark,
                        window,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .pb(px(4.))
                            .child(label(s, "ÁLBUM"))
                            .child(
                                div()
                                    .type_style(BODY_MEDIUM)
                                    .text_color(s.on_surface_variant)
                                    .child(detail.album.artist.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(s.on_surface_variant)
                                    .child(meta),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .pt(px(12.))
                                    .child(Button::new("album-play", "Reproducir").filled().on_click(play))
                                    .child(
                                        Button::new("album-shuffle", "Aleatorio")
                                            .tonal()
                                            .on_click(shuffle),
                                    ),
                            ),
                    );
                div()
                    .flex()
                    .flex_col()
                    .px(px(PAD))
                    .pb(px(PAD + 24.))
                    .child(header)
                    .child(div().h(px(20.)))
                    .child(self.song_list_in(
                        s,
                        &detail.tracks,
                        true,
                        "album",
                        RowHints {
                            album: Some(&detail.album.title),
                            artist: None,
                        },
                        dark,
                        Some(window),
                        cx,
                    ))
                    .into_any_element()
            }
        }
    }

    fn artist_body(
        &mut self,
        s: &Scheme,
        load: Load<Arc<crate::source::ArtistDetail>>,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match load {
            Load::Idle | Load::Loading => loading_block(s, "Cargando el artista…"),
            Load::Failed(err) => {
                let model = self.model.clone();
                error_block(s, &err, "No se pudo abrir el artista", move |_, _, cx| {
                    model.update(cx, |model, cx| model.retry_view(cx))
                })
            }
            Load::Ready(detail) => {
                let image = self.cover(detail.artist.thumbnail.as_deref(), ArtSize::Large, cx);
                let songs = detail.top_songs.clone();
                let play = {
                    let model = self.model.clone();
                    let tracks = songs.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        model.update(cx, |model, cx| model.play_list(tracks.to_vec(), 0, cx))
                    }
                };
                let shuffle = {
                    let model = self.model.clone();
                    let tracks = songs.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        model.update(cx, |model, cx| model.play_shuffled(tracks.to_vec(), cx))
                    }
                };
                let mut meta = div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .pb(px(4.))
                    .child(label(s, "ARTISTA"))
                    .child(
                        div()
                            .type_style(BODY_MEDIUM)
                            .text_color(s.on_surface_variant)
                            .child(list_line(
                                detail.top_songs.len(),
                                total_duration(&detail.top_songs),
                            )),
                    );
                if !songs.is_empty() {
                    meta = meta.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .pt(px(12.))
                            .child(Button::new("artist-play", "Reproducir").filled().on_click(play))
                            .child(
                                Button::new("artist-shuffle", "Aleatorio")
                                    .tonal()
                                    .on_click(shuffle),
                            ),
                    );
                }
                let header = div()
                    .flex()
                    .items_center()
                    .gap(px(24.))
                    .pt(px(12.))
                    .child(art_animated(
                        &detail.artist.id,
                        image,
                        168.,
                        84.,
                        dark,
                        window,
                        cx,
                    ))
                    .child(meta);
                let mut column = div()
                    .flex()
                    .flex_col()
                    .px(px(PAD))
                    .pb(px(PAD + 24.))
                    .child(header);
                if !detail.top_songs.is_empty() {
                    column = column
                        .child(heading(s, "Canciones populares", "Lo más escuchado"))
                        .child(self.song_list_in(
                            s,
                            &detail.top_songs,
                            true,
                            "artist",
                            RowHints {
                                album: None,
                                artist: Some(&detail.artist.name),
                            },
                            dark,
                            Some(window),
                            cx,
                        ));
                }
                if !detail.albums.is_empty() {
                    column = column
                        .child(heading(s, "Álbumes", ""))
                        .child(self.album_grid_in(
                            s,
                            &detail.albums,
                            dark,
                            "artist-albums",
                            Some(window),
                            cx,
                        ));
                }
                column.into_any_element()
            }
        }
    }

    fn playlist_body(
        &mut self,
        s: &Scheme,
        title: String,
        load: Load<Arc<Vec<Track>>>,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match load {
            Load::Idle | Load::Loading => loading_block(s, "Cargando la lista…"),
            Load::Failed(err) => {
                let model = self.model.clone();
                error_block(s, &err, "No se pudo abrir la lista", move |_, _, cx| {
                    model.update(cx, |model, cx| model.retry_view(cx))
                })
            }
            Load::Ready(tracks) => {
                let cover_url = tracks.first().and_then(|t| t.thumbnail.clone());
                let image = self.cover(cover_url.as_deref(), ArtSize::Large, cx);
                let play = {
                    let model = self.model.clone();
                    let tracks = tracks.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        model.update(cx, |model, cx| model.play_list(tracks.to_vec(), 0, cx))
                    }
                };
                let shuffle = {
                    let model = self.model.clone();
                    let tracks = tracks.clone();
                    move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                        model.update(cx, |model, cx| model.play_shuffled(tracks.to_vec(), cx))
                    }
                };
                // The list's title lives in the bar; the header carries the rest.
                let header = div()
                    .flex()
                    .items_end()
                    .gap(px(24.))
                    .pt(px(12.))
                    .child(art_animated(&title, image, 200., 24., dark, window, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .pb(px(4.))
                            .child(label(s, "LISTA"))
                            .child(
                                div()
                                    .text_size(px(12.5))
                                    .text_color(s.on_surface_variant)
                                    .child(list_line(tracks.len(), total_duration(&tracks))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .pt(px(12.))
                                    .child(
                                        Button::new("playlist-play", "Reproducir")
                                            .filled()
                                            .on_click(play),
                                    )
                                    .child(
                                        Button::new("playlist-shuffle", "Aleatorio")
                                            .tonal()
                                            .on_click(shuffle),
                                    ),
                            ),
                    );
                div()
                    .flex()
                    .flex_col()
                    .px(px(PAD))
                    .pb(px(PAD + 24.))
                    .child(header)
                    .child(div().h(px(20.)))
                    .child(self.song_list_in(
                        s,
                        &tracks,
                        true,
                        "playlist",
                        RowHints::default(),
                        dark,
                        Some(window),
                        cx,
                    ))
                    .into_any_element()
            }
        }
    }

    // -------------------------------------------------------- grids

    /// Albums as cards, as the library page asks for them.
    pub(crate) fn album_grid(
        &mut self,
        s: &Scheme,
        albums: &[AlbumCard],
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.album_grid_in(s, albums, dark, "library-albums", None, cx)
    }

    fn album_grid_in(
        &mut self,
        s: &Scheme,
        albums: &[AlbumCard],
        dark: bool,
        scope: &str,
        mut window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut grid = div().flex().flex_wrap().gap(px(16.));
        for album in albums {
            grid = grid.child(self.card_tile(
                s,
                &Card::Album(album.clone()),
                dark,
                true,
                scope,
                window.as_deref_mut(),
                cx,
            ));
        }
        grid.into_any_element()
    }

    /// Artists as cards, as the library page asks for them.
    pub(crate) fn artist_grid(
        &mut self,
        s: &Scheme,
        artists: &[ArtistCard],
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.artist_grid_in(s, artists, dark, "library-artists", None, cx)
    }

    fn artist_grid_in(
        &mut self,
        s: &Scheme,
        artists: &[ArtistCard],
        dark: bool,
        scope: &str,
        mut window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut grid = div().flex().flex_wrap().gap(px(16.));
        for artist in artists {
            grid = grid.child(self.card_tile(
                s,
                &Card::Artist(artist.clone()),
                dark,
                true,
                scope,
                window.as_deref_mut(),
                cx,
            ));
        }
        grid.into_any_element()
    }

    /// Playlists as cards, as the library page asks for them.
    pub(crate) fn playlist_grid(
        &mut self,
        s: &Scheme,
        playlists: &[PlaylistCard],
        dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.playlist_grid_in(s, playlists, dark, "library-lists", None, cx)
    }

    fn playlist_grid_in(
        &mut self,
        s: &Scheme,
        playlists: &[PlaylistCard],
        dark: bool,
        scope: &str,
        mut window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut grid = div().flex().flex_wrap().gap(px(16.));
        for playlist in playlists {
            grid = grid.child(self.card_tile(
                s,
                &Card::Playlist(playlist.clone()),
                dark,
                true,
                scope,
                window.as_deref_mut(),
                cx,
            ));
        }
        grid.into_any_element()
    }

    // -------------------------------------------------------- scrollbar

    /// A thin overlay scrollbar: always visible, wider under the pointer, draggable anywhere on
    /// its track.
    fn scrollbar(&mut self, s: &Scheme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let (area_h, content) = (self.area_height.get(), self.content_height.get());
        let max = self.max_scroll().max(0.);
        if max < 1. || area_h < 60. || content <= area_h + 1. {
            return div().into_any_element();
        }
        let id = ElementId::Name(SharedString::from("page-scrollbar"));
        let state = use_interaction(&id, window, cx);
        let hovered = state.read(cx).hovered;
        let dragging = *window
            .use_keyed_state(key(&id, "drag"), cx, |_, _| false)
            .read(cx);
        let track_h = (area_h - 12.).max(40.);
        let thumb_h = (track_h * (area_h / content).min(1.)).clamp(32., track_h);
        let progress = (self.scroll_shown / max).clamp(0., 1.);
        let thumb_top = (track_h - thumb_h) * progress;
        let active = hovered || dragging;
        let width = animate_f32(
            key(&id, "width"),
            Spring::EFFECTS,
            if active { 7. } else { 4. },
            window,
            cx,
        );
        let tone = animate_color(
            key(&id, "tone"),
            Spring::EFFECTS,
            if active {
                alpha(s.on_surface, 0.55)
            } else {
                alpha(s.on_surface, 0.3)
            },
            window,
            cx,
        );

        // Dragging: the same window-wide listener gpui-m3's slider uses, so the thumb keeps
        // following once the pointer leaves the track.
        let page = cx.entity().downgrade();
        let drag = window.use_keyed_state(key(&id, "drag"), cx, |_, _| false);
        let grip = {
            let page = page.clone();
            let drag = drag.clone();
            canvas(
                |_, _, _| {},
                move |bounds: Bounds<gpui::Pixels>, _, window, _| {
                    let (down_page, down_drag) = (page.clone(), drag.clone());
                    window.on_mouse_event(
                        move |e: &MouseDownEvent, phase, _, cx| {
                            if phase.bubble()
                                && e.button == MouseButton::Left
                                && bounds.contains(&e.position)
                            {
                                down_drag.update(cx, |d, cx| {
                                    *d = true;
                                    cx.notify();
                                });
                                let p = ((f32::from(e.position.y)
                                    - f32::from(bounds.origin.y))
                                    / track_h)
                                    .clamp(0., 1.);
                                down_page
                                    .update(cx, |page, cx| {
                                        page.scroll_to_progress(p, max, cx)
                                    })
                                    .ok();
                            }
                        },
                    );
                    let (move_page, move_drag) = (page.clone(), drag.clone());
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                        if phase.bubble()
                            && e.pressed_button.is_some()
                            && *move_drag.read(cx)
                        {
                            let p = ((f32::from(e.position.y) - f32::from(bounds.origin.y))
                                / track_h)
                                .clamp(0., 1.);
                            move_page
                                .update(cx, |page, cx| page.scroll_to_progress(p, max, cx))
                                .ok();
                        }
                    });
                    let up_drag = drag.clone();
                    window.on_mouse_event(move |_: &MouseUpEvent, _, _, cx| {
                        if *up_drag.read(cx) {
                            up_drag.update(cx, |d, cx| {
                                *d = false;
                                cx.notify();
                            });
                        }
                    });
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full()
        };

        let track = interactive(
            div()
                .id(id)
                .absolute()
                .top(px(6.))
                .right(px(2.))
                .w(px(12.))
                .h(px(track_h))
                .rounded(px(6.))
                .child(
                    div()
                        .absolute()
                        .top(px(thumb_top))
                        .left(px((12. - width) / 2.))
                        .w(px(width))
                        .h(px(thumb_h))
                        .rounded(px(4.))
                        .bg(tone),
                )
                .child(grip),
            &state,
            true,
            false,
        );
        track.into_any_element()
    }

    // -------------------------------------------------------- top bar

    fn bar(&mut self, progress: f32, cx: &mut Context<Self>) -> AnyElement {
        let (kind, title, subtitle, can_back) = {
            let model = self.model.read(cx);
            let (title, subtitle) = match &model.view {
                View::Home => ("Atic Play".to_string(), "Tu música, sin vueltas".to_string()),
                View::Search => (
                    "Buscar".to_string(),
                    if model.query.trim().is_empty() {
                        "Canciones, álbumes y artistas".to_string()
                    } else {
                        format!("«{}»", model.query.trim())
                    },
                ),
                View::Album { load, .. } => match load.ready() {
                    Some(detail) => (detail.album.title.clone(), detail.album.artist.clone()),
                    None => ("Álbum".into(), String::new()),
                },
                View::Artist { load, .. } => match load.ready() {
                    Some(detail) => (detail.artist.name.clone(), "Artista".to_string()),
                    None => ("Artista".into(), String::new()),
                },
                View::Playlist { title, .. } => (title.clone(), String::new()),
                // The library draws its own heading while it is expanded; the bar repeats the
                // name once the heading has scrolled away.
                View::Library => (
                    if progress > 0.5 {
                        "Biblioteca".to_string()
                    } else {
                        String::new()
                    },
                    String::new(),
                ),
            };
            (model.view.kind(), title, subtitle, model.can_go_back())
        };

        let back = can_back.then(|| {
            let model = self.model.clone();
            IconButton::new("bar-back", "arrow-left")
                .size(px(40.))
                .tooltip("Volver")
                .on_click(move |_, _, cx| model.update(cx, |model, cx| model.go_back(cx)))
        });
        let mut bar = TopAppBar::new("page-bar")
            .large()
            .title(title)
            .subtitle(subtitle)
            .collapse(progress);
        if let Some(back) = back {
            bar = bar.leading(back);
        }
        if kind != ViewKind::Search {
            let page = cx.entity().downgrade();
            bar = bar.trailing(
                IconButton::new("bar-search", "search")
                    .size(px(40.))
                    .tooltip("Buscar")
                    .on_click(move |_, window, cx| {
                        page.update(cx, |page, cx| page.open_search(window, cx)).ok();
                    }),
            );
        }
        if kind == ViewKind::Home {
            let model = self.model.clone();
            let source = self.model.read(cx).source_name();
            bar = bar.trailing(
                IconButton::new("bar-refresh", "refresh")
                    .size(px(40.))
                    .tooltip(format!("Actualizar desde {source}"))
                    .on_click(move |_, _, cx| {
                        model.update(cx, |model, cx| model.refresh_home(cx))
                    }),
            );
        }
        bar.into_any_element()
    }
}

impl Render for Page {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_out.is_none() {
            let field = self.search.read(cx).field().clone();
            let handle = field.read(cx).focus_handle(cx);
            let page = cx.entity().downgrade();
            self.focus_out = Some(window.on_focus_out(&handle, cx, move |_, _, cx| {
                page.update(cx, |page, cx| {
                    page.model.update(cx, |model, cx| model.close_suggestions(cx));
                })
                .ok();
            }));
        }
        let s = *Theme::of(cx);
        let generation = self.model.read(cx).nav_generation();
        let (kind, view_key) = {
            let model = self.model.read(cx);
            (model.view.kind(), page_key(&model.view))
        };

        // Leaving a view, remember where the column was; entering one, go back to the top or to
        // where «Volver» left it.
        if generation != self.nav_seen || self.view_key.as_ref() != Some(&view_key) {
            if let Some(left) = self.view_key.clone() {
                // A long session should not remember every page it ever opened.
                if self.view_scroll.len() > 64 {
                    self.view_scroll.clear();
                }
                self.view_scroll.insert(left, self.scroll);
            }
            self.nav_seen = generation;
            self.scroll = self.view_scroll.get(&view_key).copied().unwrap_or(0.);
            self.scroll_shown = self.scroll;
            self.view_key = Some(view_key);
            self.list_geom.borrow_mut().clear();
        }

        // A page that is measured shorter than the one before pulls the scroll back inside it.
        let content = self.content_height.get();
        if (content - self.last_content).abs() > 0.5 {
            self.last_content = content;
            let max = self.max_scroll().max(0.);
            self.scroll = self.scroll.clamp(0., max);
            self.scroll_shown = self.scroll_shown.clamp(0., max);
        }

        // The wheel is smoothed against the motion clock; the touchpad and the scrollbar set the
        // drawn value themselves.
        let now = motion::clock(cx);
        let dt = if self.scroll_time > 0. {
            (now - self.scroll_time).clamp(0., 0.05)
        } else {
            0.
        };
        self.scroll_time = now;
        let diff = self.scroll - self.scroll_shown;
        if MotionSettings::get(cx).reduced || diff.abs() < 0.05 {
            self.scroll_shown = self.scroll;
        } else {
            let tau = CALM_SECS * SCROLL_SETTLE / 3.;
            let k = 1. - (-dt / tau).exp();
            self.scroll_shown += diff * k;
            window.request_animation_frame();
        }
        let shown = self.scroll_shown;

        let expanded = Self::bar_expanded();
        let progress = collapse_progress(shown, expanded);
        let row_offset = (shown - collapse_range(expanded)).max(0.);
        let bar = self.bar(progress, cx);

        // The new page rises 8 px while it fades in, on the app's 620 ms rhythm.
        let transition = replay(("page-enter", 0usize), generation, CALM_SECS, window, cx);
        let enter = match transition {
            Some(t) => calm_ease()(t / CALM_SECS),
            None => 1.,
        };

        let body = match kind {
            ViewKind::Home => self.home_body(&s, window, cx),
            ViewKind::Search => self.search_body(&s, window, cx),
            ViewKind::Library => {
                let model = self.model.clone();
                crate::library::body(&model, self, &s, cx)
            }
            ViewKind::Album => {
                let load = {
                    let model = self.model.read(cx);
                    match &model.view {
                        View::Album { load, .. } => load.clone(),
                        _ => Load::Idle,
                    }
                };
                let dark = self.model.read(cx).dark;
                self.album_body(&s, load, dark, window, cx)
            }
            ViewKind::Artist => {
                let load = {
                    let model = self.model.read(cx);
                    match &model.view {
                        View::Artist { load, .. } => load.clone(),
                        _ => Load::Idle,
                    }
                };
                let dark = self.model.read(cx).dark;
                self.artist_body(&s, load, dark, window, cx)
            }
            ViewKind::Playlist => {
                let (title, load) = {
                    let model = self.model.read(cx);
                    match &model.view {
                        View::Playlist { title, load, .. } => (title.clone(), load.clone()),
                        _ => (String::new(), Load::Idle),
                    }
                };
                let dark = self.model.read(cx).dark;
                self.playlist_body(&s, title, load, dark, window, cx)
            }
        };

        let (content_height, area_height, area_top) = (
            self.content_height.clone(),
            self.area_height.clone(),
            self.area_top.clone(),
        );
        let measure_content = canvas(
            move |bounds, _, _| content_height.set(f32::from(bounds.size.height)),
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let measure_area = canvas(
            move |bounds, _, _| {
                area_top.set(f32::from(bounds.origin.y));
                area_height.set(f32::from(bounds.size.height));
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let scrollbar = self.scrollbar(&s, window, cx);

        div()
            .id("page")
            .size_full()
            .flex()
            .flex_col()
            .bg(s.surface)
            .text_color(s.on_surface)
            .on_scroll_wheel(cx.listener(|page, event: &ScrollWheelEvent, _, cx| {
                let max = page.max_scroll().max(0.);
                match event.delta {
                    // A touchpad (or a precision wheel) is followed exactly: no smoothing on top
                    // of the one the device already does.
                    ScrollDelta::Pixels(delta) => {
                        page.scroll = (page.scroll - f32::from(delta.y)).clamp(0., max);
                        page.scroll_shown = page.scroll;
                    }
                    // A wheel notch asks for a jump; the render smooths it.
                    ScrollDelta::Lines(delta) => {
                        page.scroll = (page.scroll - delta.y * LINE_PX).clamp(0., max);
                    }
                }
                page.scroll_generation += 1;
                cx.notify();
            }))
            // A press anywhere but the search itself puts the suggestion list away.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|page, _: &gpui::MouseDownEvent, _, cx| {
                    page.model.update(cx, |model, cx| model.close_suggestions(cx));
                }),
            )
            .child(bar)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(measure_area)
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(-row_offset + 8. * (1. - enter)))
                            .opacity(enter)
                            .child(measure_content)
                            .child(body),
                    )
                    .child(scrollbar),
            )
    }
}

// ------------------------------------------------------------ shared pieces

/// A cover, or a coloured stand-in that follows the theme until the real one arrives.
pub(crate) fn art(
    key: &str,
    image: Option<Arc<gpui::RenderImage>>,
    size: f32,
    radius: f32,
    dark: bool,
) -> AnyElement {
    art_frame(key, image, size, radius, dark, 1.)
}

/// The same cover, fading in when the image arrives. Used by the page, which has a window to
/// animate in.
fn art_animated(
    key: &str,
    image: Option<Arc<gpui::RenderImage>>,
    size: f32,
    radius: f32,
    dark: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let fade = if image.is_some() {
        entrance(
            ElementId::Name(SharedString::from(format!("art|{key}|{size:.0}"))),
            Spring::EFFECTS_SLOW,
            0.,
            window,
            cx,
        )
    } else {
        0.
    };
    art_frame(key, image, size, radius, dark, fade)
}

/// Placeholder, image and the 1 px edge that keeps black and white covers apart from the page.
fn art_frame(
    key: &str,
    image: Option<Arc<gpui::RenderImage>>,
    size: f32,
    radius: f32,
    dark: bool,
    fade: f32,
) -> AnyElement {
    let mut frame = div()
        .relative()
        .flex_none()
        .size(px(size))
        .rounded(px(radius));
    match image {
        Some(image) => {
            frame = frame.child(
                img(ImageSource::Render(image))
                    .size(px(size))
                    .rounded(px(radius))
                    .object_fit(ObjectFit::Cover)
                    .opacity(fade),
            );
        }
        None => {
            let hue = hue_of(key);
            let (from, to, icon_color) = if dark {
                (
                    hsla(hue, 0.42, 0.32, 1.),
                    hsla((hue + 0.08).fract(), 0.5, 0.18, 1.),
                    hsla(hue, 0.4, 0.8, 0.5),
                )
            } else {
                (
                    hsla(hue, 0.5, 0.86, 1.),
                    hsla((hue + 0.08).fract(), 0.45, 0.64, 1.),
                    hsla(hue, 0.35, 0.32, 0.45),
                )
            };
            frame = frame
                .bg(linear_gradient(
                    140.,
                    linear_color_stop(from, 0.),
                    linear_color_stop(to, 1.),
                ))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(icon("music"))
                        .size(px(size * 0.3))
                        .color(icon_color),
                );
        }
    }
    // Pure black or white at 10 %, never a tinted neutral: an edge, not a colour.
    frame.child(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .rounded(px(radius))
            .border_1()
            .border_color(if dark {
                hsla(0., 0., 1., 0.1)
            } else {
                hsla(0., 0., 0., 0.1)
            }),
    )
    .into_any_element()
}

fn hue_of(key: &str) -> f32 {
    let mut hash: u32 = 2166136261;
    for byte in key.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    (hash % 360) as f32 / 360.
}

fn page_key(view: &View) -> PageKey {
    match view {
        View::Home => PageKey::Home,
        View::Search => PageKey::Search,
        View::Library => PageKey::Library,
        View::Album { id, .. } => PageKey::Album(id.clone()),
        View::Artist { id, .. } => PageKey::Artist(id.clone()),
        View::Playlist { id, .. } => PageKey::Playlist(id.clone()),
    }
}

/// "Artist · Album" as the row needs it: no stray separator, nothing that repeats the page it is
/// on or the song's own title.
fn track_line(track: &Track, hints: RowHints<'_>) -> String {
    let artist = track.artist.trim();
    let artist = if Some(artist) == hints.artist {
        ""
    } else {
        artist
    };
    let album = track
        .album
        .as_deref()
        .map(str::trim)
        .filter(|album| !album.is_empty())
        .filter(|album| *album != track.title.trim() && Some(*album) != hints.album);
    match (artist.is_empty(), album) {
        (true, None) => String::new(),
        (true, Some(album)) => album.to_string(),
        (false, None) => artist.to_string(),
        (false, Some(album)) => format!("{artist} · {album}"),
    }
}

/// "14 canciones · 43 min".
fn list_line(count: usize, total: std::time::Duration) -> String {
    let songs = match count {
        0 => "sin canciones".to_string(),
        1 => "1 canción".to_string(),
        n => format!("{n} canciones"),
    };
    match length_text(total) {
        length if length.is_empty() => songs,
        length => format!("{songs} · {length}"),
    }
}

fn open_card(model: &Entity<Model>, card: &Card, cx: &mut App) {
    match card {
        Card::Song(track) => {
            let track = track.clone();
            model.update(cx, |model, cx| model.play_now(track, cx));
        }
        Card::Album(album) => {
            let id = album.id.clone();
            model.update(cx, |model, cx| model.open_album(id, cx));
        }
        Card::Artist(artist) => {
            let id = artist.id.clone();
            model.update(cx, |model, cx| model.open_artist(id, cx));
        }
        Card::Playlist(playlist) => {
            let (id, title) = (playlist.id.clone(), playlist.title.clone());
            model.update(cx, |model, cx| model.open_playlist(id, title, cx));
        }
    }
}

/// A section title with an optional line under it, shared with the library page.
pub(crate) fn heading(s: &Scheme, title: &str, subtitle: &str) -> AnyElement {
    let mut block = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .pt(px(26.))
        .pb(px(12.))
        .child(div().type_emphasized(TITLE_LARGE).child(title.to_string()));
    if !subtitle.is_empty() {
        block = block.child(
            div()
                .type_style(BODY_MEDIUM)
                .text_color(s.on_surface_variant)
                .child(subtitle.to_string()),
        );
    }
    block.into_any_element()
}

fn label(s: &Scheme, text: &str) -> AnyElement {
    div()
        .type_style(LABEL_MEDIUM)
        .text_color(s.on_surface_variant)
        .child(text.to_string())
        .into_any_element()
}

/// A shelf of the right shape, pulsing while the real one loads.
fn skeleton_shelf(s: &Scheme, window: &mut Window, cx: &mut App) -> AnyElement {
    let pulse = motion::pulse(1.6, window, cx);
    let mut tiles = div().flex().gap(px(20.)).overflow_hidden();
    for _ in 0..6 {
        tiles = tiles.child(
            div()
                .flex_none()
                .w(px(TILE))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .size(px(COVER))
                        .rounded(px(16.))
                        .bg(s.surface_container_high)
                        .opacity(pulse),
                )
                .child(
                    div()
                        .w(px(TILE * 0.7))
                        .h(px(13.))
                        .rounded(px(6.))
                        .bg(s.surface_container_high)
                        .opacity(pulse * 0.8),
                ),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .pt(px(22.))
        .child(
            div()
                .w(px(190.))
                .h(px(22.))
                .rounded(px(8.))
                .bg(s.surface_container_high)
                .opacity(pulse),
        )
        .child(tiles)
        .into_any_element()
}

/// The spinner-and-label block for a page that is still loading, shared with the library page.
pub(crate) fn loading_block(s: &Scheme, text: &str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(14.))
        .h(px(240.))
        .w_full()
        .child(LoadingIndicator::new())
        .child(
            div()
                .text_size(px(13.5))
                .text_color(s.on_surface_variant)
                .child(text.to_string()),
        )
        .into_any_element()
}

fn no_results(s: &Scheme, what: &str) -> AnyElement {
    div()
        .py(px(40.))
        .text_size(px(13.5))
        .text_color(s.on_surface_variant)
        .child(format!("No hay {what} para esta búsqueda."))
        .into_any_element()
}

fn empty_block(
    s: &Scheme,
    icon_name: &str,
    title: &str,
    text: &str,
    action_label: &str,
    action: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .py(px(64.))
        .w_full()
        .child(
            Icon::new(icon_name.to_string())
                .size(px(40.))
                .color(alpha(s.on_surface_variant, 0.7)),
        )
        .child(
            div()
                .type_emphasized(HEADLINE_SMALL)
                .text_color(s.on_surface)
                .child(title.to_string()),
        )
        .child(
            div()
                .max_w(px(460.))
                .text_size(px(13.5))
                .text_color(s.on_surface_variant)
                .child(text.to_string()),
        )
        .child(
            div().pt(px(8.)).child(
                Button::new("empty-action", action_label.to_string())
                    .tonal()
                    .on_click(action),
            ),
        )
        .into_any_element()
}

fn hint_block(s: &Scheme, icon_name: &str, title: &str, text: &str) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .py(px(72.))
        .w_full()
        .child(
            Icon::new(icon_name.to_string())
                .size(px(40.))
                .color(alpha(s.on_surface_variant, 0.7)),
        )
        .child(
            div()
                .type_emphasized(HEADLINE_SMALL)
                .text_color(s.on_surface)
                .child(title.to_string()),
        )
        .child(
            div()
                .max_w(px(460.))
                .text_size(px(13.5))
                .text_color(s.on_surface_variant)
                .child(text.to_string()),
        )
        .into_any_element()
}

/// A failed load, with a retry: the layer's technical detail becomes a human sentence.
pub(crate) fn error_block(
    s: &Scheme,
    error: &str,
    title: &str,
    retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .py(px(64.))
        .w_full()
        .child(
            Icon::new("alert")
                .size(px(36.))
                .color(s.error),
        )
        .child(
            div()
                .type_emphasized(HEADLINE_SMALL)
                .text_color(s.on_surface)
                .child(title.to_string()),
        )
        .child(
            div()
                .max_w(px(520.))
                .text_size(px(13.))
                .text_color(s.on_surface_variant)
                .child(human_error(error)),
        )
        .child(
            div()
                .pt(px(8.))
                .child(Button::new("error-retry", "Reintentar").filled().on_click(retry)),
        )
        .into_any_element()
}

/// One of a few human sentences instead of the layer's technical string; the detail goes to the
/// log, where it is useful.
fn human_error(error: &str) -> String {
    log::warn!("page: {error}");
    let detail = error.to_lowercase();
    let offline = [
        "connect",
        "dns",
        "network",
        "timed out",
        "timeout",
        "offline",
        "unreachable",
        "reset",
        "eof",
    ];
    if offline.iter().any(|needle| detail.contains(needle)) {
        "Parece que no hay conexión. Revisa tu red y vuelve a intentarlo.".to_string()
    } else if detail.contains("http") || detail.contains("status") {
        "YouTube Music no respondió. Prueba de nuevo en un momento.".to_string()
    } else {
        "No se pudo cargar esta página. Prueba de nuevo en un momento.".to_string()
    }
}
