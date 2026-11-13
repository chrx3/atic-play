//! Atic Play: a fast, comfortable desktop music player built with GPUI and gpui-m3.
//!
//! The shell owns the rail, the page, the queue sheet and the playing bar; all the state lives
//! in [`Model`].
//!
//! The account lives in the system keyring ([`account`]): the rail shows its photo, and the
//! Biblioteca page ([`library`]) shows Me gusta, listas, álbumes, artistas and historial. The
//! theme, the volume and where the window was are remembered in `settings.json` ([`settings`]),
//! the media panel of the system follows what plays ([`media_controls`]), and the keyboard is
//! [`shortcuts`] (Espacio, Ctrl+F, Alt+←, Ctrl+← →, Ctrl+↑ ↓, Ctrl+L, Ctrl+/).
//!
//! `ATIC_PLAY_SOURCE=fake` swaps the YouTube Music client for a built-in catalog, so the app can
//! be opened and clicked without the network (or while `play-ytmusic` is still in progress).
//! `ATIC_PLAY_ANONYMOUS=1` starts the real client without the saved session, to look at the
//! signed-out UI without touching the account.
//!
//! For design reviews the window can also be opened straight into a state, with no clicks. All of
//! these are optional; the app behaves as usual when they are not set:
//!
//! - `ATIC_PLAY_THEME=light|dark` picks the starting colour scheme (`M3_MODE=dark` still works as
//!   a fallback). Without either, the theme saved in the settings file wins, and failing that the
//!   one Windows is using.
//! - `ATIC_PLAY_SIZE=<ancho>x<alto>` sets the starting window size, in logical pixels
//!   (`ATIC_PLAY_SIZE=800x600` for a small window; 360 px is the smallest side).
//! - `ATIC_PLAY_OPEN=<estado>` goes straight to one of these:
//!   - `home` (the default): the start page.
//!   - `library[:pestaña]`: Biblioteca, on Me gusta (the default), Listas, Álbumes, Artistas or
//!     Historial.
//!   - `search:<texto>`: Buscar, with the text already searched and the results showing.
//!   - `suggest:<texto>`: Buscar, with the text typed and the suggestion panel open.
//!   - `empty-search:<texto>`: a search that finds nothing, for the empty state.
//!   - `album`: the first album of the start page, or of the first song when it has none.
//!   - `artist`: the first artist of the start page, or of a search for the first song's artist.
//!   - `playlist`: the first playlist of the start page.
//!   - `now`: the first song of the start page (or of its first playlist), paused, with the
//!     "Reproduciendo" sheet open.
//!   - `lyrics`: like `now`, on the Letra tab.
//!   - `queue`: like `now`, paused, with the queue sheet open instead.
//!
//! `ATIC_PLAY_OPEN` never starts playback and leaves the volume alone: the states that need the
//! catalog wait for it in the background, so the window still opens right away.

mod account;
mod audio;
mod covers;
mod dev;
mod dialogs;
mod icons;
mod library;
mod logging;
mod media_controls;
mod model;
mod motion;
mod page;
mod player;
mod queue;
mod settings;
mod shortcuts;
mod source;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyView, App, AppContext as _, Application, Bounds, Context, Entity, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Point, Render, StyleRefinement, Styled,
    Subscription, TitlebarOptions, Window, WindowAppearance, WindowBounds, WindowOptions,
    canvas, div, point, prelude::FluentBuilder as _, px, size,
};
use gpui_m3::{
    Icon, IconButton, MotionSettings, RailItem, Shape, ShapeName, Theme, ThemeReveal, Toast,
    Toolbar, ToolbarItem,
    motion::animate_f32,
    theme_reveal_at,
};

use icons::{AppAssets, icon};
use library::LibTab;
use media_controls::{MediaCommand, MediaControls, Now};
use model::{Model, ViewKind};
use page::Page;
use player::{MiniPlayer, NowPlaying};
use queue::QueuePanel;
use shortcuts::{
    FocusSearch, GoBack, NextTrack, PreviousTrack, ShowShortcuts, ToggleLike, TogglePlay, VolumeDown,
    VolumeUp,
};
use source::Track;

/// Height of the band under the page that holds the playing bar.
const BAND: f32 = 96.;

