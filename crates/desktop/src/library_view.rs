use gpui::{
    AnyElement, ClickEvent, Context, Entity, EventEmitter, Subscription, Window, div, prelude::*,
    px,
};
use gpui_m3::{Button, IconButton, ListItem, LoadingIndicator, Segment, SegmentedButtons, Theme};
use ytm_core::{Library, Playlist};

use crate::account::{Account, AccountState};
use crate::covers::Covers;
use crate::playback::Playback;
use crate::search_view::{album_card, artist_card, observe_opening};
use crate::ui::{
    CARD_COVER_SIZE, CARD_WIDTH, format_time, media_card, placeholder, section_title, thumbnail,
};

const THUMB_SIZE: u32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Playlists,
    Liked,
    Albums,
    Artists,
}

const TABS: [Tab; 4] = [Tab::Playlists, Tab::Liked, Tab::Albums, Tab::Artists];

pub enum LibraryEvent {
    SignIn,
    OpenPlaylist(Playlist),
}

/// What the signed-in user saved in YouTube Music.
pub struct LibraryView {
    account: Entity<Account>,
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    tab: Tab,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<LibraryEvent> for LibraryView {}

impl LibraryView {
    pub fn new(
        account: Entity<Account>,
        playback: Entity<Playback>,
        covers: Entity<Covers>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&account, |_, _, cx| cx.notify()),
            cx.observe(&covers, |_, _, cx| cx.notify()),
            observe_opening(&playback, cx),
        ];
        LibraryView {
            account,
            playback,
            covers,
            tab: Tab::Playlists,
            _subscriptions: subscriptions,
        }
    }

    fn render_playlists(&self, playlists: &[Playlist], cx: &mut Context<Self>) -> AnyElement {
        let cards: Vec<_> = playlists
            .iter()
            .enumerate()
            .map(|(index, playlist)| {
                let cover = self.covers.update(cx, |covers, cx| {
                    covers.get(
                        &playlist.id,
                        playlist.cover_url.as_deref(),
                        CARD_COVER_SIZE,
                        cx,
                    )
                });
                let mut subtitle = Vec::new();
                subtitle.extend(playlist.owner.clone());
                subtitle.extend(
                    playlist
                        .track_count
                        .map(|count| format!("{count} canciones")),
                );
                let open = playlist.clone();
                media_card(
                    ("playlist", index),
                    thumbnail(cover, CARD_WIDTH, 12., cx),
                    playlist.title.clone(),
                    (!subtitle.is_empty()).then(|| subtitle.join(" · ").into()),
                    false,
                    cx,
                )
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.emit(LibraryEvent::OpenPlaylist(open.clone()))
                }))
            })
            .collect();
        grid(cards)
    }

    fn render_liked(&self, library: &Library, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<_> = library
            .liked
            .iter()
            .enumerate()
            .map(|(index, track)| {
                let thumb = self.covers.update(cx, |covers, cx| {
                    covers.get(&track.id, track.cover_url.as_deref(), THUMB_SIZE, cx)
                });
                let mut subtitle = vec![track.artists.clone()];
                subtitle.extend(track.album.clone());
                subtitle.extend(track.duration_secs.map(|secs| format_time(secs.into())));
                let playback = self.playback.clone();
                let liked = library.liked.clone();
                ListItem::new(("liked", index), track.title.clone())
                    .subtitle(subtitle.join(" · "))
                    .leading(thumbnail(thumb, 48., 8., cx))
                    // Plays the liked songs from this one on, as YouTube Music does.
                    .on_click(move |_: &ClickEvent, _, cx| {
                        let liked = liked.clone();
                        playback.update(cx, |playback, cx| playback.play_tracks(liked, index, cx));
                    })
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(rows)
            .into_any_element()
    }
}

impl Render for LibraryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let account = self.account.read(cx);
        let error = account.error().cloned();
        let loading = account.is_library_loading();
        let library = account.library().cloned();

        match account.state() {
            AccountState::Restoring | AccountState::SigningIn => {
                return div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(LoadingIndicator::new().size(px(56.)))
                    .into_any_element();
            }
            AccountState::SignedOut => {
                return placeholder(
                    "Tu biblioteca",
                    "Inicia sesión para ver tus playlists, tus me gusta, álbumes y artistas",
                    cx,
                )
                .child(
                    div().pt_2().child(
                        Button::new("library-sign-in", "Iniciar sesión")
                            .filled()
                            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                                cx.emit(LibraryEvent::SignIn)
                            })),
                    ),
                )
                .into_any_element();
            }
            AccountState::SignedIn => {}
        }

        let Some(library) = library else {
            if let Some(error) = error {
                return placeholder("No se pudo cargar tu biblioteca", error, cx)
                    .into_any_element();
            }
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(LoadingIndicator::new().size(px(56.)))
                .into_any_element();
        };

        let content = match self.tab {
            Tab::Playlists if !library.playlists.is_empty() => {
                self.render_playlists(&library.playlists, cx)
            }
            Tab::Liked if !library.liked.is_empty() => self.render_liked(&library, cx),
            Tab::Albums if !library.albums.is_empty() => {
                let cards: Vec<_> = library
                    .albums
                    .iter()
                    .enumerate()
                    .map(|(index, album)| {
                        album_card(
                            ("saved-album", index),
                            album,
                            &self.covers,
                            &self.playback,
                            cx,
                        )
                    })
                    .collect();
                grid(cards)
            }
            Tab::Artists if !library.artists.is_empty() => {
                let cards: Vec<_> = library
                    .artists
                    .iter()
                    .enumerate()
                    .map(|(index, artist)| {
                        artist_card(
                            ("saved-artist", index),
                            artist,
                            &self.covers,
                            &self.playback,
                            cx,
                        )
                    })
                    .collect();
                grid(cards)
            }
            _ => div()
                .pt_16()
                .flex()
                .justify_center()
                .text_color(s.on_surface_variant)
                .child("Nada por aquí todavía")
                .into_any_element(),
        };

        let view = cx.entity().downgrade();
        let selected = TABS.iter().position(|tab| *tab == self.tab).unwrap_or(0);
        div()
            .id("library")
            .size_full()
            .overflow_y_scroll()
            .px_4()
            .pb_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
                    .child(section_title("Tu biblioteca"))
                    .child(
                        IconButton::new("library-reload", "refresh")
                            .disabled(loading)
                            .tooltip("Actualizar")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.account
                                    .update(cx, |account, cx| account.reload_library(cx));
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        SegmentedButtons::new(
                            "library-tabs",
                            [
                                Segment::label("Playlists"),
                                Segment::label("Me gusta"),
                                Segment::label("Álbumes"),
                                Segment::label("Artistas"),
                            ],
                            selected,
                        )
                        .on_change(move |index, _, cx| {
                            view.update(cx, |this, cx| {
                                this.tab = TABS[index];
                                cx.notify();
                            })
                            .ok();
                        }),
                    ),
            )
            .children(error.map(|error| div().text_sm().text_color(s.error).child(error)))
            .child(content)
            .into_any_element()
    }
}

fn grid(cards: Vec<impl IntoElement>) -> AnyElement {
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .children(cards)
        .into_any_element()
}
