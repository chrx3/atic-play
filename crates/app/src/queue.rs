//! The queue: a standard side sheet with what plays next.

use std::sync::Arc;

use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div, px,
    prelude::FluentBuilder as _,
};
use gpui_m3::{
    Button, Icon, ListItem, SideSheet, Theme,
    theme::type_scale::{BODY_MEDIUM, TypeScaleExt},
};

use crate::covers::CoverCache;
use crate::icons::icon;
use crate::model::{Model, click, total_duration};
use crate::page::art;
use crate::source::{Track, clock_text};

pub const WIDTH: f32 = 340.;

pub struct QueuePanel {
    model: Entity<Model>,
    covers: Entity<CoverCache>,
    /// The queue as it was before "Vaciar", so the mistake can be undone.
    undo: Option<Vec<Track>>,
}

impl QueuePanel {
    pub fn new(model: Entity<Model>, covers: Entity<CoverCache>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&covers, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            covers,
            undo: None,
        }
    }

    fn cover(&mut self, url: Option<&str>, cx: &mut Context<Self>) -> Option<Arc<gpui::RenderImage>> {
        let peeked = { self.covers.read(cx).peek(url) };
        if peeked.is_some() {
            return peeked;
        }
        self.covers.update(cx, |covers, cx| {
            covers.image(url, cx);
        });
        None
    }

    /// Empties the queue, keeping a copy of it to be able to undo it.
    fn clear(&mut self, cx: &mut Context<Self>) {
        let queue = self.model.read(cx).queue.clone();
        if queue.is_empty() {
            return;
        }
        self.undo = Some(queue);
        let model = self.model.clone();
        model.update(cx, |model, cx| model.clear_queue(cx));
        model.update(cx, |model, cx| model.say("Cola vaciada", cx));
    }

    /// Puts the queue back the way it was.
    fn undo_clear(&mut self, cx: &mut Context<Self>) {
        let Some(queue) = self.undo.take() else {
            return;
        };
        let model = self.model.clone();
        model.update(cx, |model, cx| {
            model.queue = queue;
            cx.notify();
        });
    }
}

impl Render for QueuePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let (queue, current, playing, dark) = {
            let model = self.model.read(cx);
            (
                model.queue.clone(),
                model.current.clone(),
                model.playing,
                model.dark,
            )
        };
        let total = total_duration(&queue);
        let mut rows = div().flex().flex_col().gap(px(2.));
        if let Some(track) = &current {
            let image = self.cover(track.thumbnail.as_deref(), cx);
            // The waves only while it plays: in pause the row is the one that is on.
            let marker = if playing {
                Icon::new(icon("waves")).size(px(18.)).color(s.primary)
            } else {
                Icon::new(icon("disc")).size(px(18.)).color(s.on_surface_variant)
            };
            rows = rows.child(
                ListItem::new("queue-current", track.title.clone())
                    .subtitle(track.subtitle())
                    .leading(art(&track.id, image, 44., 10., dark))
                    .trailing(marker),
            );
        }
        for (index, track) in queue.iter().enumerate() {
            let image = self.cover(track.thumbnail.as_deref(), cx);
            let model = self.model.clone();
            let label = match track.duration {
                Some(_) => format!("{} · {}", track.artist, clock_text(track.secs())),
                None => track.artist.clone(),
            };
            rows = rows.child(
                ListItem::new(("queue-row", index), track.title.clone())
                    .subtitle(label)
                    .leading(art(&track.id, image, 44., 10., dark))
                    .on_click(move |_, _, cx| {
                        model.update(cx, |model, cx| model.play_queue_item(index, cx))
                    }),
            );
        }
        let empty = current.is_none() && queue.is_empty();

        SideSheet::standard("queue-sheet")
            .width(px(WIDTH))
            .title("Cola")
            .on_close(click(&self.model, |model, cx| {
                model.queue_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .pb(px(8.))
                    .px(px(12.))
                    .type_style(BODY_MEDIUM)
                    .text_color(s.on_surface_variant)
                    .child(if empty {
                        "La cola está vacía".to_string()
                    } else if playing {
                        format!(
                            "{} en cola · {} min · sonando",
                            queue.len(),
                            total.as_secs() / 60
                        )
                    } else {
                        format!("{} en cola · {} min", queue.len(), total.as_secs() / 60)
                    }),
            )
            .child(if empty {
                div()
                    .flex()
                    .h(px(200.))
                    .items_center()
                    .justify_center()
                    .px(px(24.))
                    .text_size(px(13.))
                    .text_color(s.on_surface_variant)
                    .child("Toca una canción y aquí verás lo que sigue.")
                    .into_any_element()
            } else {
                rows.into_any_element()
            })
            // "Vaciar" leaves the undo next to it, so a mistake is one click away from being
            // fixed.
            .when(self.undo.is_some(), |sheet| {
                sheet.action(
                    Button::new("queue-undo", "Deshacer")
                        .text()
                        .on_click({
                            let panel = cx.entity();
                            move |_, _, cx| {
                                panel.update(cx, |panel, cx| panel.undo_clear(cx));
                            }
                        }),
                )
            })
            .when(self.undo.is_none() && !queue.is_empty(), |sheet| {
                sheet.action(
                    Button::new("queue-clear", "Vaciar")
                        .text()
                        .on_click({
                            let panel = cx.entity();
                            move |_, _, cx| {
                                panel.update(cx, |panel, cx| panel.clear(cx));
                            }
                        }),
                )
            })
    }
}