/// Switches the mode with the circular reveal growing from `origin`, and remembers the choice.
fn set_dark(
    model: &Entity<Model>,
    dark: bool,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    settings::save_theme(dark);
    let scheme = model.update(cx, |model, cx| {
        model.set_dark(dark, cx);
        model.scheme()
    });
    if let Some(hwnd) = window_handle(window) {
        set_dark_titlebar(hwnd, dark);
    }
    theme_reveal_at(scheme, dark, origin, window, cx);
}

pub struct Shell {
    model: Entity<Model>,
    page: Entity<Page>,
    queue: Entity<QueuePanel>,
    mini: Entity<MiniPlayer>,
    now: Entity<NowPlaying>,
    /// Center of the theme button, where the reveal starts.
    toggle_origin: Rc<Cell<Point<Pixels>>>,
    /// The system media panel (Windows).
    media: MediaControls,
    /// Last window bounds seen while drawing; the ticker saves them once they stop moving.
    bounds_seen: Rc<Cell<Option<[f32; 4]>>>,
    bounds_saved: Option<[f32; 4]>,
    bounds_stable: u32,
    /// `ATIC_PLAY_SIZE` owns the window while it is set: a design review does not overwrite what
    /// the user had.
    saves_window: bool,
    volume_saved: f32,
    /// Keeps the theme in step with the system appearance while there is no saved preference.
    _appearance: Option<Subscription>,
}

impl Shell {
    fn new(model: Entity<Model>, cx: &mut Context<Self>) -> Self {
        let covers = model.read(cx).covers.clone();
        let clock = model.read(cx).clock.clone();
        let page = cx.new(|cx| Page::new(model.clone(), covers.clone(), cx));
        let queue = cx.new(|cx| QueuePanel::new(model.clone(), covers.clone(), cx));
        let mini = cx.new(|cx| MiniPlayer::new(model.clone(), clock.clone(), covers.clone(), cx));
        let now = cx.new(|cx| NowPlaying::new(model.clone(), clock.clone(), covers, cx));
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let volume = model.read(cx).volume;
        let shell = Self {
            model: model.clone(),
            page,
            queue,
            mini,
            now,
            toggle_origin: Rc::new(Cell::new(point(px(40.), px(600.)))),
            media: MediaControls::new(),
            bounds_seen: Rc::default(),
            bounds_saved: None,
            bounds_stable: 0,
            saves_window: std::env::var("ATIC_PLAY_SIZE").is_err(),
            volume_saved: volume,
            _appearance: None,
        };
        shell.start_ticking(cx);
        shell
    }

    /// Moves the clock and reacts to the engine, a little more than ten times a second.
    fn start_ticking(&self, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let shell = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
                let alive = cx
                    .update(|cx| model.update(cx, |model, cx| model.tick(cx)))
                    .is_ok();
                if !alive {
                    break;
                }
                let alive = cx
                    .update(|cx| shell.update(cx, |shell, cx| shell.tick_extras(cx)))
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();
    }

    /// The slow bookkeeping of the tick: the media panel, the session, and the preferences.
    fn tick_extras(&mut self, cx: &mut Context<Self>) {
        // Buttons of the media panel and the media keys of the keyboard and the headset.
        let commands = self.media.drain();
        if !commands.is_empty() {
            self.model.update(cx, |model, cx| {
                for command in commands {
                    match command {
                        MediaCommand::Play => {
                            if !model.playing {
                                model.toggle_play(cx);
                            }
                        }
                        MediaCommand::Pause | MediaCommand::Stop => {
                            if model.playing {
                                model.toggle_play(cx);
                            }
                        }
                        MediaCommand::Next => model.next(cx),
                        MediaCommand::Previous => model.previous(cx),
                        MediaCommand::Seek(position) => model.seek(position.as_secs_f32(), cx),
                    }
                }
            });
        }

        // A session that died under us flips the app back to the public catalog.
        let model = self.model.clone();
        let _ = model.update(cx, |model, cx| model.check_session(cx));

        // What the system panel shows. It follows the song, the state and (slowly) the position.
        let now = {
            let model = self.model.read(cx);
            let (position, duration) = {
                let clock = model.clock.read(cx);
                (clock.position, clock.duration)
            };
            Now::of(
                model.current.as_ref(),
                model.playing,
                Duration::from_secs_f32(position.max(0.)),
                Duration::from_secs_f32(duration.max(0.)),
            )
        };
        self.media.update(&now);

        // The volume: written once it settled, not on every pixel of the drag.
        let volume = self.model.read(cx).volume;
        if (volume - self.volume_saved).abs() >= 1. {
            self.volume_saved = volume;
            settings::save_volume(volume);
        }

        // The window: written once it stopped moving (see `Render`).
        let seen = self.bounds_seen.get();
        if seen != self.bounds_saved {
            self.bounds_stable += 1;
            if self.bounds_stable >= 20 {
                self.bounds_saved = seen;
                self.bounds_stable = 0;
                if let Some([x, y, width, height]) = seen
                    && self.saves_window
                {
                    settings::save_window(settings::WindowPrefs {
                        x,
                        y,
                        width,
                        height,
                        maximized: false,
                    });
                }
            }
        } else {
            self.bounds_stable = 0;
        }
    }

