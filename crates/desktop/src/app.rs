use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, AnyView, ClickEvent, Context, Div, Entity, FocusHandle, Focusable, Hsla,
    MouseButton, NavigationDirection, StyleRefinement, Subscription, Window, div, prelude::*, px,
};
use gpui_m3::{IconButton, Segment, SegmentedButtons, Spring, Theme, Toast, animate_theme, motion};
use ytm_core::{Album, Artist, Client, Playlist};

use crate::account::Account;
use crate::album_view::AlbumView;
use crate::artist_view::ArtistView;
use crate::covers::Covers;
use crate::history_view::HistoryView;
use crate::home_view::HomeView;
use crate::library_view::{LibraryEvent, LibraryView};
use crate::lyrics_panel::LyricsPanel;
use crate::media_session::SeekDirection;
use crate::navigation::{Navigation, NavigationEvent, render_track_menu};
use crate::now_playing_view::{NowPlayingEvent, NowPlayingView};
use crate::playback::Playback;
use crate::player_bar::{PlayerBar, PlayerBarEvent};
use crate::playlist_view::PlaylistView;
use crate::queue_panel::QueuePanel;
use crate::search_box::{SearchBox, SearchBoxEvent};
use crate::search_view::SearchView;
use crate::settings;
use crate::settings_view::{SettingsEvent, SettingsView};
use crate::shortcuts::{
    self, Close, CycleRepeat, GoBack, NextTrack, PreviousTrack, SeekBackward, SeekForward,
    ToggleMute, TogglePlay, ToggleShuffle, VolumeDown, VolumeUp,
};
use crate::sidebar::{Route, Sidebar};
use crate::sign_in::SignInDialog;
use crate::toasts::Toasts;
use crate::ui::placeholder;

const SIDEBAR_WIDTH: f32 = 240.;
const QUEUE_WIDTH: f32 = 340.;
/// How far a new page slides up as it fades in.
const ROUTE_SLIDE: f32 = 24.;
/// How far "Reproduciendo" rises as it opens.
const NOW_PLAYING_SLIDE: f32 = 64.;
/// Older pages are forgotten past this many.
const MAX_BACK_PAGES: usize = 50;
/// What the arrow keys move.
const SEEK_STEP: Duration = Duration::from_secs(5);
const VOLUME_STEP: f32 = 0.05;
/// Room for the player bar under the toasts.
const TOAST_BOTTOM: f32 = 104.;

/// A page the back button returns to: a section or what was opened in it.
#[derive(Clone)]
enum Page {
    Route(Route),
    Playlist(Playlist),
    Album { id: String, summary: Option<Album> },
    Artist { id: String, summary: Option<Artist> },
}

impl Page {
    fn is_same(&self, other: &Page) -> bool {
        match (self, other) {
            (Page::Route(a), Page::Route(b)) => a == b,
            (Page::Playlist(a), Page::Playlist(b)) => a.id == b.id,
            (Page::Album { id: a, .. }, Page::Album { id: b, .. })
            | (Page::Artist { id: a, .. }, Page::Artist { id: b, .. }) => a == b,
            _ => false,
        }
    }
}

/// What the right panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidePanel {
    Queue,
    Lyrics,
}

/// The window: sidebar, content and queue panel above the player bar.
pub struct YtmApp {
    route: Route,
    /// The page on screen and the ones before it, for going back.
    page: Page,
    back: Vec<Page>,
    focus_handle: FocusHandle,
    /// The id of the playlist page on screen, to highlight it in the sidebar.
    open_playlist: Option<String>,
    account: Entity<Account>,
    playback: Entity<Playback>,
    navigation: Entity<Navigation>,
    search_box: Entity<SearchBox>,
    home_view: Entity<HomeView>,
    search_view: Entity<SearchView>,
    library_view: Entity<LibraryView>,
    history_view: Entity<HistoryView>,
    playlist_view: Entity<PlaylistView>,
    album_view: Entity<AlbumView>,
    artist_view: Entity<ArtistView>,
    settings_view: Entity<SettingsView>,
    queue_panel: Entity<QueuePanel>,
    lyrics_panel: Entity<LyricsPanel>,
    now_playing_view: Entity<NowPlayingView>,
    player_bar: Entity<PlayerBar>,
    queue_open: bool,
    side_panel: SidePanel,
    now_playing_open: bool,
    /// Bumped on every page change, so each new page plays its entrance.
    page_generation: usize,
    sign_in_open: bool,
    dark: bool,
    applied_seed: Option<Hsla>,
    _subscriptions: Vec<Subscription>,
}

