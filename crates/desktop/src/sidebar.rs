use std::rc::Rc;

use gpui::{AnyElement, App, ClickEvent, Entity, FontWeight, Window, div, img, prelude::*, px};
use gpui_m3::{Button, IconButton, NavItem, Theme};

use ytm_core::Playlist;

use crate::account::{Account, AccountState};
use crate::assets;
use crate::covers::Covers;
use crate::ui::{CARD_COVER_SIZE, skeleton, thumbnail};

/// Height of a playlist row, and the size of its cover.
const PLAYLIST_ROW_HEIGHT: f32 = 44.;
const PLAYLIST_COVER: f32 = 32.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Home,
    Explore,
    Library,
    History,
    Search,
    Playlist,
    Album,
    Artist,
    Settings,
}

type NavigateHandler = Rc<dyn Fn(Route, &mut Window, &mut App)>;
type ActionHandler = Rc<dyn Fn(&mut Window, &mut App)>;
type OpenPlaylistHandler = Rc<dyn Fn(Playlist, &mut Window, &mut App)>;

/// Left panel: main sections, the user's playlists and the account button.
#[derive(IntoElement)]
pub struct Sidebar {
    route: Route,
    /// The playlist on screen, highlighted in the list.
    open_playlist: Option<String>,
    account: Entity<Account>,
    /// Where the playlist covers come from; without it the playlists show placeholders.
    covers: Option<Entity<Covers>>,
    on_navigate: NavigateHandler,
    on_open_playlist: OpenPlaylistHandler,
    on_sign_in: ActionHandler,
    on_new_playlist: ActionHandler,
}

impl Sidebar {
    pub fn new(
        route: Route,
        open_playlist: Option<String>,
        account: Entity<Account>,
        on_navigate: impl Fn(Route, &mut Window, &mut App) + 'static,
        on_open_playlist: impl Fn(Playlist, &mut Window, &mut App) + 'static,
        on_sign_in: impl Fn(&mut Window, &mut App) + 'static,
        on_new_playlist: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Sidebar {
            route,
            open_playlist,
            account,
            covers: None,
            on_navigate: Rc::new(on_navigate),
            on_open_playlist: Rc::new(on_open_playlist),
            on_sign_in: Rc::new(on_sign_in),
            on_new_playlist: Rc::new(on_new_playlist),
        }
    }

    pub fn covers(mut self, covers: Entity<Covers>) -> Self {
        self.covers = Some(covers);
        self
    }

    fn item(&self, route: Route, label: &'static str, icon: &'static str) -> NavItem {
        let on_navigate = self.on_navigate.clone();
        NavItem::new(label, label)
            .icon(icon)
            .selected(self.route == route)
            .on_click(move |_: &ClickEvent, window, cx| on_navigate(route, window, cx))
    }

    fn render_playlists(&self, cx: &mut App) -> impl IntoElement {
        let s = *Theme::of(cx);
        let account = self.account.read(cx);
        let hint = |text: &'static str| {
            div()
                .px_3()
                .pt_1()
                .text_sm()
                .text_color(s.on_surface_variant)
                .child(text)
                .into_any_element()
        };