    /// Connects the system media panel to the window, once it exists.
    fn attach_media(&mut self, hwnd: isize) {
        self.media.attach(hwnd);
        self.media.invalidate();
        log::info!("media panel: {}", if self.media.attached() { "on" } else { "off" });
    }

    /// Follows the system appearance, while the theme is not the user's choice.
    fn follow_system(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).dark == dark {
            return;
        }
        let origin = point(
            window.viewport_size().width / 2.,
            window.viewport_size().height / 2.,
        );
        set_dark(&self.model.clone(), dark, origin, window, cx);
    }

    /// `ATIC_PLAY_OPEN`: walks to a state once the data it needs has arrived, without a click.
    fn apply_open(&self, open: dev::Open, cx: &mut Context<Self>) {
        if open == dev::Open::Home {
            return;
        }
        let (model, page, now) = (self.model.clone(), self.page.clone(), self.now.clone());
        let queue_sheet = open == dev::Open::Queue;
        let lyrics_tab = open == dev::Open::Lyrics;
        let wanted = format!("{open:?}");
        cx.spawn(async move |_, cx| match open {
            dev::Open::Home => {}
            dev::Open::Library(tab) => {
                // With a session the library loads on its own; the wait is only for the account.
                wait_for(&model, cx, 10, |model| model.account.signed_in).await;
                let _ = model.update(cx, |model, cx| model.open_library(tab, cx));
                wait_for(&model, cx, 30, Model::library_settled).await;
            }
            dev::Open::Search(text) => {
                let _ = page.update(cx, |page, cx| page.preset_search(text, false, cx));
            }
            dev::Open::Suggest(text) => {
                let _ = page.update(cx, |page, cx| page.preset_search(text, true, cx));
            }
            dev::Open::EmptySearch(text) => {
                let _ = page.update(cx, |page, cx| page.preset_search(text, false, cx));
            }
            dev::Open::Album => {
                if !wait_for_home(&model, cx).await {
                    log::warn!("open {wanted}: the start page did not load");
                    return;
                }
                let from_home = model
                    .update(cx, |model, _| model.first_home_album())
                    .unwrap_or(None);
                let id = if let Some(album) = from_home {
                    Some(album.id)
                } else {
                    // The shelves had no albums: the first song's album is the next best.
                    let Some(track) = seed_track(&model, cx).await else {
                        log::warn!("open {wanted}: no song to name an album");
                        return;
                    };
                    if track.album_id.is_some() {
                        track.album_id
                    } else {
                        let text = track.album.unwrap_or(track.artist);
                        if !search_for(&model, &text, cx).await {
                            log::warn!("open {wanted}: the album search did not come back");
                            return;
                        }
                        model
                            .update(cx, |model, _| model.first_search_album())
                            .unwrap_or(None)
                            .map(|album| album.id)
                    }
                };
                let Some(id) = id else {
                    log::warn!("open {wanted}: no album to open");
                    return;
                };
                let _ = model.update(cx, |model, cx| model.open_album(id, cx));
                wait_for(&model, cx, 30, Model::view_settled).await;
            }
            dev::Open::Artist => {
                if !wait_for_home(&model, cx).await {
                    log::warn!("open {wanted}: the start page did not load");
                    return;
                }
                let from_home = model
                    .update(cx, |model, _| model.first_home_artist())
                    .unwrap_or(None);
                let id = if let Some(artist) = from_home {
                    Some(artist.id)
                } else {
                    // The shelves had no artists: a search for the first song's artist.
                    let Some(track) = seed_track(&model, cx).await else {
                        log::warn!("open {wanted}: no song to name an artist");
                        return;
                    };
                    if track.artist.trim().is_empty() {
                        None
                    } else if !search_for(&model, &track.artist, cx).await {
                        log::warn!("open {wanted}: the artist search did not come back");
                        return;
                    } else {
                        model
                            .update(cx, |model, _| model.first_search_artist())
                            .unwrap_or(None)
                            .map(|artist| artist.id)
                    }
                };
                let Some(id) = id else {
                    log::warn!("open {wanted}: no artist to open");
                    return;
                };
                let _ = model.update(cx, |model, cx| model.open_artist(id, cx));
                wait_for(&model, cx, 30, Model::view_settled).await;
            }
            dev::Open::Playlist => {
                if !wait_for_home(&model, cx).await {
                    log::warn!("open {wanted}: the start page did not load");
                    return;
                }
                let Some(playlist) = model
                    .update(cx, |model, _| model.first_home_playlist())
                    .unwrap_or(None)
                else {
                    log::warn!("open {wanted}: the start page has no playlists");
                    return;
                };
                let _ = model.update(cx, |model, cx| {
                    model.open_playlist(playlist.id, playlist.title, cx)
                });
                wait_for(&model, cx, 30, Model::view_settled).await;
            }
            dev::Open::Now | dev::Open::Lyrics | dev::Open::Queue => {
                if !wait_for_home(&model, cx).await {
                    log::warn!("open {wanted}: the start page did not load");
                    return;
                }
                let Some(track) = seed_track(&model, cx).await else {
                    log::warn!("open {wanted}: the start page has no songs");
                    return;
                };
                let _ = model.update(cx, |model, cx| {
                    model.open_paused(track, cx);
                    model.now_open = !queue_sheet;
                    model.queue_open = queue_sheet;
                    cx.notify();
                });
                if lyrics_tab {
                    let _ = now.update(cx, |now, cx| now.show_lyrics(cx));
                }
                // The queue comes from the radio: wait for it so the sheet is not empty.
                if !wait_for(&model, cx, 15, |model| !model.queue.is_empty()).await {
                    log::warn!("open {wanted}: the radio queue is still empty");
                }
            }
        })
        .detach();
    }

    fn rail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let (kind, dark, signed_in) = {
            let model = self.model.read(cx);
            (model.view.kind(), model.dark, model.account.signed_in)
        };
        let model = self.model.clone();
        let page = self.page.clone();

        let logo = div().pt(px(20.)).pb(px(12.)).child(
            Shape::new(ShapeName::Cookie9).size(px(44.)).color(s.primary).child(
                Icon::new(icon("music"))
                    .size(px(22.))
                    .color(s.on_primary),
            ),
        );
        let home = RailItem::new("rail-home", icon("home"), "Inicio")
            .selected(kind == ViewKind::Home)
            .on_click({
                let model = model.clone();
                let page = page.clone();
                move |_, _, cx| {
                    model.update(cx, |model, cx| model.go_home(cx));
                    page.update(cx, |page, cx| page.scroll_top(cx));
                }
            });
        let search = RailItem::new("rail-search", "search", "Buscar")
            .selected(kind == ViewKind::Search)
            .on_click({
                let page = page.clone();
                move |_, window, cx| {
                    page.update(cx, |page, cx| page.open_search(window, cx));
                }
            });
        // The library only exists with an account: it is the account's music.
        let library = signed_in.then(|| {
            let model = model.clone();
            let page = page.clone();
            RailItem::new("rail-library", icon("library"), "Biblioteca")
                .selected(kind == ViewKind::Library)
                .on_click(move |_, _, cx| {
                    model.update(cx, |model, cx| model.open_library(LibTab::Liked, cx));
                    page.update(cx, |page, cx| page.scroll_top(cx));
                })
        });

        let origin = self.toggle_origin.clone();
        let toggle = div()
            .relative()
            .child(
                IconButton::new("rail-theme", if dark { "sun" } else { "moon" })
                    .tonal()
                    .size(px(44.))
                    .tooltip(if dark { "Tema claro" } else { "Tema oscuro" })
                    .on_click({
                        let model = model.clone();
                        move |_, window, cx| {
                            let origin = window.mouse_position();
                            set_dark(&model, !dark, origin, window, cx);
                        }
                    }),
            )
            .child(
                canvas(
                    move |bounds, _, _| origin.set(bounds.center()),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );

        let avatar = account::rail_button(&model, cx);

        div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .w(px(80.))
            .h_full()
            .child(logo)
            .child(home)
            .child(search)
            .children(library)
            .child(div().flex_1())
            .child(avatar)
            .child(toggle)
            .child(div().flex_none().w_full().h(px(BAND)))
    }

    fn bottom_band(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (queue_open, now_open) = {
            let model = self.model.read(cx);
            (model.queue_open, model.now_open)
        };
        let model = self.model.clone();
        let toolbar = Toolbar::floating()
            .item(
                ToolbarItem::new("tool-queue", icon("queue"))
                    .selected(queue_open)
                    .tooltip("Cola")
                    .on_click(model::click(&model, |model, cx| {
                        model.queue_open = !model.queue_open;
                        cx.notify();
                    })),
            )
            .item(
                ToolbarItem::new("tool-now", icon("lyrics"))
                    .selected(now_open)
                    .tooltip("Reproduciendo y letra")
                    .on_click(model::click(&model, |model, cx| {
                        model.now_open = true;
                        cx.notify();
                    })),
            );
        div()
            .flex()
            .flex_none()
            .items_start()
            .gap(px(12.))
            .w_full()
            .h(px(BAND))
            .pt(px(8.))
            .pr(px(16.))
            .child(div().flex_1().min_w_0().child(self.mini.clone()))
            .child(div().mt(px(4.)).child(toolbar))
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui_m3::motion::clock(cx);
        // What the window is doing right now, for the settings file (saved by the ticker).
        let bounds = window.bounds();
        self.bounds_seen.set(Some([
            bounds.origin.x.into(),
            bounds.origin.y.into(),
            bounds.size.width.into(),
            bounds.size.height.into(),
        ]));

        let s = *Theme::of(cx);
        let (queue_open, now_open, toast) = {
            let model = self.model.read(cx);
            (model.queue_open, model.now_open, model.toast.clone())
        };
        let model = self.model.clone();

        let width = animate_f32(
            "queue-width",
            crate::motion::CALM,
            if queue_open { queue::WIDTH + 8. } else { 0. },
            window,
            cx,
        )
        .max(0.);
        let page = div().relative().flex_1().min_w_0().h_full().child(
            div().absolute().top_0().left_0().size_full().child(
                AnyView::from(self.page.clone()).cached(StyleRefinement::default().size_full()),
            ),
        );
        let sheet = div()
            .flex_none()
            .w(px(width))
            .h_full()
            .overflow_hidden()
            .child(
                div()
                    .w(px(queue::WIDTH + 8.))
                    .h_full()
                    .pr(px(8.))
                    .pb(px(8.))
                    .child(
                        AnyView::from(self.queue.clone())
                            .cached(StyleRefinement::default().size_full()),
                    ),
            );

        // The sheet leaves at the same calm pace it arrives with; `presence` alone closes in
        // about 150 ms.
        let now_calm = animate_f32(
            "now-calm",
            crate::motion::CALM,
            if now_open { 1. } else { 0. },
            window,
            cx,
        );
        let now_p = (now_open || now_calm > 0.01).then_some(now_calm.clamp(0., 1.));
        let close_now = model::click(&model, |model, cx| {
            model.now_open = false;
            cx.notify();
        });
        // Any press outside the search itself puts the suggestion list away. The search block
        // stops the event, so it never gets here.
        let dismiss_suggestions = {
            let model = model.clone();
            move |_: &gpui::MouseDownEvent, _: &mut Window, cx: &mut App| {
                model.update(cx, |model, cx| model.close_suggestions(cx));
            }
        };

        // ------------------------------------------------------------ keyboard
        let key_page = self.page.clone();
        let key_model = self.model.clone();
        let key_model2 = self.model.clone();
        let key_model3 = self.model.clone();
        let key_model4 = self.model.clone();
        let key_model5 = self.model.clone();
        let key_model6 = self.model.clone();
        let key_model7 = self.model.clone();
        let key_model8 = self.model.clone();

        let banner = account::banner(&model, &s, cx);
        let account_dialog = account::dialog(&model, cx);
        let dialog = dialogs::render(&model, cx);

        div()
            .id("shell")
            .relative()
            .size_full()
            .flex()
            .bg(s.surface)
            .text_color(s.on_surface)
            .font_family(gpui_m3::theme::FONT_FAMILY)
            .key_context(shortcuts::CONTEXT)
            // ------------------------------------------------------------ shortcuts
            .on_action(cx.listener(move |_, _: &TogglePlay, _, cx| {
                key_model.update(cx, |model, cx| model.toggle_play(cx));
            }))
            .on_action(cx.listener(move |_, _: &FocusSearch, window, cx| {
                key_page.update(cx, |page, cx| page.open_search(window, cx));
            }))
            .on_action(cx.listener(move |_, _: &GoBack, _, cx| {
                key_model2.update(cx, |model, cx| model.go_back(cx));
            }))
            .on_action(cx.listener(move |_, _: &NextTrack, _, cx| {
                key_model3.update(cx, |model, cx| model.next(cx));
            }))
            .on_action(cx.listener(move |_, _: &PreviousTrack, _, cx| {
                key_model4.update(cx, |model, cx| model.previous(cx));
            }))
            .on_action(cx.listener(move |_, _: &VolumeUp, _, cx| {
                key_model5.update(cx, |model, cx| {
                    let volume = (model.volume + 5.).min(100.);
                    model.set_volume(volume, cx);
                });
            }))
            .on_action(cx.listener(move |_, _: &VolumeDown, _, cx| {
                key_model6.update(cx, |model, cx| {
                    let volume = (model.volume - 5.).max(0.);
                    model.set_volume(volume, cx);
                });
            }))
            .on_action(cx.listener(move |_, _: &ToggleLike, _, cx| {
                key_model7.update(cx, |model, cx| model.toggle_like_current(cx));
            }))
            .on_action(cx.listener(move |_, _: &ShowShortcuts, _, cx| {
                key_model8.update(cx, |model, cx| {
                    model.dialogs.shortcuts = true;
                    cx.notify();
                });
            }))
            .on_mouse_down(MouseButton::Left, dismiss_suggestions)
            .child(self.rail(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(div().flex_1().min_h_0().flex().child(page).child(sheet))
                    .child(self.bottom_band(cx)),
            )
            .when_some(banner, |d, banner| {
                d.child(
                    div()
                        .absolute()
                        .top(px(6.))
                        .left(px(80.))
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(banner),
                )
            })
            .when_some(now_p, |d, p| {
                d.child(
                    gpui_m3::BottomSheet::modal("now-sheet")
                        .spring(crate::motion::CALM)
                        .max_width(px(1000.))
                        .padding(px(24.))
                        .exit(p)
                        .on_dismiss(close_now)
                        .child(self.now.clone()),
                )
            })
            .when_some(toast, |d, (text, generation)| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(BAND + 8.))
                        .flex()
                        .justify_center()
                        .child(Toast::new(("toast", generation), text)),
                )
            })
            .when_some(account_dialog, |d, dialog| d.child(dialog))
            .when_some(dialog, |d, dialog| d.child(dialog))
            .child(ThemeReveal::new())
    }
}

