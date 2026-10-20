use gpui::{
    ClickEvent, Context, ElementId, Entity, FontWeight, Subscription, Window, div, prelude::*, px,
};
use gpui_m3::{Button, IconButton, ListItem, ShapeName, Spring, Theme, motion};

use crate::covers::Covers;
use crate::download_ui::downloaded_mark;
use crate::downloads::Downloads;
use crate::navigation::{Navigation, with_track_menu};
use crate::playback::Playback;
use crate::ui::{format_time, placeholder, section_title, thumbnail, track_cover};

const THUMB_SIZE: u32 = 120;
/// Seconds between the entrances of consecutive new rows.
const ROW_STAGGER: f32 = 0.035;
/// Rows past this one enter together, so a long new queue does not trickle in.
const MAX_STAGGERED: usize = 12;
/// How far a new row slides in from the right.
const ROW_SLIDE: f32 = 24.;
const COVER_SIZE: u32 = 544;
/// Enough to see what comes; a radio queue can grow to hundreds of tracks.
const MAX_UPCOMING: usize = 50;

/// Right panel: the current track in large and what plays next.
pub struct QueuePanel {
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    navigation: Entity<Navigation>,
    _subscriptions: Vec<Subscription>,
}

impl QueuePanel {
    pub fn new(
        playback: Entity<Playback>,
        covers: Entity<Covers>,
        navigation: Entity<Navigation>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Only queue changes matter here, not every progress tick, so compare before redrawing.
        let mut last_seen = None;
        let subscriptions = vec![
            cx.observe(&playback, move |_, playback, cx| {
                let playback = playback.read(cx);
                let seen = (
                    playback.queue().current_index(),
                    playback.queue().tracks().len(),
                    playback.queue().shuffle(),
                    playback.is_fetching_radio(),
                );
                if last_seen != Some(seen) {
                    last_seen = Some(seen);
                    cx.notify();
                }
            }),
            cx.observe(&covers, |_, _, cx| cx.notify()),
            // The marks on downloaded songs.
            cx.observe(&Downloads::global(cx), |_, _, cx| cx.notify()),
        ];
        QueuePanel {
            playback,
            covers,
            navigation,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for QueuePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let playback = self.playback.read(cx);
        let Some(current) = playback.current().cloned() else {
            return placeholder("Cola vacía", "Reproduce algo para empezar", cx).into_any_element();
        };
        let first_upcoming = playback
            .queue()
            .current_index()
            .map_or(0, |index| index + 1);
        let upcoming: Vec<_> = playback
            .queue()
            .upcoming()
            .iter()
            .take(MAX_UPCOMING)
            .cloned()
            .collect();
        let hidden = playback
            .queue()
            .upcoming()
            .len()
            .saturating_sub(MAX_UPCOMING);
        let fetching_radio = playback.is_fetching_radio();

        let cover_path = self.covers.update(cx, |covers, cx| {
            covers.get(&current.id, current.cover_url.as_deref(), COVER_SIZE, cx)
        });
        let cover = track_cover(
            "queue",
            &current.id,
            cover_path,
            (ShapeName::Cookie12, ShapeName::Sunny),
            256.,
            window,
            cx,
        );

        let downloads = Downloads::global(cx);
        let rows = upcoming.into_iter().enumerate().map(|(offset, track)| {
            let thumb = self.covers.update(cx, |covers, cx| {
                covers.get(&track.id, track.cover_url.as_deref(), THUMB_SIZE, cx)
            });
            let index = first_upcoming + offset;
            let playback = self.playback.clone();
            let subtitle = match track.duration_secs {
                Some(secs) => format!("{} · {}", track.artists, format_time(secs.into())),
                None => track.artists.clone(),
            };
            let mark = downloaded_mark(("upcoming-downloaded", index), &track.id, &downloads, cx);
            let to_remove = self.playback.clone();
            let remove = IconButton::new(("upcoming-remove", index), "x")
                .size(px(32.))
                .tooltip("Quitar de la cola")
                .on_click(move |_: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    to_remove.update(cx, |playback, cx| playback.remove_from_queue(index, cx));
                });
            let item = ListItem::new(("upcoming", index), track.title.clone())
                .subtitle(subtitle)
                .leading(thumbnail(thumb, 40., 6., cx))
                .trailing(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .children(mark)
                        .child(remove),
                )
                .on_click(move |_: &ClickEvent, _, cx| {
                    playback.update(cx, |playback, cx| playback.play_index(index, cx));
                });
            // Keyed by place and song: rows that stay when the queue advances keep still, and
            // only songs new to a place slide in.
            let progress = motion::entrance(
                ElementId::Name(format!("queue-row-{index}-{}", track.id).into()),
                Spring::SPATIAL,
                offset.min(MAX_STAGGERED) as f32 * ROW_STAGGER,
                window,
                cx,
            );
            div()
                .relative()
                .left(px((1. - progress) * ROW_SLIDE))
                .opacity(progress.clamp(0., 1.))
                .child(with_track_menu(item, &track, &self.navigation))
        });
        let rows: Vec<_> = rows.collect();

        div()
            .id("queue-panel")
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .flex()
            .flex_col()
            .gap_4()
            .child(section_title("Sonando ahora"))
            .child(div().flex().justify_center().py_2().child(cover))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .line_clamp(2)
                            .child(current.title.clone()),
                    )
                    .child(
                        div()
                            .text_color(s.on_surface_variant)
                            .child(current.artists.clone()),
                    )
                    .children(current.album.clone().map(|album| {
                        div()
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .child(album)
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(section_title("A continuación"))
                    .when(!rows.is_empty(), |header| {
                        let playback = self.playback.clone();
                        header.child(Button::new("clear-queue", "Vaciar").text().on_click(
                            move |_: &ClickEvent, _, cx| {
                                playback.update(cx, |playback, cx| playback.clear_upcoming(cx));
                            },
                        ))
                    }),
            )
            .child(if rows.is_empty() {
                div()
                    .text_sm()
                    .text_color(s.on_surface_variant)
                    .child(if fetching_radio {
                        "Buscando canciones parecidas…"
                    } else {
                        "No hay más canciones en la cola"
                    })
                    .into_any_element()
            } else {
                div().flex().flex_col().children(rows).into_any_element()
            })
            .when(hidden > 0, |panel| {
                panel.child(
                    div()
                        .text_sm()
                        .text_color(s.on_surface_variant)
                        .child(format!("y {hidden} más")),
                )
            })
            .into_any_element()
    }
}