impl YtmApp {
    pub fn new(client: Client, data_dir: PathBuf, cx: &mut Context<Self>) -> Self {
        let covers = cx.new(|_| Covers::new(client.clone()));
        let account = cx.new(|cx| Account::new(client.clone(), data_dir, cx));
        let playback = cx.new(|cx| Playback::new(client.clone(), cx));
        let navigation = cx.new(|_| Navigation::new());
        let library_view = cx.new(|cx| {
            LibraryView::new(
                account.clone(),
                playback.clone(),
                covers.clone(),
                navigation.clone(),
                cx,
            )
        });
        let home_view = cx.new(|cx| {
            HomeView::new(
                client.clone(),
                playback.clone(),
                covers.clone(),
                account.clone(),
                navigation.clone(),
                cx,
            )
        });
        let search_view = cx.new(|cx| {
            SearchView::new(
                client.clone(),
                playback.clone(),
                covers.clone(),
                navigation.clone(),
                cx,
            )
        });
        let playlist_view = cx.new(|cx| {
            PlaylistView::new(
                client.clone(),
                playback.clone(),
                covers.clone(),
                navigation.clone(),
                cx,
            )
        });
        let album_view = cx.new(|cx| {
            AlbumView::new(
                client.clone(),
                playback.clone(),
                covers.clone(),
                navigation.clone(),
                cx,
            )
        });
        let artist_view = cx.new(|cx| {
            ArtistView::new(
                client.clone(),
                playback.clone(),
                covers.clone(),
                navigation.clone(),
                cx,
            )
        });
        let history_view =
            cx.new(|cx| HistoryView::new(playback.clone(), covers.clone(), navigation.clone(), cx));
        let settings_view = cx.new(|cx| SettingsView::new(client.clone(), account.clone(), cx));
        let queue_panel =
            cx.new(|cx| QueuePanel::new(playback.clone(), covers.clone(), navigation.clone(), cx));
        let lyrics_panel = cx.new(|cx| LyricsPanel::new(playback.clone(), cx));
        let now_playing_view = cx.new(|cx| {
            NowPlayingView::new(
                playback.clone(),
                covers.clone(),
                account.clone(),
                lyrics_panel.clone(),
                cx,
            )
        });
        let player_bar =
            cx.new(|cx| PlayerBar::new(playback.clone(), covers.clone(), account.clone(), cx));
        let search_box =
            cx.new(|cx| SearchBox::new(client.clone(), playback.clone(), navigation.clone(), cx));

        let subscriptions = vec![
            cx.subscribe(&search_box, |this, _, event: &SearchBoxEvent, cx| {
                let SearchBoxEvent::Search(query) = event;
                this.visit(Page::Route(Route::Search), cx);
                this.search_view
                    .update(cx, |view, cx| view.search(query.clone(), cx));
            }),
            cx.subscribe(
                &player_bar,
                |this, _, event: &PlayerBarEvent, cx| match event {
                    PlayerBarEvent::ToggleQueue => {
                        this.queue_open = !this.queue_open;
                        let open = this.queue_open;
                        this.player_bar
                            .update(cx, |bar, cx| bar.set_queue_open(open, cx));
                        settings::update(cx, |settings| settings.queue_open = Some(open));
                        cx.notify();
                    }
                    PlayerBarEvent::ToggleNowPlaying => {
                        this.set_now_playing_open(!this.now_playing_open, cx)
                    }
                },
            ),
            cx.subscribe(
                &now_playing_view,
                |this, _, event: &NowPlayingEvent, cx| match event {
                    NowPlayingEvent::Close => this.set_now_playing_open(false, cx),
                },
            ),
            cx.subscribe(
                &library_view,
                |this, _, event: &LibraryEvent, cx| match event {
                    LibraryEvent::SignIn => {
                        this.sign_in_open = true;
                        cx.notify();
                    }
                    LibraryEvent::OpenPlaylist(playlist) => {
                        this.visit(Page::Playlist(playlist.clone()), cx)
                    }
                },
            ),
            cx.subscribe(
                &navigation,
                |this, _, event: &NavigationEvent, cx| match event {
                    NavigationEvent::OpenAlbum { id, summary } => this.visit(
                        Page::Album {
                            id: id.clone(),
                            summary: summary.clone(),
                        },
                        cx,
                    ),
                    NavigationEvent::OpenArtist { id, summary } => this.visit(
                        Page::Artist {
                            id: id.clone(),
                            summary: summary.clone(),
                        },
                        cx,
                    ),
                },
            ),
            cx.subscribe(
                &settings_view,
                |this, _, event: &SettingsEvent, cx| match event {
                    SettingsEvent::SetDark(dark) => {
                        if *dark != this.dark {
                            this.toggle_dark(cx);
                        }
                    }
                    SettingsEvent::SignIn => {
                        this.sign_in_open = true;
                        cx.notify();
                    }
                },
            ),
            // The song menu opens and closes.
            cx.observe(&navigation, |_, _, cx| cx.notify()),
            cx.observe(&Toasts::global(cx), |_, _, cx| cx.notify()),
            // The sidebar shows the session and the playlists; the dialog closes once signed in.
            cx.observe(&account, |this, account, cx| {
                if account.read(cx).is_signed_in() {
                    this.sign_in_open = false;
                }
                cx.notify();
            }),
            // Recolor the app from each new cover, blending into the new colors. Progress ticks
            // change nothing here, so this does not redraw the window on its own.
            cx.observe(&playback, |this, playback, cx| {
                let seed = playback.read(cx).seed();
                if seed != this.applied_seed {
                    this.applied_seed = seed;
                    animate_theme(this.dark, seed, cx);
                }
            }),
        ];

        // The theme and the side panel as they were left.
        let saved = settings::get(cx);
        let dark = saved.dark.unwrap_or(true);
        let queue_open = saved.queue_open.unwrap_or(true);
        if !dark {
            Theme::set(dark, None, cx);
        }
        player_bar.update(cx, |bar, cx| bar.set_queue_open(queue_open, cx));

        YtmApp {
            route: Route::Home,
            page: Page::Route(Route::Home),
            back: Vec::new(),
            focus_handle: cx.focus_handle(),
            open_playlist: None,
            account,
            playback,
            navigation,
            search_box,
            home_view,
            search_view,
            library_view,
            history_view,
            playlist_view,
            album_view,
            artist_view,
            settings_view,
            queue_panel,
            lyrics_panel,
            now_playing_view,
            player_bar,
            queue_open,
            side_panel: SidePanel::Queue,
            now_playing_open: false,
            page_generation: 0,
            sign_in_open: false,
            dark,
            applied_seed: None,
            _subscriptions: subscriptions,
        }
    }