/// Waits until the shelves are in, keeping the UI alive while they load.
async fn wait_for_home(model: &Entity<Model>, cx: &mut gpui::AsyncApp) -> bool {
    wait_for(model, cx, 30, Model::home_settled).await
}

/// The first song the start page can offer: a song card, or else the first song of the first
/// playlist (the real start page is all playlists for now).
async fn seed_track(model: &Entity<Model>, cx: &mut gpui::AsyncApp) -> Option<Track> {
    if let Some(track) = model
        .update(cx, |model, _| model.first_home_song())
        .unwrap_or(None)
    {
        return Some(track);
    }
    let playlist = model
        .update(cx, |model, _| model.first_home_playlist())
        .unwrap_or(None)?;
    let source = model
        .update(cx, |model, _| model.source.clone())
        .ok()?;
    let list = cx
        .background_spawn(async move { source.playlist(&playlist.id) })
        .await;
    let list = match list {
        Ok(list) => list,
        Err(err) => {
            log::warn!("seed from «{}» failed: {err}", playlist.title);
            return None;
        }
    };
    log::info!("seeded from the playlist «{}»", playlist.title);
    list.into_iter().next()
}

/// Searches `text` like pressing Enter, then waits for the results. `false` when they do not
/// come back in time.
async fn search_for(model: &Entity<Model>, text: &str, cx: &mut gpui::AsyncApp) -> bool {
    let text = text.trim().to_string();
    if text.is_empty() {
        return false;
    }
    let _ = model.update(cx, |model, cx| model.submit_query(text, cx));
    wait_for(model, cx, 30, Model::search_settled).await
}

