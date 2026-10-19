use std::path::PathBuf;

use gpui::{
    AnyElement, AnyView, ClickEvent, Context, Div, Entity, Hsla, StyleRefinement, Subscription,
    Window, div, prelude::*, px,
};
use gpui_m3::{IconButton, TextField, TextFieldEvent, Theme};
use ytm_core::{Client, Playlist};

use crate::account::Account;
use crate::covers::Covers;
use crate::home_view::HomeView;
use crate::library_view::{LibraryEvent, LibraryView};
use crate::playback::Playback;
use crate::player_bar::{PlayerBar, PlayerBarEvent};
use crate::playlist_view::PlaylistView;
use crate::queue_panel::QueuePanel;
use crate::search_view::SearchView;
use crate::sidebar::{Route, Sidebar};
use crate::sign_in::SignInDialog;
use crate::ui::placeholder;

const SIDEBAR_WIDTH: f32 = 240.;
const QUEUE_WIDTH: f32 = 340.;

/// The window: sidebar, content and queue panel above the player bar.
pub struct YtmApp {
    route: Route,
    /// The id of the playlist page on screen, to highlight it in the sidebar.
    open_playlist: Option<String>,
    account: Entity<Account>,
    search_field: Entity<TextField>,
    home_view: Entity<HomeView>,
    search_view: Entity<SearchView>,
    library_view: Entity<LibraryView>,
    playlist_view: Entity<PlaylistView>,
    queue_panel: Entity<QueuePanel>,
    player_bar: Entity<PlayerBar>,
    queue_open: bool,
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
        let library_view =
            cx.new(|cx| LibraryView::new(account.clone(), playback.clone(), covers.clone(), cx));
        let home_view =
            cx.new(|cx| HomeView::new(client.clone(), playback.clone(), covers.clone(), cx));
        let search_view =
            cx.new(|cx| SearchView::new(client.clone(), playback.clone(), covers.clone(), cx));
        let playlist_view =
            cx.new(|cx| PlaylistView::new(client.clone(), playback.clone(), covers.clone(), cx));
        let queue_panel = cx.new(|cx| QueuePanel::new(playback.clone(), covers.clone(), cx));
        let player_bar = cx.new(|cx| PlayerBar::new(playback.clone(), covers.clone(), cx));
        let search_field = cx.new(|cx| {
            TextField::new(cx)
                .placeholder("¿Qué quieres escuchar?")
                .icon("search")
        });

        let subscriptions = vec![
            cx.subscribe(&search_field, |this, _, event: &TextFieldEvent, cx| {
                if let TextFieldEvent::Submitted(query) = event {
                    this.route = Route::Search;
                    this.search_view
                        .update(cx, |view, cx| view.search(query.to_string(), cx));
                    cx.notify();
                }
            }),
            cx.subscribe(
                &player_bar,
                |this, _, event: &PlayerBarEvent, cx| match event {
                    PlayerBarEvent::ToggleQueue => {
                        this.queue_open = !this.queue_open;
                        let open = this.queue_open;
                        this.player_bar
                            .update(cx, |bar, cx| bar.set_queue_open(open, cx));
                        cx.notify();
                    }
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
                        this.open_playlist(playlist.clone(), cx)
                    }
                },
            ),
            // The sidebar shows the session and the playlists; the dialog closes once signed in.
            cx.observe(&account, |this, account, cx| {
                if account.read(cx).is_signed_in() {
                    this.sign_in_open = false;
                }
                cx.notify();
            }),
            // Recolor the app from each new cover. Progress ticks change nothing here, so this
            // does not redraw the window on its own.
            cx.observe(&playback, |this, playback, cx| {
                let seed = playback.read(cx).seed();
                if seed != this.applied_seed {
                    this.applied_seed = seed;
                    Theme::set(this.dark, seed, cx);
                }
            }),
        ];

        YtmApp {
            route: Route::Home,
            open_playlist: None,
            account,
            search_field,
            home_view,
            search_view,
            library_view,
            playlist_view,
            queue_panel,
            player_bar,
            queue_open: true,
            sign_in_open: false,
            dark: true,
            applied_seed: None,
            _subscriptions: subscriptions,
        }
    }

    fn open_playlist(&mut self, playlist: Playlist, cx: &mut Context<Self>) {
        self.route = Route::Playlist;
        self.open_playlist = Some(playlist.id.clone());
        self.playlist_view
            .update(cx, |view, cx| view.open(playlist, cx));
        cx.notify();
    }

    fn toggle_dark(&mut self, cx: &mut Context<Self>) {
        self.dark = !self.dark;
        Theme::set(self.dark, self.applied_seed, cx);
        cx.notify();
    }

    fn render_content(&self, cx: &mut Context<Self>) -> AnyElement {
        match self.route {
            // Cached: the player bar redraws the window every frame while the wave moves.
            Route::Search => AnyView::from(self.search_view.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            Route::Home => AnyView::from(self.home_view.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            Route::Explore => placeholder(
                "Explorar",
                "Pronto: tendencias, géneros y estados de ánimo",
                cx,
            )
            .into_any_element(),
            Route::Library => AnyView::from(self.library_view.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            Route::Playlist => AnyView::from(self.playlist_view.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
        }
    }
}

impl Render for YtmApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .update(cx, |this, cx| {
                        this.route = route;
                        cx.notify();
                    })
                    .ok();
            },
            move |playlist, _, cx| {
                open_playlist
                    .update(cx, |this, cx| this.open_playlist(playlist, cx))
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
                div()
                    .flex_1()
                    .max_w(px(560.))
                    .child(self.search_field.clone()),
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
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .pb_0()
            .bg(s.surface)
            .text_color(s.on_surface)
            .font_family(gpui_m3::theme::FONT_FAMILY)
            .child(
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
                            .child(div().flex_1().min_h_0().child(self.render_content(cx))),
                    )
                    .when(self.queue_open, |row| {
                        row.child(
                            panel(cx).w(px(QUEUE_WIDTH)).flex_none().child(
                                AnyView::from(self.queue_panel.clone())
                                    .cached(StyleRefinement::default().size_full()),
                            ),
                        )
                    }),
            )
            .child(self.player_bar.clone())
            .children(sign_in)
    }
}

fn panel(cx: &Context<YtmApp>) -> Div {
    div()
        .rounded(px(16.))
        .overflow_hidden()
        .bg(Theme::of(cx).surface_container)
}