        let content = match (account.state(), account.library()) {
            (AccountState::SignedIn, Some(library)) if library.playlists.is_empty() => {
                hint("Aún no tienes playlists guardadas")
            }
            (AccountState::SignedIn, Some(library)) => {
                let playlists = library.playlists.clone();
                let items: Vec<_> = playlists
                    .into_iter()
                    .enumerate()
                    .map(|(index, playlist)| {
                        let selected = self.route == Route::Playlist
                            && self.open_playlist.as_deref() == Some(playlist.id.as_str());
                        self.render_playlist(index, playlist, selected, cx)
                    })
                    .collect();
                div().flex().flex_col().children(items).into_any_element()
            }
            (AccountState::SignedIn, None) | (AccountState::Restoring, _) => div()
                .flex()
                .flex_col()
                .children((0..5).map(|index| {
                    let width = 90. + (index * 37 % 70) as f32;
                    div()
                        .h(px(36.))
                        .px_4()
                        .flex()
                        .items_center()
                        .child(skeleton(Some(width), 12., 6.).order(index))
                }))
                .into_any_element(),
            _ => hint("Inicia sesión para ver tus playlists"),
        };
        div()
            .id("sidebar-playlists")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(content)
    }

    /// A playlist with its cover, styled like the navigation items above it.
    fn render_playlist(
        &self,
        index: usize,
        playlist: Playlist,
        selected: bool,
        cx: &mut App,
    ) -> AnyElement {
        let s = *Theme::of(cx);
        // The size the library downloads, so both share one file.
        let cover = self.covers.as_ref().and_then(|covers| {
            covers.update(cx, |covers, cx| {
                covers.get(
                    &playlist.id,
                    playlist.cover_url.as_deref(),
                    CARD_COVER_SIZE,
                    cx,
                )
            })
        });
        let on_open_playlist = self.on_open_playlist.clone();
        let title = playlist.title.clone();
        div()
            .id(("playlist", index))
            .flex()
            .items_center()
            .gap_3()
            .h(px(PLAYLIST_ROW_HEIGHT))
            .px(px((PLAYLIST_ROW_HEIGHT - PLAYLIST_COVER) / 2.))
            .rounded(px(PLAYLIST_ROW_HEIGHT / 2.))
            .cursor_pointer()
            .text_size(px(13.))
            .map(|row| {
                if selected {
                    row.bg(s.secondary_container)
                        .text_color(s.on_secondary_container)
                        .font_weight(FontWeight(700.))
                } else {
                    row.text_color(s.on_surface)
                        .font_weight(FontWeight(500.))
                        .hover(|row| row.bg(s.surface_container_highest))
                }
            })
            .child(thumbnail(cover, PLAYLIST_COVER, 8., cx))
            .child(div().flex_1().min_w_0().truncate().child(title))
            .on_click(move |_: &ClickEvent, window, cx| {
                on_open_playlist(playlist.clone(), window, cx)
            })
            .into_any_element()
    }

    fn render_account(&self, cx: &mut App) -> impl IntoElement {
        let s = *Theme::of(cx);
        let signed_in = self.account.read(cx).is_signed_in();
        let account = self.account.clone();
        let on_sign_in = self.on_sign_in.clone();
        div()
            .pt_2()
            .flex()
            .flex_col()
            .gap_2()
            .when(signed_in, |row| {
                row.child(
                    div()
                        .px_3()
                        .text_sm()
                        .text_color(s.on_surface_variant)
                        .child("Sesión de YouTube Music iniciada"),
                )
                .child(Button::new("sign-out", "Cerrar sesión").text().on_click(
                    move |_: &ClickEvent, _, cx| {
                        account.update(cx, |account, cx| account.sign_out(cx));
                    },
                ))
            })
            .when(!signed_in, |row| {
                row.child(
                    Button::new("sign-in", "Iniciar sesión")
                        .tonal()
                        .on_click(move |_: &ClickEvent, window, cx| on_sign_in(window, cx)),
                )
            })
    }
}

impl RenderOnce for Sidebar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let s = *Theme::of(cx);
        div()
            .size_full()
            .p_3()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .px_3()
                    .pt_2()
                    .pb_4()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(img(assets::LOGO).size(px(32.)).flex_none())
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(crate::APP_NAME),
                    ),
            )
            .child(self.item(Route::Home, "Inicio", assets::HOME))
            .child(self.item(Route::Explore, "Explorar", assets::EXPLORE))
            .child(self.item(Route::Library, "Biblioteca", assets::LIBRARY))
            .child(self.item(Route::History, "Historial", "history"))
            .child(div().mx_3().my_3().h(px(1.)).bg(s.outline_variant))
            .child(
                div()
                    .pl_3()
                    .pb_1()
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(s.on_surface_variant)
                    .child("Playlists")
                    .when(self.account.read(cx).is_signed_in(), |header| {
                        let on_new_playlist = self.on_new_playlist.clone();
                        header.child(
                            IconButton::new("new-playlist", "plus")
                                .size(px(32.))
                                .tooltip("Nueva playlist")
                                .on_click(move |_: &ClickEvent, window, cx| {
                                    on_new_playlist(window, cx)
                                }),
                        )
                    }),
            )
            .child(self.render_playlists(cx))
            .child(self.render_account(cx))
    }
}
