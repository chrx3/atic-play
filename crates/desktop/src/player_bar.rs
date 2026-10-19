use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, ClickEvent, Context, Entity, EventEmitter, FontWeight, MouseButton, MouseDownEvent,
    Pixels, Subscription, Window, canvas, div, prelude::*, px,
};
use gpui_m3::{IconButton, LoadingIndicator, Shape, ShapeName, Slider, Theme, WavyProgress};
use ytm_core::RepeatMode;

use crate::assets;
use crate::covers::Covers;
use crate::playback::Playback;
use crate::ui::format_time;

const COVER_SIZE: u32 = 120;
const SIDE_WIDTH: f32 = 300.;

pub enum PlayerBarEvent {
    ToggleQueue,
}

/// Bottom bar: current track, transport controls, seekable progress and volume.
///
/// Its own view, because the wave animates every frame and should not redraw the rest of the app.
pub struct PlayerBar {
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    queue_open: bool,
    /// Where the progress wave was laid out, to turn a click into a position.
    progress_bounds: Rc<Cell<Bounds<Pixels>>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PlayerBarEvent> for PlayerBar {}

impl PlayerBar {
    pub fn new(playback: Entity<Playback>, covers: Entity<Covers>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe(&playback, |_, _, cx| cx.notify()),
            cx.observe(&covers, |_, _, cx| cx.notify()),
        ];
        PlayerBar {
            playback,
            covers,
            queue_open: true,
            progress_bounds: Rc::default(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_queue_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.queue_open = open;
        cx.notify();
    }

    fn seek_to_click(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let bounds = self.progress_bounds.get();
        if bounds.size.width <= px(0.) {
            return;
        }
        let fraction = (event.position.x - bounds.origin.x) / bounds.size.width;
        self.playback
            .update(cx, |playback, cx| playback.seek_fraction(fraction, cx));
    }

    fn render_track(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let playback = self.playback.read(cx);
        let track = playback.current().cloned();
        let cover = track.as_ref().and_then(|track| {
            self.covers.update(cx, |covers, cx| {
                covers.get(&track.id, track.cover_url.as_deref(), COVER_SIZE, cx)
            })
        });

        let shape = Shape::new(ShapeName::Cookie9).size(px(56.));
        let shape = match cover {
            Some(path) => shape.image(path.as_path()),
            None => shape.color(s.surface_container_highest),
        };
        div()
            .w(px(SIDE_WIDTH))
            .flex()
            .items_center()
            .gap_3()
            .child(shape)
            .children(track.map(|track| {
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(track.title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(s.on_surface_variant)
                            .truncate()
                            .child(track.artists),
                    )
            }))
    }

    fn render_transport(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let playback = self.playback.read(cx);
        let has_track = playback.current().is_some();
        let loading = playback.is_loading();
        let playing = playback.is_playing();
        let shuffle = playback.queue().shuffle();
        let repeat = playback.queue().repeat();
        let position = playback.position();
        let duration = playback.duration();
        let progress = duration.map(|duration| {
            (position.as_secs_f32() / duration.as_secs_f32().max(1.)).clamp(0., 1.)
        });

        let play_button = if loading {
            div()
                .size(px(48.))
                .flex()
                .items_center()
                .justify_center()
                .child(LoadingIndicator::new().size(px(40.)))
                .into_any_element()
        } else {
            IconButton::new(
                "play-pause",
                if playing { assets::PAUSE } else { assets::PLAY },
            )
            .filled()
            .hover_morph(true)
            .size(px(48.))
            .disabled(!has_track)
            .tooltip(if playing { "Pausar" } else { "Reproducir" })
            .on_click(self.on_playback(|playback, cx| playback.toggle_pause(cx), cx))
            .into_any_element()
        };

        let progress_bounds = self.progress_bounds.clone();
        div()
            .flex_1()
            .max_w(px(720.))
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        IconButton::new("shuffle", assets::SHUFFLE)
                            .selected(shuffle)
                            .tooltip(if shuffle {
                                "Desactivar aleatorio"
                            } else {
                                "Aleatorio"
                            })
                            .on_click(
                                self.on_playback(|playback, cx| playback.toggle_shuffle(cx), cx),
                            ),
                    )
                    .child(
                        IconButton::new("previous", assets::SKIP_PREVIOUS)
                            .disabled(!has_track)
                            .tooltip("Anterior")
                            .on_click(self.on_playback(|playback, cx| playback.previous(cx), cx)),
                    )
                    .child(play_button)
                    .child(
                        IconButton::new("next", assets::SKIP_NEXT)
                            .disabled(!has_track)
                            .tooltip("Siguiente")
                            .on_click(self.on_playback(|playback, cx| playback.next(cx), cx)),
                    )
                    .child(
                        IconButton::new(
                            "repeat",
                            if repeat == RepeatMode::One {
                                assets::REPEAT_ONE
                            } else {
                                assets::REPEAT
                            },
                        )
                        .selected(repeat != RepeatMode::Off)
                        .tooltip(match repeat {
                            RepeatMode::Off => "Repetir todo",
                            RepeatMode::All => "Repetir una",
                            RepeatMode::One => "No repetir",
                        })
                        .on_click(self.on_playback(|playback, cx| playback.cycle_repeat(cx), cx)),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_xs()
                    .text_color(s.on_surface_variant)
                    .child(
                        div()
                            .w(px(40.))
                            .flex()
                            .justify_end()
                            .child(format_time(position.as_secs())),
                    )
                    .child(
                        div()
                            .id("seek")
                            .relative()
                            .flex_1()
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .when(duration.is_some(), |bar| bar.cursor_pointer())
                            .child(WavyProgress::new("progress", progress.or(Some(0.))))
                            .child(
                                canvas(
                                    move |bounds, _, _| progress_bounds.set(bounds),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.seek_to_click(event, cx)
                                }),
                            ),
                    )
                    .child(
                        div().w(px(40.)).child(
                            duration
                                .map(|d| format_time(d.as_secs()))
                                .unwrap_or_default(),
                        ),
                    ),
            )
    }

