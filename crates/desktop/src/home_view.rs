use gpui::{
    AnyElement, ClickEvent, Context, Entity, SharedString, Subscription, Window, div, prelude::*,
    px,
};
use gpui_m3::{Button, ListItem, Theme};
use ytm_core::{Client, HomeFeed, Track};

use crate::account::Account;
use crate::covers::Covers;
use crate::navigation::{Navigation, with_track_menu};
use crate::playback::Playback;
use crate::search_view::{album_card, artist_card, observe_opening};
use crate::ui::{
    CARD_COVER_SIZE, CARD_WIDTH, format_time, friendly_error, media_card, placeholder,
    section_title, skeleton, skeleton_cards, skeleton_rows, thumbnail,
};

const THUMB_SIZE: u32 = 120;
const MAX_CARDS: usize = 12;
const MAX_TOP_ROWS: usize = 10;

enum State {
    Loading,
    Ready(HomeFeed),
    Failed(SharedString),
}

/// The home page: what the user played lately when signed in, then the charts of the
/// listener's country and new releases.
pub struct HomeView {
    client: Client,
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    navigation: Entity<Navigation>,
    state: State,
    /// Recently played songs, fetched once per session; `None` while signed out or loading.
    history: Option<Vec<Track>>,
    history_requested: bool,
    _subscriptions: Vec<Subscription>,
}

