use gpui::{ClickEvent, Context, Entity, FontWeight, Subscription, Window, div, prelude::*, px};
use gpui_m3::{ListItem, Shape, ShapeName, Theme};

use crate::covers::Covers;
use crate::playback::Playback;
use crate::ui::{format_time, placeholder, section_title, thumbnail};

const THUMB_SIZE: u32 = 120;
const COVER_SIZE: u32 = 544;
/// Enough to see what comes; a radio queue can grow to hundreds of tracks.
const MAX_UPCOMING: usize = 50;

/// Right panel: the current track in large and what plays next.
pub struct QueuePanel {
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    _subscriptions: Vec<Subscription>,
}

impl QueuePanel {
    pub fn new(playback: Entity<Playback>, covers: Entity<Covers>, cx: &mut Context<Self>) -> Self {
        // Only queue changes matter here, not every progress tick, so compare before redrawing.
        let mut last_seen = None;
        let subscriptions = vec![
            cx.observe(&playback, move |_, playback, cx| {
                let playback = playback.read(cx);
                let seen = (
                    playback.queue().current_index(),
                    playback.queue().tracks().len(),
                    playback.queue().shuffle(),
                );
                if last_seen != Some(seen) {
                    last_seen = Some(seen);
                    cx.notify();
                }
            }),
            cx.observe(&covers, |_, _, cx| cx.notify()),
        ];
        QueuePanel {
            playback,
            covers,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for QueuePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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

        let cover_path = self.covers.update(cx, |covers, cx| {
            covers.get(&current.id, current.cover_url.as_deref(), COVER_SIZE, cx)
        });
        let cover = Shape::new(ShapeName::Cookie12).size(px(256.));
        let cover = match cover_path {
            Some(path) => cover.image(path.as_path()),
            None => cover.color(s.surface_container_highest),
        };

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
            ListItem::new(("upcoming", index), track.title.clone())
                .subtitle(subtitle)
                .leading(thumbnail(thumb, 40., 6., cx))
                .on_click(move |_: &ClickEvent, _, cx| {
                    playback.update(cx, |playback, cx| playback.play_index(index, cx));
                })
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
            .child(section_title("A continuación"))
            .child(if rows.is_empty() {
                div()
                    .text_sm()
                    .text_color(s.on_surface_variant)
                    .child("Buscando canciones parecidas…")
                    .into_any_element()
            } else {
                div().flex().flex_col().children(rows).into_any_element()
            })
            .into_any_element()
    }
}
