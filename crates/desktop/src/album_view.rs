use std::ops::Range;

use gpui::{
    AnyElement, ClickEvent, Context, Entity, FontWeight, SharedString, Subscription, Window, div,
    prelude::*, px, uniform_list,
};
use gpui_m3::{IconButton, ListItem, Theme};
use ytm_core::{Album, AlbumPage, Client, Track};

use crate::assets;
use crate::covers::Covers;
use crate::download_ui::{download_button, downloaded_mark};
use crate::downloads::Downloads;
use crate::navigation::{Navigation, with_track_menu};
use crate::playback::Playback;
use crate::ui::{
    CARD_COVER_SIZE, failed, format_time, friendly_error, skeleton, skeleton_rows, thumbnail,
};

const COVER_WIDTH: f32 = 200.;

enum State {
    Loading,
    Ready(AlbumPage),
    Failed(SharedString),
}

/// An album page: header with play and shuffle, then its tracks.
pub struct AlbumView {
    client: Client,
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    navigation: Entity<Navigation>,
    downloads: Entity<Downloads>,
    /// What the card that opened it already knew, shown while the tracks load.
    summary: Option<Album>,
    /// The album on screen, to load it again after a failure.
    id: String,
    state: State,
    /// Bumped on every `open`, so a slow album cannot replace a newer one.
    generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl AlbumView {
    pub fn new(
        client: Client,
        playback: Entity<Playback>,
        covers: Entity<Covers>,
        navigation: Entity<Navigation>,
        cx: &mut Context<Self>,
    ) -> Self {
        let downloads = Downloads::global(cx);
        let subscriptions = vec![
            cx.observe(&covers, |_, _, cx| cx.notify()),
            // Download progress and the marks on downloaded songs.
            cx.observe(&downloads, |_, _, cx| cx.notify()),
        ];
        AlbumView {
            client,
            playback,
            covers,
            navigation,
            downloads,
            summary: None,
            id: String::new(),
            state: State::Loading,
            generation: 0,
            _subscriptions: subscriptions,
        }
    }

    pub fn open(&mut self, id: String, summary: Option<Album>, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let request = self.client.album(id.clone());
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

    fn tracks(&self) -> &[Track] {
        match &self.state {
            State::Ready(page) => &page.tracks,
            _ => &[],
        }
    }

    fn render_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let s = *Theme::of(cx);
        let tracks: Vec<Track> = self.tracks()[range.clone()].to_vec();
        tracks
            .into_iter()
            .zip(range)
            .map(|(track, index)| {
                // Album tracks share the album's cover, so the number stands in for it.
                let row = ListItem::new(("album-track", index), track.title.clone())
                    .subtitle(track.artists.clone())
                    .leading(
                        div()
                            .w(px(32.))
                            .flex()
                            .justify_end()
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .child((index + 1).to_string()),
                    )
                    .trailing(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .children(downloaded_mark(
                                ("album-downloaded", index),
                                &track.id,
                                &self.downloads,
                                cx,
                            ))
                            .child(
                                div().text_sm().text_color(s.on_surface_variant).child(
                                    track
                                        .duration_secs
                                        .map(|secs| format_time(secs.into()))
                                        .unwrap_or_default(),
                                ),
                            ),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        let tracks = this.tracks().to_vec();
                        this.playback
                            .update(cx, |playback, cx| playback.play_tracks(tracks, index, cx));
                    }));
                with_track_menu(row, &track, &self.navigation).into_any_element()
            })
            .collect()
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let page = match &self.state {
            State::Ready(page) => Some(page),
            _ => None,
        };
        let album = page.map(|page| &page.album).or(self.summary.as_ref());
        let cover = album.and_then(|album| {
            self.covers.update(cx, |covers, cx| {
                covers.get(&album.id, album.cover_url.as_deref(), CARD_COVER_SIZE, cx)
            })
        });
        let mut meta = Vec::new();
        if let Some(album) = album {
            meta.push(album.artists.clone());
            meta.extend(album.year.map(|year| year.to_string()));
        }
        if let Some(page) = page {
            meta.push(match page.tracks.len() {
                1 => "1 canción".to_string(),
                count => format!("{count} canciones"),
            });
        }
        let has_tracks = page.is_some_and(|page| !page.tracks.is_empty());
        // The album's artist, to open their page from the header.
        let artist_id = page.and_then(|page| page.tracks.iter().find_map(|t| t.artist_id.clone()));

        let title = match album {
            Some(album) => div()
                .text_size(px(32.))
                .font_weight(FontWeight::BOLD)
                .truncate()
                .child(album.title.clone())
                .into_any_element(),
            None => skeleton(Some(280.), 32., 12.).into_any_element(),
        };
        let navigation = self.navigation.clone();
        let artists = div()
            .id("album-artists")
            .text_sm()
            .text_color(s.on_surface_variant)
            .child(meta.join(" · "))
            .when_some(artist_id, |artists, artist_id| {
                artists
                    .cursor_pointer()
                    .hover(|style| style.underline())
                    .on_click(move |_: &ClickEvent, _, cx| {
                        let artist_id = artist_id.clone();
                        navigation.update(cx, |navigation, cx| {
                            navigation.open_artist(artist_id, None, cx)
                        });
                    })
            });

        div()
            .flex()
            .items_end()
            .gap_6()
            .px_4()
            .pb_4()
            .child(thumbnail(cover, COVER_WIDTH, 16., cx))
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
                            .child("Álbum"),
                    )
                    .child(title)
                    .child(artists)
                    .child(
                        div()
                            .pt_2()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                IconButton::new("album-play", assets::PLAY)
                                    .filled()
                                    .hover_morph(true)
                                    .size(px(56.))
                                    .disabled(!has_tracks)
                                    .tooltip("Reproducir")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        let tracks = this.tracks().to_vec();
                                        this.playback.update(cx, |playback, cx| {
                                            playback.play_tracks(tracks, 0, cx)
                                        });
                                    })),
                            )
                            .child(
                                IconButton::new("album-shuffle", assets::SHUFFLE)
                                    .tonal()
                                    .size(px(48.))
                                    .disabled(!has_tracks)
                                    .tooltip("Reproducir en aleatorio")
                                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        let tracks = this.tracks().to_vec();
                                        this.playback.update(cx, |playback, cx| {
                                            playback.play_shuffled(tracks, cx)
                                        });
                                    })),
                            )
                            .children(page.filter(|_| has_tracks).map(|page| {
                                download_button(
                                    &page.album.id,
                                    &page.album.title,
                                    &page.tracks,
                                    &self.downloads,
                                    cx,
                                )
                            })),
                    ),
            )
    }
}

impl Render for AlbumView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let body = match &self.state {
            State::Loading => div().px_4().child(skeleton_rows(10)).into_any_element(),
            State::Failed(error) => {
                return failed(
                    "No se pudo abrir el álbum",
                    error.clone(),
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.open(this.id.clone(), this.summary.clone(), cx)
                    }),
                    cx,
                )
                .into_any_element();
            }
            State::Ready(page) if page.tracks.is_empty() => div()
                .pt_16()
                .flex()
                .justify_center()
                .text_color(s.on_surface_variant)
                .child("Este álbum no tiene canciones")
                .into_any_element(),
            State::Ready(page) => uniform_list(
                "album-tracks",
                page.tracks.len(),
                cx.processor(|this, range, _, cx| this.render_rows(range, cx)),
            )
            .flex_1()
            .px_4()
            .pb_4()
            .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_header(cx))
            .child(body)
            .into_any_element()
    }
}