    /// Goes to `page`, remembering the one on screen for going back.
    fn visit(&mut self, page: Page, cx: &mut Context<Self>) {
        if !page.is_same(&self.page) {
            let left = std::mem::replace(&mut self.page, page.clone());
            self.back.push(left);
            if self.back.len() > MAX_BACK_PAGES {
                self.back.remove(0);
            }
        }
        self.show(page, cx);
    }

    /// Closes "Reproduciendo", or returns to the page before this one.
    fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.now_playing_open {
            self.set_now_playing_open(false, cx);
            return;
        }
        if let Some(page) = self.back.pop() {
            self.page = page.clone();
            self.show(page, cx);
        }
    }

    fn show(&mut self, page: Page, cx: &mut Context<Self>) {
        match page {
            Page::Route(route) => self.navigate(route, cx),
            Page::Playlist(playlist) => self.open_playlist(playlist, cx),
            Page::Album { id, summary } => {
                self.navigate(Route::Album, cx);
                self.album_view
                    .update(cx, |view, cx| view.open(id, summary, cx));
            }
            Page::Artist { id, summary } => {
                self.navigate(Route::Artist, cx);
                self.artist_view
                    .update(cx, |view, cx| view.open(id, summary, cx));
            }
        }
    }

    /// Escape: closes the song menu, then "Reproduciendo".
    fn close(&mut self, cx: &mut Context<Self>) {
        if self.navigation.read(cx).track_menu().is_some() {
            self.navigation
                .update(cx, |navigation, cx| navigation.close_track_menu(cx));
        } else {
            self.set_now_playing_open(false, cx);
        }
    }

    fn update_playback(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Playback, &mut Context<Playback>),
    ) {
        self.playback.update(cx, f);
    }

    /// Shows `route`, leaving "Reproduciendo" if it is open.
    fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        // Album and artist pages replace each other under the same route: each is a new page.
        if route != self.route || matches!(route, Route::Album | Route::Artist) {
            self.page_generation += 1;
        }
        if route != self.route {
            match route {
                Route::Settings => self.settings_view.update(cx, |view, cx| view.refresh(cx)),
                Route::History => self.history_view.update(cx, |view, cx| view.refresh(cx)),
                _ => {}
            }
        }
        self.route = route;
        self.set_now_playing_open(false, cx);
        cx.notify();
    }

    fn open_playlist(&mut self, playlist: Playlist, cx: &mut Context<Self>) {
        // Another playlist is another page, even though the route stays the same.
        if self.open_playlist.as_ref() != Some(&playlist.id) {
            self.page_generation += 1;
        }
        self.open_playlist = Some(playlist.id.clone());
        self.navigate(Route::Playlist, cx);
        self.playlist_view
            .update(cx, |view, cx| view.open(playlist, cx));
    }

    fn set_now_playing_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if open == self.now_playing_open {
            return;
        }
        self.now_playing_open = open;
        // Each way plays an entrance: "Reproduciendo" rises in, the page comes back.
        self.page_generation += 1;
        self.player_bar
            .update(cx, |bar, cx| bar.set_now_playing_open(open, cx));
        cx.notify();
    }

    fn toggle_dark(&mut self, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        Theme::set(self.dark, self.applied_seed, cx);
        let dark = self.dark;
        settings::update(cx, |settings| settings.dark = Some(dark));
        cx.notify();
    }

    /// The page on screen, fading and sliding in when it changes.
    fn render_content(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let progress = motion::entrance(
            ("page", self.page_generation),
            Spring::SPATIAL,
            0.,
            window,
            cx,
        );
        let view = match self.route {
            Route::Search => Some(AnyView::from(self.search_view.clone())),
            Route::Home => Some(AnyView::from(self.home_view.clone())),
            Route::Explore => None,
            Route::Library => Some(AnyView::from(self.library_view.clone())),
            Route::History => Some(AnyView::from(self.history_view.clone())),
            Route::Playlist => Some(AnyView::from(self.playlist_view.clone())),
            Route::Album => Some(AnyView::from(self.album_view.clone())),
            Route::Artist => Some(AnyView::from(self.artist_view.clone())),
            Route::Settings => Some(AnyView::from(self.settings_view.clone())),
        };
        let page = match view {
            // Cached: the player bar redraws the window every frame while the wave moves. Not
            // while it moves in, though: a cached view would replay its last frame without the
            // fade.
            Some(view) if progress >= 1. => view
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            Some(view) => view.into_any_element(),
            None => placeholder(
                "Explorar",
                "Pronto: tendencias, géneros y estados de ánimo",
                cx,
            )
            .into_any_element(),
        };
        if progress >= 1. {
            return page;
        }
        div()
            .size_full()
            .relative()
            .top(px((1. - progress) * ROUTE_SLIDE))
            .opacity(progress.clamp(0., 1.))
            .child(page)
            .into_any_element()
    }

    /// The right panel: the queue or the lyrics, under tabs.
    fn render_side_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let (content, selected) = match self.side_panel {
            SidePanel::Queue => (AnyView::from(self.queue_panel.clone()), 0),
            SidePanel::Lyrics => (AnyView::from(self.lyrics_panel.clone()), 1),
        };
        div()
            .w(px(QUEUE_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .child(
                div().px_4().pt_4().child(
                    SegmentedButtons::new(
                        "side-panel",
                        [Segment::label("Cola"), Segment::label("Letra")],
                        selected,
                    )
                    .on_change(move |index, _, cx| {
                        view.update(cx, |this, cx| {
                            this.side_panel = if index == 0 {
                                SidePanel::Queue
                            } else {
                                SidePanel::Lyrics
                            };
                            cx.notify();
                        })
                        .ok();
                    }),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(content.cached(StyleRefinement::default().size_full())),
            )
    }
}

impl Render for YtmApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let view = cx.entity().downgrade();
        let navigate = view.clone();
        let open_playlist = view.clone();
        let open_sign_in = view.clone();
        let sidebar = Sidebar::new(
            self.route,
            self.open_playlist.clone(),
            self.account.clone(),
            move |route, _, cx| {
                navigate
                    .update(cx, |this, cx| this.visit(Page::Route(route), cx))
                    .ok();
            },
            move |playlist, _, cx| {
                open_playlist
                    .update(cx, |this, cx| this.visit(Page::Playlist(playlist), cx))
                    .ok();
            },
            move |_, cx| {
                open_sign_in
                    .update(cx, |this, cx| {
                        this.sign_in_open = true;
                        cx.notify();
                    })
                    .ok();
            },
        );
        let sign_in = self.sign_in_open.then(|| {
            SignInDialog::new(self.account.clone(), move |_, cx| {
                view.update(cx, |this, cx| {
                    this.sign_in_open = false;
                    cx.notify();
                })
                .ok();
            })
        });

        let header = div()
            .flex()
            .items_center()
            .gap_4()
            .px_4()
            .py_3()
            .child(
                IconButton::new("back", "arrow-left")
                    .tooltip("Atrás")
                    .disabled(self.back.is_empty())
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.go_back(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .max_w(px(560.))
                    .child(self.search_box.clone()),
            )
            .child(div().flex_1())
            .child(
                IconButton::new("theme", if self.dark { "sun" } else { "moon" })
                    .tooltip(if self.dark {
                        "Tema claro"
                    } else {
                        "Tema oscuro"
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_dark(cx))),
            )
            .child(
                IconButton::new("settings", "gear")
                    .selected(self.route == Route::Settings)
                    .tooltip("Ajustes")
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.visit(Page::Route(Route::Settings), cx)
                    })),
            );

        // The right panel grows and shrinks instead of popping in and out.
        let side_width = motion::animate_f32(
            "side-panel-width",
            Spring::SPATIAL,
            if self.queue_open { QUEUE_WIDTH } else { 0. },
            window,
            cx,
        );
        let main = if self.now_playing_open {
            let progress = motion::entrance(
                ("now-playing", self.page_generation),
                Spring::SPATIAL,
                0.,
                window,
                cx,
            );
            panel(cx)
                .flex_1()
                .min_h_0()
                .relative()
                .top(px((1. - progress) * NOW_PLAYING_SLIDE))
                .opacity(progress.clamp(0., 1.))
                .child(self.now_playing_view.clone())
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap_2()
                .child(panel(cx).w(px(SIDEBAR_WIDTH)).flex_none().child(sidebar))
                .child(
                    panel(cx)
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(header)
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .child(self.render_content(window, cx)),
                        ),
                )
                // Under a pixel wide it is gone, and so is its gap.
                .when(side_width >= 1., |row| {
                    row.child(
                        panel(cx)
                            .w(px(side_width))
                            .flex_none()
                            .child(self.render_side_panel(cx)),
                    )
                })
        };

        let toast = Toasts::global(cx).read(cx).current().cloned();
        let toast = toast.map(|(shown, text)| {
            div()
                .absolute()
                .bottom(px(TOAST_BOTTOM))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .max_w(px(480.))
                        .child(Toast::new(("toast", shown), text)),
                )
        });

        div()
            .track_focus(&self.focus_handle)
            .key_context(shortcuts::KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &TogglePlay, _, cx| {
                this.update_playback(cx, Playback::toggle_pause)
            }))
            .on_action(
                cx.listener(|this, _: &NextTrack, _, cx| this.update_playback(cx, Playback::next)),
            )
            .on_action(cx.listener(|this, _: &PreviousTrack, _, cx| {
                this.update_playback(cx, Playback::previous)
            }))
            .on_action(cx.listener(|this, _: &SeekForward, _, cx| {
                this.update_playback(cx, |playback, cx| {
                    playback.seek_by(SeekDirection::Forward, SEEK_STEP, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &SeekBackward, _, cx| {
                this.update_playback(cx, |playback, cx| {
                    playback.seek_by(SeekDirection::Backward, SEEK_STEP, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &VolumeUp, _, cx| {
                this.update_playback(cx, |playback, cx| {
                    playback.set_volume(playback.volume() + VOLUME_STEP, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &VolumeDown, _, cx| {
                this.update_playback(cx, |playback, cx| {
                    playback.set_volume(playback.volume() - VOLUME_STEP, cx)
                })
            }))
            .on_action(cx.listener(|this, _: &ToggleMute, _, cx| {
                this.update_playback(cx, Playback::toggle_mute)
            }))
            .on_action(cx.listener(|this, _: &ToggleShuffle, _, cx| {
                this.update_playback(cx, Playback::toggle_shuffle)
            }))
            .on_action(cx.listener(|this, _: &CycleRepeat, _, cx| {
                this.update_playback(cx, Playback::cycle_repeat)
            }))
            .on_action(cx.listener(|this, _: &GoBack, _, cx| this.go_back(cx)))
            .on_action(cx.listener(|this, _: &Close, _, cx| this.close(cx)))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, _, cx| this.go_back(cx)),
            )
            // A click anywhere takes the keyboard back from a text field, so the shortcuts work
            // again; a click on a text field gives it the keyboard right after.
            .capture_any_mouse_down(cx.listener(|this, _, window, _| {
                window.focus(&this.focus_handle);
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .pb_0()
            .bg(s.surface)
            .text_color(s.on_surface)
            .font_family(gpui_m3::theme::FONT_FAMILY)
            .child(main)
            .child(self.player_bar.clone())
            .children(render_track_menu(
                &self.navigation,
                &self.playback,
                &self.account,
                cx,
            ))
            .children(sign_in)
            .children(toast)
    }
}

impl Focusable for YtmApp {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn panel(cx: &Context<YtmApp>) -> Div {
    div()
        .rounded(px(16.))
        .overflow_hidden()
        .bg(Theme::of(cx).surface_container)
}