/// Polls `check` every 50 ms, for up to `seconds`, and tells whether it ever held.
async fn wait_for(
    model: &Entity<Model>,
    cx: &mut gpui::AsyncApp,
    seconds: u64,
    check: impl Fn(&Model) -> bool,
) -> bool {
    for _ in 0..seconds * 20 {
        if model.update(cx, |model, _| check(model)).unwrap_or(false) {
            return true;
        }
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
    false
}

fn main() {
    let log_file = logging::init();
    log::info!(
        "atic-play {} starting on {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    match &log_file {
        Some(path) => log::info!("log file: {}", path.display()),
        None => log::warn!("could not open the log file: logging to stderr only"),
    }
    match settings::path().exists() {
        true => log::info!("settings: {}", settings::path().display()),
        false => log::info!("settings: none yet at {}", settings::path().display()),
    }
    Application::new().with_assets(AppAssets).run(|cx: &mut App| {
        gpui_m3::init(cx);
        // Windows' "Animation effects" is the app's motion switch; M3_REDUCED=1 forces it on.
        if settings::reduced_motion() {
            log::info!("motion: reduced");
            cx.set_global(MotionSettings {
                time_scale: 1.,
                reduced: true,
            });
        }
        shortcuts::install(cx);
        let source = source::from_env();
        log::info!("music source: {}", source.name());
        // The window: what was saved, or ATIC_PLAY_SIZE for a design review, or the default.
        let (window_size, min_size) = match dev::size() {
            Some((width, height)) => (
                size(px(width), px(height)),
                size(px(width.min(980.)), px(height.min(640.))),
            ),
            None => (size(px(1280.), px(800.)), size(px(980.), px(640.))),
        };
        let saved = settings::window();
        let bounds = match (saved, dev::size()) {
            (_, Some((width, height))) => Bounds::centered(None, size(px(width), px(height)), cx),
            (Some(saved), None) => Bounds::new(
                point(px(saved.x), px(saved.y)),
                size(px(saved.width), px(saved.height)),
            ),
            (None, None) => Bounds::centered(None, window_size, cx),
        };
        let window_bounds = match saved {
            Some(saved) if saved.maximized && dev::size().is_none() => {
                WindowBounds::Maximized(bounds)
            }
            _ => WindowBounds::Windowed(bounds),
        };
        let volume = settings::volume();
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(window_bounds),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Atic Play".into()),
                        ..Default::default()
                    }),
                    app_id: Some("atic-play".into()),
                    window_min_size: Some(min_size),
                    ..Default::default()
                },
                move |window, cx| {
                    // The theme follows Windows until the user picks one.
                    let system_dark = matches!(
                        window.appearance(),
                        WindowAppearance::Dark | WindowAppearance::VibrantDark
                    );
                    dev::set_system_dark(system_dark);
                    let model = cx.new(|cx| Model::new(source, cx));
                    if let Some(volume) = volume {
                        model.update(cx, |model, cx| model.set_volume(volume, cx));
                    }
                    let dark = model.read(cx).dark;
                    Theme::set_scheme(model.read(cx).scheme(), dark, cx);
                    let shell = cx.new(|cx| Shell::new(model, cx));
                    // The media panel belongs to the window: it is attached as soon as there is
                    // one, and from then on the media keys and the panel drive the app.
                    if let Some(hwnd) = window_handle(window) {
                        shell.update(cx, |shell, _| shell.attach_media(hwnd));
                        set_dark_titlebar(hwnd, dark);
                    }
                    // Windows changes its mind about light and dark while the app is open.
                    let weak = shell.downgrade();
                    let appearance = window.observe_window_appearance(move |window, cx| {
                        let dark = matches!(
                            window.appearance(),
                            WindowAppearance::Dark | WindowAppearance::VibrantDark
                        );
                        dev::set_system_dark(dark);
                        if dev::theme_follows_system() {
                            weak.update(cx, |shell, cx| shell.follow_system(dark, window, cx))
                                .ok();
                        }
                    });
                    shell.update(cx, |shell, _| shell._appearance = Some(appearance));
                    let _ = shell.update(cx, |shell, cx| shell.apply_open(dev::open(), cx));
                    shell
                },
            )
            .expect("no se pudo abrir la ventana");
        handle
            .update(cx, |_, window, cx| {
                set_window_icon(window);
                cx.activate(true);
            })
            .expect("la ventana se cerró al abrirse");
    });
}