    fn render_volume(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let playback = self.playback.read(cx);
        let muted = playback.is_muted() || playback.volume() == 0.;
        let volume = if playback.is_muted() {
            0.
        } else {
            playback.volume()
        };
        let entity = self.playback.downgrade();

        div()
            .w(px(SIDE_WIDTH))
            .flex()
            .items_center()
            .justify_end()
            .gap_2()
            .child(
                IconButton::new(
                    "mute",
                    if muted {
                        assets::VOLUME_OFF
                    } else {
                        assets::VOLUME
                    },
                )
                .tooltip(if muted { "Activar sonido" } else { "Silenciar" })
                .on_click(self.on_playback(|playback, cx| playback.toggle_mute(cx), cx)),
            )
            .child(
                Slider::new("volume", volume, 0.0..=1.0)
                    .width(px(120.))
                    .on_change(move |value, _, cx| {
                        entity
                            .update(cx, |playback, cx| playback.set_volume(value, cx))
                            .ok();
                    }),
            )
            .child(
                IconButton::new("queue", assets::QUEUE)
                    .selected(self.queue_open)
                    .tooltip(if self.queue_open {
                        "Ocultar cola"
                    } else {
                        "Mostrar cola"
                    })
                    .on_click(
                        cx.listener(|_, _: &ClickEvent, _, cx| {
                            cx.emit(PlayerBarEvent::ToggleQueue)
                        }),
                    ),
            )
    }

    /// A click handler that runs `action` on the playback model.
    fn on_playback(
        &self,
        action: impl Fn(&mut Playback, &mut Context<Playback>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static {
        let playback = self.playback.clone();
        cx.listener(move |_, _: &ClickEvent, _, cx| {
            playback.update(cx, |playback, cx| action(playback, cx));
        })
    }
}

impl Render for PlayerBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let error = self.playback.read(cx).error().cloned();
        div()
            .flex()
            .flex_col()
            .children(error.map(|error| {
                div()
                    .px_4()
                    .py_1()
                    .text_sm()
                    .text_color(s.error)
                    .truncate()
                    .child(error)
            }))
            .child(
                div()
                    .h(px(88.))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(self.render_track(cx))
                    .child(self.render_transport(cx))
                    .child(self.render_volume(cx)),
            )
    }
}
