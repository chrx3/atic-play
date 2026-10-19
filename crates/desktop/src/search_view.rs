use std::future::Future;

use anyhow::Result;
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, Entity, SharedString, Stateful, Subscription,
    Window, div, prelude::*, px,
};
use gpui_m3::{IconButton, ListItem, Segment, SegmentedButtons, Theme};
use ytm_core::{Album, Artist, Client, Track};

use crate::assets;
use crate::covers::Covers;
use crate::navigation::{Navigation, with_track_menu};
use crate::playback::Playback;
use crate::ui::{
    CARD_COVER_SIZE, CARD_WIDTH, MEDIA_CARD_GROUP, avatar, compact_count, format_time, media_card,
    placeholder, section_title, skeleton_cards, skeleton_rows, thumbnail,
};

const THUMB_SIZE: u32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Songs,
    Albums,
    Artists,
}

const TABS: [Tab; 3] = [Tab::Songs, Tab::Albums, Tab::Artists];

enum Load<T> {
    Loading,
    Ready(Vec<T>),
    Failed(SharedString),
}

/// Search results, split by kind as in YouTube Music.
pub struct SearchView {
    client: Client,
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    navigation: Entity<Navigation>,
    query: SharedString,
    /// Bumped on every search, so slow results of an older query are dropped.
    generation: u64,
    tab: Tab,
    songs: Load<Track>,
    albums: Load<Album>,
    artists: Load<Artist>,
    _subscriptions: Vec<Subscription>,
}

