use gpui::{
    AnyElement, ClickEvent, Context, Entity, FontWeight, SharedString, Subscription, Window, div,
    prelude::*, px,
};
use gpui_m3::{IconButton, Theme};
use ytm_core::{Artist, ArtistPage, Client, Track};

use crate::assets;
use crate::covers::Covers;
use crate::downloads::Downloads;
use crate::navigation::Navigation;
use crate::playback::Playback;
use crate::search_view::{album_card, artist_card, observe_opening, track_row};
use crate::ui::{
    CARD_COVER_SIZE, avatar, compact_count, failed, friendly_error, section_title, skeleton,
    skeleton_cards, skeleton_rows,
};

const PICTURE_SIZE: f32 = 200.;
/// Popular songs shown before the albums, as in YouTube Music.
const TOP_TRACKS: usize = 5;
const MAX_CARDS: usize = 12;

enum State {
    Loading,
    Ready(ArtistPage),
    Failed(SharedString),
}

/// An artist page: picture and name, popular songs, albums and similar artists.
pub struct ArtistView {
    client: Client,
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    navigation: Entity<Navigation>,
    /// What the card that opened it already knew, shown while the page loads.
    summary: Option<Artist>,
    /// The artist on screen, to load it again after a failure.
    id: String,
    state: State,
    /// Bumped on every `open`, so a slow artist cannot replace a newer one.
    generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl ArtistView {
    pub fn new(
        client: Client,
        playback: Entity<Playback>,
        covers: Entity<Covers>,
        navigation: Entity<Navigation>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&covers, |_, _, cx| cx.notify()),
            observe_opening(&playback, cx),
            // The marks on downloaded songs.
            cx.observe(&Downloads::global(cx), |_, _, cx| cx.notify()),
        ];
        ArtistView {
            client,
            playback,
            covers,
            navigation,
            summary: None,
            id: String::new(),
            state: State::Loading,
            generation: 0,
            _subscriptions: subscriptions,
        }
    }

    pub fn open(&mut self, id: String, summary: Option<Artist>, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let request = self.client.artist(id.clone());
        self.id = id;
        self.summary = summary;
        self.state = State::Loading;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let page = request.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.state = match page {
                    Ok(page) => State::Ready(page),
                    Err(err) => State::Failed(friendly_error(&err)),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn top_tracks(&self) -> &[Track] {
        match &self.state {
            State::Ready(page) => &page.top_tracks,
            _ => &[],
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let page = match &self.state {
            State::Ready(page) => Some(page),
            _ => None,
        };
        let artist = page.map(|page| &page.artist).or(self.summary.as_ref());
        let picture = artist.and_then(|artist| {
            self.covers.update(cx, |covers, cx| {
                covers.get(
                    &artist.id,
                    artist.avatar_url.as_deref(),
                    CARD_COVER_SIZE,
                    cx,
                )
            })
        });
        let name = match artist {
            Some(artist) => div()
                .text_size(px(40.))
                .font_weight(FontWeight::BOLD)
                .truncate()
                .child(artist.name.clone())
                .into_any_element(),
            None => skeleton(Some(260.), 40., 14.).into_any_element(),
        };
        let subscribers = artist
            .and_then(|artist| artist.subscribers)
            .map(|count| format!("{} suscriptores", compact_count(count)));
        let description = page.and_then(|page| page.description.clone());
        let has_tracks = !self.top_tracks().is_empty();

        div()
            .flex()
            .items_end()
            .gap_6()
            .px_4()
            .pb_4()
            .child(avatar(picture, PICTURE_SIZE, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .child("Artista"),
                    )
                    .child(name)
                    .children(subscribers.map(|subscribers| {
                        div()
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .child(subscribers)
                    }))
                    .children(description.map(|description| {
                        div()
                            .max_w(px(640.))
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .line_clamp(3)
                            .child(description)
                    }))
                    .child(
                        div()
                            .pt_2()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                IconButton::new("artist-play", assets::PLAY)
                                    .filled()
                                    .hover_morph(true)
                                    .size(px(56.))
                                    .disabled(!has_tracks)
                                    .tooltip("Reproducir")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        let tracks = this.top_tracks().to_vec();
                                        this.playback.update(cx, |playback, cx| {
                                            playback.play_tracks(tracks, 0, cx)
                                        });
                                    })),
                            )
                            .child(
                                IconButton::new("artist-shuffle", assets::SHUFFLE)
                                    .tonal()
                                    .size(px(48.))
                                    .disabled(!has_tracks)
                                    .tooltip("Reproducir en aleatorio")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        let tracks = this.top_tracks().to_vec();
                                        this.playback.update(cx, |playback, cx| {
                                            playback.play_shuffled(tracks, cx)
                                        });
                                    })),
                            ),
                    ),
            )
    }

    fn render_sections(&self, page: &ArtistPage, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut sections = Vec::new();
        if !page.top_tracks.is_empty() {
            let rows: Vec<_> = page
                .top_tracks
                .iter()
                .take(TOP_TRACKS)
                .enumerate()
                .map(|(index, track)| {
                    track_row(
                        ("artist-top", index),
                        track,
                        &self.covers,
                        &self.playback,
                        &self.navigation,
                        cx,
                    )
                })
                .collect();
            sections.push(section(
                "Canciones populares",
                div().flex().flex_col().gap_1().children(rows),
            ));
        }
        if !page.albums.is_empty() {
            let cards: Vec<_> = page
                .albums
                .iter()
                .take(MAX_CARDS)
                .enumerate()
                .map(|(index, album)| {
                    album_card(
                        ("artist-album", index),
                        album,
                        &self.covers,
                        &self.playback,
                        &self.navigation,
                        cx,
                    )
                })
                .collect();
            sections.push(section(
                "Álbumes y sencillos",
                div().flex().flex_wrap().gap_2().children(cards),
            ));
        }
        if !page.similar.is_empty() {
            let cards: Vec<_> = page
                .similar
                .iter()
                .take(MAX_CARDS)
                .enumerate()
                .map(|(index, artist)| {
                    artist_card(
                        ("similar-artist", index),
                        artist,
                        &self.covers,
                        &self.playback,
                        &self.navigation,
                        cx,
                    )
                })
                .collect();
            sections.push(section(
                "Artistas similares",
                div().flex().flex_wrap().gap_2().children(cards),
            ));
        }
        sections
    }
}

impl Render for ArtistView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sections = match &self.state {
            State::Loading => vec![
                section("Canciones populares", skeleton_rows(TOP_TRACKS)),
                section("Álbumes y sencillos", skeleton_cards(6, false)),
            ],
            State::Failed(error) => {
                return failed(
                    "No se pudo abrir el artista",
                    error.clone(),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.open(this.id.clone(), this.summary.clone(), cx)
                    }),
                    cx,
                )
                .into_any_element();
            }
            State::Ready(page) => {
                let page = page.clone();
                self.render_sections(&page, cx)
            }
        };
        div()
            .id("artist")
            .size_full()
            .overflow_y_scroll()
            .pb_6()
            .flex()
            .flex_col()
            .gap_8()
            .child(self.render_header(cx))
            .child(div().px_4().flex().flex_col().gap_8().children(sections))
            .into_any_element()
    }
}

fn section(title: &'static str, content: impl IntoElement) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(section_title(title))
        .child(content)
        .into_any_element()
}