/// The raw window handle, for the icon, the title bar colour and the media panel.
#[cfg(target_os = "windows")]
fn window_handle(window: &Window) -> Option<isize> {
    use raw_window_handle::RawWindowHandle;
    let handle = raw_window_handle::HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get() as isize),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn window_handle(_window: &Window) -> Option<isize> {
    None
}

/// Puts our icon in the title bar and the taskbar. GPUI 0.2 has no API for it, so this goes to
/// the Windows message loop directly.
#[cfg(target_os = "windows")]
fn set_window_icon(window: &Window) {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, ICON_BIG, ICON_SMALL, LR_DEFAULTCOLOR, SendMessageW,
        WM_SETICON,
    };

    const ICON: &[u8] = include_bytes!("../assets/brand/atic-play.ico");

    let Ok(handle) = raw_window_handle::HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = win32.hwnd.get() as *mut core::ffi::c_void;
    let Some((offset, size)) = largest_icon(ICON) else {
        return;
    };
    let data = &ICON[offset..offset + size];
    let hicon = unsafe {
        CreateIconFromResourceEx(
            data.as_ptr(),
            size as u32,
            1,
            0x0003_0000,
            0,
            0,
            LR_DEFAULTCOLOR,
        )
    };
    if hicon.is_null() {
        return;
    }
    unsafe {
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, hicon as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, hicon as isize);
    }
}