impl HomeView {
    pub fn new(
        client: Client,
        playback: Entity<Playback>,
        covers: Entity<Covers>,
        account: Entity<Account>,
        navigation: Entity<Navigation>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&covers, |_, _, cx| cx.notify()),
            observe_opening(&playback, cx),
            cx.observe(&account, |this, account, cx| {
                let signed_in = account.read(cx).is_signed_in();
                this.follow_session(signed_in, cx);
            }),
        ];
        let signed_in = account.read(cx).is_signed_in();
        let mut view = HomeView {
            client,
            playback,
            covers,
            navigation,
            state: State::Loading,
            history: None,
            history_requested: false,
            _subscriptions: subscriptions,
        };
        view.load(cx);
        view.follow_session(signed_in, cx);
        view
    }

    /// Fetches the history on signing in and forgets it on signing out.
    fn follow_session(&mut self, signed_in: bool, cx: &mut Context<Self>) {
        if !signed_in {
            if self.history_requested {
                self.history_requested = false;
                self.history = None;
                cx.notify();
            }
            return;
        }
        if self.history_requested {
            return;
        }
        self.history_requested = true;
        let history = self.client.history();
        cx.spawn(async move |this, cx| {
            let result = history.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(tracks) if this.history_requested => this.history = Some(tracks),
                    Ok(_) => {}
                    // The rest of the page still works without it.
                    Err(err) => log::warn!("could not load the history: {err:#}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// "Escuchar de nuevo": recent songs in a row; each plays with its radio after it.
    fn render_history(&self, tracks: &[Track], cx: &mut Context<Self>) -> AnyElement {
        let cards: Vec<_> = tracks
            .iter()
            .take(MAX_CARDS)
            .enumerate()
            .map(|(index, track)| {
                let cover = self.covers.update(cx, |covers, cx| {
                    covers.get(&track.id, track.cover_url.as_deref(), CARD_COVER_SIZE, cx)
                });
                let playback = self.playback.clone();
                let clicked = track.clone();
                let card = media_card(
                    ("history", index),
                    thumbnail(cover, CARD_WIDTH, 12., cx),
                    track.title.clone(),
                    Some(track.artists.clone().into()),
                    false,
                    cx,
                )
                .flex_none()
                .on_click(move |_: &ClickEvent, _, cx| {
                    let track = clicked.clone();
                    playback.update(cx, |playback, cx| playback.play_tracks(vec![track], 0, cx));
                });
                with_track_menu(card, track, &self.navigation)
            })
            .collect();
        div()
            .id("history")
            .overflow_x_scroll()
            .flex()
            .gap_2()
            .children(cards)
            .into_any_element()
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.state = State::Loading;
        cx.notify();
        let home = self.client.home();
        cx.spawn(async move |this, cx| {
            let result = home.await;
            this.update(cx, |this, cx| {
                this.state = match result {
                    Ok(feed) => State::Ready(feed),
                    Err(err) => State::Failed(friendly_error(&err)),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Clicking a chart song plays the chart from that song on.
    fn play_list(
        &self,
        tracks: &[Track],
        index: usize,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static {
        let playback = self.playback.clone();
        let tracks = tracks.to_vec();
        move |_, _, cx| {
            let tracks = tracks.clone();
            playback.update(cx, |playback, cx| playback.play_tracks(tracks, index, cx));
        }
    }

    fn render_trending(&self, tracks: &[Track], cx: &mut Context<Self>) -> AnyElement {
        let cards: Vec<_> = tracks
            .iter()
            .take(MAX_CARDS)
            .enumerate()
            .map(|(index, track)| {
                let cover = self.covers.update(cx, |covers, cx| {
                    covers.get(&track.id, track.cover_url.as_deref(), CARD_COVER_SIZE, cx)
                });
                let card = media_card(
                    ("trending", index),
                    thumbnail(cover, CARD_WIDTH, 12., cx),
                    track.title.clone(),
                    Some(track.artists.clone().into()),
                    false,
                    cx,
                )
                .on_click(self.play_list(tracks, index));
                with_track_menu(card, track, &self.navigation)
            })
            .collect();
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(cards)
            .into_any_element()
    }

    fn render_top(&self, tracks: &[Track], cx: &mut Context<Self>) -> AnyElement {
        let s = *Theme::of(cx);
        let rows: Vec<_> = tracks
            .iter()
            .take(MAX_TOP_ROWS)
            .enumerate()
            .map(|(index, track)| {
                let thumb = self.covers.update(cx, |covers, cx| {
                    covers.get(&track.id, track.cover_url.as_deref(), THUMB_SIZE, cx)
                });
                let subtitle = match track.duration_secs {
                    Some(secs) => format!("{} · {}", track.artists, format_time(secs.into())),
                    None => track.artists.clone(),
                };
                let row = ListItem::new(("top", index), track.title.clone())
                    .subtitle(subtitle)
                    .leading(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .w(px(24.))
                                    .flex()
                                    .justify_end()
                                    .text_color(s.on_surface_variant)
                                    .child((index + 1).to_string()),
                            )
                            .child(thumbnail(thumb, 48., 8., cx)),
                    )
                    .on_click(self.play_list(tracks, index));
                with_track_menu(row, track, &self.navigation)
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

impl Render for HomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let feed = match &self.state {
            State::Loading => {
                return div()
                    .size_full()
                    .px_4()
                    .flex()
                    .flex_col()
                    .gap_8()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(skeleton(Some(160.), 20., 10.))
                            .child(skeleton_rows(4)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(skeleton(Some(220.), 20., 10.))
                            .child(skeleton_cards(6, false)),
                    )
                    .into_any_element();
            }
            State::Failed(error) => {
                return placeholder("No se pudo cargar el inicio", error.clone(), cx)
                    .child(
                        div().pt_2().child(
                            Button::new("retry", "Reintentar")
                                .filled()
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.load(cx))),
                        ),
                    )
                    .into_any_element();
            }
            State::Ready(feed) => feed.clone(),
        };

        let mut sections: Vec<AnyElement> = Vec::new();
        if let Some(history) = self.history.clone().filter(|history| !history.is_empty()) {
            sections.push(section(
                "Escuchar de nuevo",
                self.render_history(&history, cx),
            ));
        }
        if !feed.trending.is_empty() {
            sections.push(section(
                "Tendencias",
                self.render_trending(&feed.trending, cx),
            ));
        }
        if !feed.top.is_empty() {
            sections.push(section("Lo más escuchado", self.render_top(&feed.top, cx)));
        }
        if !feed.new_albums.is_empty() {
            let cards: Vec<_> = feed
                .new_albums
                .iter()
                .take(MAX_CARDS)
                .enumerate()
                .map(|(index, album)| {
                    album_card(
                        ("new-album", index),
                        album,
                        &self.covers,
                        &self.playback,
                        &self.navigation,
                        cx,
                    )
                })
                .collect();
            sections.push(section(
                "Nuevos lanzamientos",
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(cards)
                    .into_any_element(),
            ));
        }
        if !feed.artists.is_empty() {
            let cards: Vec<_> = feed
                .artists
                .iter()
                .take(MAX_CARDS)
                .enumerate()
                .map(|(index, artist)| {
                    artist_card(
                        ("chart-artist", index),
                        artist,
                        &self.covers,
                        &self.playback,
                        &self.navigation,
                        cx,
                    )
                })
                .collect();
            sections.push(section(
                "Artistas populares",
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(cards)
                    .into_any_element(),
            ));
        }
        if sections.is_empty() {
            return placeholder("Inicio", "YouTube Music no devolvió recomendaciones", cx)
                .into_any_element();
        }

        div()
            .id("home")
            .size_full()
            .overflow_y_scroll()
            .px_4()
            .pb_6()
            .flex()
            .flex_col()
            .gap_8()
            .children(sections)
            .into_any_element()
    }
}

fn section(title: &'static str, content: AnyElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(section_title(title))
        .child(content)
        .into_any_element()
}