impl SearchView {
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
        ];
        SearchView {
            client,
            playback,
            covers,
            navigation,
            query: SharedString::default(),
            generation: 0,
            tab: Tab::Songs,
            songs: Load::Ready(Vec::new()),
            albums: Load::Ready(Vec::new()),
            artists: Load::Ready(Vec::new()),
            _subscriptions: subscriptions,
        }
    }

    pub fn search(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.query = query.clone().into();
        self.generation += 1;
        self.songs = Load::Loading;
        self.albums = Load::Loading;
        self.artists = Load::Loading;
        cx.notify();

        self.load(
            self.client.search_tracks(query.clone()),
            |this| &mut this.songs,
            cx,
        );
        self.load(
            self.client.search_albums(query.clone()),
            |this| &mut this.albums,
            cx,
        );
        self.load(
            self.client.search_artists(query),
            |this| &mut this.artists,
            cx,
        );
    }

    fn load<T: 'static>(
        &mut self,
        request: impl Future<Output = Result<Vec<T>>> + 'static,
        slot: fn(&mut Self) -> &mut Load<T>,
        cx: &mut Context<Self>,
    ) {
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                *slot(this) = match result {
                    Ok(items) => Load::Ready(items),
                    Err(err) => Load::Failed(format!("{err:#}").into()),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_songs(&self, songs: &[Track], cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<_> = songs
            .iter()
            .enumerate()
            .map(|(index, track)| {
                track_row(
                    ("song", index),
                    track,
                    &self.covers,
                    &self.playback,
                    &self.navigation,
                    cx,
                )
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(rows)
            .into_any_element()
    }

    fn render_albums(&self, albums: &[Album], cx: &mut Context<Self>) -> AnyElement {
        let cards: Vec<_> = albums
            .iter()
            .enumerate()
            .map(|(index, album)| {
                album_card(
                    ("album", index),
                    album,
                    &self.covers,
                    &self.playback,
                    &self.navigation,
                    cx,
                )
            })
            .collect();
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(cards)
            .into_any_element()
    }

    fn render_artists(&self, artists: &[Artist], cx: &mut Context<Self>) -> AnyElement {
        let cards: Vec<_> = artists
            .iter()
            .enumerate()
            .map(|(index, artist)| {
                artist_card(
                    ("artist", index),
                    artist,
                    &self.covers,
                    &self.playback,
                    &self.navigation,
                    cx,
                )
            })
            .collect();
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(cards)
            .into_any_element()
    }
}

impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.query.is_empty() {
            return placeholder("Buscar", "Escribe arriba y presiona Enter", cx).into_any_element();
        }
        let view = cx.entity().downgrade();
        let selected = TABS.iter().position(|tab| *tab == self.tab).unwrap_or(0);

        let content = match self.tab {
            Tab::Songs => match &self.songs {
                Load::Ready(items) if !items.is_empty() => {
                    let items = items.clone();
                    self.render_songs(&items, cx)
                }
                other => status(other, Tab::Songs, cx),
            },
            Tab::Albums => match &self.albums {
                Load::Ready(items) if !items.is_empty() => {
                    let items = items.clone();
                    self.render_albums(&items, cx)
                }
                other => status(other, Tab::Albums, cx),
            },
            Tab::Artists => match &self.artists {
                Load::Ready(items) if !items.is_empty() => {
                    let items = items.clone();
                    self.render_artists(&items, cx)
                }
                other => status(other, Tab::Artists, cx),
            },
        };

        div()
            .id("search-results")
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
                    .justify_between()
                    .gap_4()
                    .child(section_title(format!("Resultados para “{}”", self.query)))
                    .child(
                        SegmentedButtons::new(
                            "search-tabs",
                            [
                                Segment::label("Canciones"),
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
            .child(content)
            .into_any_element()
    }
}

/// What a tab shows while its results load, fail or come back empty.
fn status<T>(load: &Load<T>, tab: Tab, cx: &mut Context<SearchView>) -> AnyElement {
    let s = *Theme::of(cx);
    let message = match load {
        Load::Loading => {
            return match tab {
                Tab::Songs => skeleton_rows(8),
                Tab::Albums => skeleton_cards(8, false),
                Tab::Artists => skeleton_cards(8, true),
            }
            .into_any_element();
        }
        Load::Failed(error) => div()
            .text_color(s.error)
            .child(format!("La búsqueda falló: {error}"))
            .into_any_element(),
        Load::Ready(_) => div()
            .text_color(s.on_surface_variant)
            .child("Sin resultados")
            .into_any_element(),
    };
    div()
        .pt_16()
        .flex()
        .justify_center()
        .child(message)
        .into_any_element()
}

/// Redraws a view when an album or artist starts or stops loading, not on every progress tick.
pub fn observe_opening<V: 'static>(
    playback: &Entity<Playback>,
    cx: &mut Context<V>,
) -> Subscription {
    let mut last_opening = None;
    cx.observe(playback, move |_, playback, cx| {
        let opening = playback.read(cx).opening().map(str::to_string);
        if opening != last_opening {
            last_opening = opening;
            cx.notify();
        }
    })
}

/// A song row. Like YouTube Music, clicking plays the song and lets its radio fill the queue;
/// a right click opens the song menu.
pub fn track_row<V: 'static>(
    id: impl Into<gpui::ElementId>,
    track: &Track,
    covers: &Entity<Covers>,
    playback: &Entity<Playback>,
    navigation: &Entity<Navigation>,
    cx: &mut Context<V>,
) -> Div {
    let thumb = covers.update(cx, |covers, cx| {
        covers.get(&track.id, track.cover_url.as_deref(), THUMB_SIZE, cx)
    });
    let mut subtitle = vec![track.artists.clone()];
    subtitle.extend(track.album.clone());
    subtitle.extend(track.duration_secs.map(|secs| format_time(secs.into())));
    let playback = playback.clone();
    let clicked = track.clone();
    let row = ListItem::new(id, track.title.clone())
        .subtitle(subtitle.join(" · "))
        .leading(thumbnail(thumb, 48., 8., cx))
        .on_click(move |_: &ClickEvent, _, cx| {
            let track = clicked.clone();
            playback.update(cx, |playback, cx| playback.play_tracks(vec![track], 0, cx));
        });
    with_track_menu(row, track, navigation)
}

/// An album card: opens the album page; its play button plays the album right away.
pub fn album_card<V: 'static>(
    id: impl Into<gpui::ElementId>,
    album: &Album,
    covers: &Entity<Covers>,
    playback: &Entity<Playback>,
    navigation: &Entity<Navigation>,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let cover = covers.update(cx, |covers, cx| {
        covers.get(&album.id, album.cover_url.as_deref(), CARD_COVER_SIZE, cx)
    });
    let subtitle = match album.year {
        Some(year) => format!("{year} · {}", album.artists),
        None => album.artists.clone(),
    };
    let busy = playback.read(cx).opening() == Some(album.id.as_str());
    let play = {
        let playback = playback.clone();
        let album_id = album.id.clone();
        move |cx: &mut App| {
            playback.update(cx, |playback, cx| playback.play_album(album_id.clone(), cx))
        }
    };
    let navigation = navigation.clone();
    let summary = album.clone();
    media_card(
        id,
        card_artwork(thumbnail(cover, CARD_WIDTH, 12., cx), busy, play),
        album.title.clone(),
        Some(subtitle.into()),
        busy,
        cx,
    )
    .on_click(move |_: &ClickEvent, _, cx| {
        let summary = summary.clone();
        navigation.update(cx, |navigation, cx| {
            navigation.open_album(summary.id.clone(), Some(summary), cx)
        });
    })
}

/// An artist card: opens the artist page; its play button plays their top songs.
pub fn artist_card<V: 'static>(
    id: impl Into<gpui::ElementId>,
    artist: &Artist,
    covers: &Entity<Covers>,
    playback: &Entity<Playback>,
    navigation: &Entity<Navigation>,
    cx: &mut Context<V>,
) -> Stateful<Div> {
    let picture = covers.update(cx, |covers, cx| {
        covers.get(
            &artist.id,
            artist.avatar_url.as_deref(),
            CARD_COVER_SIZE,
            cx,
        )
    });
    let subtitle = artist
        .subscribers
        .map(|count| format!("{} suscriptores", compact_count(count)).into());
    let busy = playback.read(cx).opening() == Some(artist.id.as_str());
    let play = {
        let playback = playback.clone();
        let artist_id = artist.id.clone();
        move |cx: &mut App| {
            playback.update(cx, |playback, cx| {
                playback.play_artist(artist_id.clone(), cx)
            })
        }
    };
    let navigation = navigation.clone();
    let summary = artist.clone();
    media_card(
        id,
        card_artwork(avatar(picture, CARD_WIDTH, cx), busy, play),
        artist.name.clone(),
        subtitle,
        busy,
        cx,
    )
    .items_center()
    .on_click(move |_: &ClickEvent, _, cx| {
        let summary = summary.clone();
        navigation.update(cx, |navigation, cx| {
            navigation.open_artist(summary.id.clone(), Some(summary), cx)
        });
    })
}

/// Card artwork with a play button in its corner, shown while the card is hovered.
fn card_artwork(
    artwork: impl IntoElement,
    busy: bool,
    play: impl Fn(&mut App) + 'static,
) -> impl IntoElement {
    div().relative().child(artwork).when(!busy, |artwork| {
        artwork.child(
            div()
                .absolute()
                .right_2()
                .bottom_2()
                .opacity(0.)
                .group_hover(MEDIA_CARD_GROUP, |style| style.opacity(1.))
                .child(
                    IconButton::new("card-play", assets::PLAY)
                        .filled()
                        .size(px(44.))
                        .tooltip("Reproducir")
                        .on_click(move |_: &ClickEvent, _, cx| {
                            // The card under it would open the page too.
                            cx.stop_propagation();
                            play(cx);
                        }),
                ),
        )
    })
}