#[cfg(not(target_os = "windows"))]
fn set_window_icon(_window: &Window) {}

/// `DWMWA_USE_IMMERSIVE_DARK_MODE`: Windows draws the title bar dark or light to match the app.
#[cfg(target_os = "windows")]
fn set_dark_titlebar(hwnd: isize, dark: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute,
    };

    let value: i32 = if dark { 1 } else { 0 };
    let result = unsafe {
        DwmSetWindowAttribute(
            HWND(hwnd as *mut core::ffi::c_void),
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const i32 as *const core::ffi::c_void,
            core::mem::size_of::<i32>() as u32,
        )
    };
    if let Err(err) = result {
        log::debug!("title bar colour: {err}");
    }
}

#[cfg(not(target_os = "windows"))]
fn set_dark_titlebar(_hwnd: isize, _dark: bool) {}

/// Offset and length of the biggest image inside an ICO file.
#[cfg(target_os = "windows")]
fn largest_icon(bytes: &[u8]) -> Option<(usize, usize)> {
    if bytes.len() < 6 || bytes[2] != 1 {
        return None;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let mut best: Option<(u32, usize, usize)> = None;
    for index in 0..count {
        let entry = 6 + index * 16;
        if entry + 16 > bytes.len() {
            return None;
        }
        let width = if bytes[entry] == 0 {
            256
        } else {
            bytes[entry] as u32
        };
        let height = if bytes[entry + 1] == 0 {
            256
        } else {
            bytes[entry + 1] as u32
        };
        let size = u32::from_le_bytes([
            bytes[entry + 8],
            bytes[entry + 9],
            bytes[entry + 10],
            bytes[entry + 11],
        ]) as usize;
        let offset = u32::from_le_bytes([
            bytes[entry + 12],
            bytes[entry + 13],
            bytes[entry + 14],
            bytes[entry + 15],
        ]) as usize;
        if size == 0 || offset + size > bytes.len() {
            continue;
        }
        let area = width * height;
        if best.is_none_or(|(best_area, _, _)| area > best_area) {
            best = Some((area, offset, size));
        }
    }
    best.map(|(_, offset, size)| (offset, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_largest_icon_entry() {
        if let Some((offset, size)) = largest_icon(include_bytes!("../assets/brand/atic-play.ico")) {
            assert!(size > 0);
            assert!(offset + size <= include_bytes!("../assets/brand/atic-play.ico").len());
        }
    }
}
