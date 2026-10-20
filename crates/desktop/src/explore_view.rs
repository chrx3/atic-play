//! "Explorar": YouTube Music's moods and genres, and each one's playlists.

use gpui::{
    AnyElement, ClickEvent, Context, Entity, EventEmitter, FontWeight, Hsla, SharedString,
    Subscription, Window, div, prelude::*, px, rgb,
};
use gpui_m3::Theme;
use gpui_m3::theme::alpha;
use ytm_core::{Client, Genre, GenrePage, Playlist};

use crate::covers::Covers;
use crate::ui::{
    CARD_COVER_SIZE, CARD_WIDTH, failed, friendly_error, hero, media_card, section_title, skeleton,
    skeleton_cards, thumbnail,
};

/// A genre page lists many playlists per section; this many are enough to browse, and keep the
/// page from downloading hundreds of covers.
const MAX_SECTION_PLAYLISTS: usize = 12;
const GENRE_CARD_WIDTH: f32 = 200.;
const GENRE_CARD_HEIGHT: f32 = 64.;

pub enum ExploreEvent {
    OpenGenre(Genre),
    OpenPlaylist(Playlist),
}

enum Load<T> {
    Loading,
    Ready(T),
    Failed(SharedString),
}

pub struct ExploreView {
    client: Client,
    covers: Entity<Covers>,
    /// `None` until "Explorar" is first opened.
    genres: Option<Load<Vec<Genre>>>,
    /// The genre page on screen, if any.
    genre: Option<(Genre, Load<GenrePage>)>,
    /// Bumped on every genre opened, so a slow page cannot replace a newer one.
    generation: u64,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ExploreEvent> for ExploreView {}

impl ExploreView {
    pub fn new(client: Client, covers: Entity<Covers>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&covers, |_, _, cx| cx.notify())];
        ExploreView {
            client,
            covers,
            genres: None,
            genre: None,
            generation: 0,
            _subscriptions: subscriptions,
        }
    }

    /// Shows the moods and genres, loading them the first time.
    pub fn show_genres(&mut self, cx: &mut Context<Self>) {
        self.genre = None;
        if !matches!(self.genres, Some(Load::Ready(_) | Load::Loading)) {
            self.load_genres(cx);
        }
        cx.notify();
    }

    fn load_genres(&mut self, cx: &mut Context<Self>) {
        self.genres = Some(Load::Loading);
        let request = self.client.genres();
        cx.spawn(async move |this, cx| {
            let genres = request.await;
            this.update(cx, |this, cx| {
                this.genres = Some(match genres {
                    Ok(genres) => Load::Ready(genres),
                    Err(err) => Load::Failed(friendly_error(&err)),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub fn open_genre(&mut self, genre: Genre, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let request = self.client.genre(genre.id.clone());
        self.genre = Some((genre, Load::Loading));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let page = request.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                if let Some((_, load)) = &mut this.genre {
                    *load = match page {
                        Ok(page) => Load::Ready(page),
                        Err(err) => Load::Failed(friendly_error(&err)),
                    };
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_genres(&self, cx: &mut Context<Self>) -> AnyElement {
        let genres = match &self.genres {
            Some(Load::Ready(genres)) => genres.clone(),
            Some(Load::Failed(error)) => {
                return failed(
                    "No se pudo cargar Explorar",
                    error.clone(),
                    cx.listener(|this, _: &ClickEvent, _, cx| this.load_genres(cx)),
                    cx,
                )
                .into_any_element();
            }
            _ => {
                return div()
                    .px_4()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(skeleton(Some(220.), 20., 10.))
                    .child(genre_grid((0..12).map(|index| {
                        skeleton(Some(GENRE_CARD_WIDTH), GENRE_CARD_HEIGHT, 20.)
                            .order(index)
                            .into_any_element()
                    })))
                    .into_any_element();
            }
        };
        let (moods, genres): (Vec<_>, Vec<_>) = genres.into_iter().partition(|genre| genre.is_mood);
        div()
            .px_4()
            .pb_6()
            .flex()
            .flex_col()
            .gap_6()
            .child(self.render_genre_group("Estados de ánimo y momentos", moods, cx))
            .child(self.render_genre_group("Géneros", genres, cx))
            .into_any_element()
    }

    fn render_genre_group(
        &self,
        title: &'static str,
        genres: Vec<Genre>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let s = *Theme::of(cx);
        let cards = genres.into_iter().map(|genre| {
            let color = genre_color(&genre);
            let name = genre.name.clone();
            div()
                .id(SharedString::from(format!("genre-{}", genre.id)))
                .relative()
                .w(px(GENRE_CARD_WIDTH))
                .h(px(GENRE_CARD_HEIGHT))
                .pl_6()
                .pr_3()
                .flex()
                .items_center()
                .overflow_hidden()
                .rounded(px(20.))
                .bg(alpha(color, 0.22))
                .hover(|card| card.bg(alpha(color, 0.34)))
                .cursor_pointer()
                .text_color(s.on_surface)
                .font_weight(FontWeight::MEDIUM)
                // The genre's own color, as a stripe at its start like in YouTube Music.
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(8.))
                        .bg(color),
                )
                .child(div().truncate().child(name))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.emit(ExploreEvent::OpenGenre(genre.clone()))
                }))
                .into_any_element()
        });
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(section_title(title))
            .child(genre_grid(cards))
    }

    fn render_genre(
        &self,
        genre: &Genre,
        load: &Load<GenrePage>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = *Theme::of(cx);
        let header = hero(Some(genre_color(genre)), cx)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(s.on_surface_variant)
                    .child(if genre.is_mood {
                        "Estado de ánimo"
                    } else {
                        "Género"
                    }),
            )
            .child(
                div()
                    .text_size(px(40.))
                    .font_weight(FontWeight::BOLD)
                    .truncate()
                    .child(genre.name.clone()),
            );
        let body = match load {
            Load::Loading => div()
                .px_4()
                .flex()
                .flex_col()
                .gap_3()
                .child(skeleton(Some(220.), 20., 10.))
                .child(skeleton_cards(6, false))
                .into_any_element(),
            Load::Failed(error) => {
                let genre = genre.clone();
                failed(
                    "No se pudo abrir",
                    error.clone(),
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.open_genre(genre.clone(), cx)
                    }),
                    cx,
                )
                .h(px(240.))
                .into_any_element()
            }
            Load::Ready(page) => {
                let sections: Vec<_> = page
                    .sections
                    .iter()
                    .enumerate()
                    .map(|(index, section)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(section_title(section.name.clone()))
                            .child(
                                div()
                                    .id(("genre-section", index))
                                    .overflow_x_scroll()
                                    .flex()
                                    .gap_2()
                                    .pb_2()
                                    .children(self.render_playlists(index, &section.playlists, cx)),
                            )
                    })
                    .collect();
                div()
                    .px_4()
                    .pb_6()
                    .flex()
                    .flex_col()
                    .gap_6()
                    .children(sections)
                    .into_any_element()
            }
        };
        div().child(header).child(body).into_any_element()
    }

    fn render_playlists(
        &self,
        section: usize,
        playlists: &[Playlist],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        playlists
            .iter()
            .take(MAX_SECTION_PLAYLISTS)
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
                let open = playlist.clone();
                media_card(
                    ("genre-playlist", section * MAX_SECTION_PLAYLISTS + index),
                    thumbnail(cover, CARD_WIDTH, 12., cx),
                    playlist.title.clone(),
                    playlist.owner.clone().map(SharedString::from),
                    false,
                    cx,
                )
                .flex_none()
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.emit(ExploreEvent::OpenPlaylist(open.clone()))
                }))
                .into_any_element()
            })
            .collect()
    }
}

impl Render for ExploreView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.genre {
            Some((genre, load)) => self.render_genre(genre, load, cx),
            None => div()
                .pt_2()
                .child(section_title("Explorar").px_4().pb_4())
                .child(self.render_genres(cx))
                .into_any_element(),
        };
        div()
            .id("explore")
            .size_full()
            .overflow_y_scroll()
            .child(content)
    }
}

fn genre_grid(cards: impl IntoIterator<Item = AnyElement>) -> impl IntoElement {
    div().flex().flex_wrap().gap_3().children(cards)
}

fn genre_color(genre: &Genre) -> Hsla {
    rgb(genre.color).into()
}
