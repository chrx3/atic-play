use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, ClickEvent, Context, Entity, EventEmitter, FontWeight, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, Subscription, Window, canvas,
    div, prelude::*, px,
};
use gpui_m3::{IconButton, LoadingIndicator, Shape, ShapeName, Slider, Theme, Tooltip};
use ytm_core::RepeatMode;

use crate::account::Account;
use crate::assets;
use crate::audio_wave::AudioWave;
use crate::covers::Covers;
use crate::playback::Playback;
use crate::ui::{format_time, like_button, track_cover};

const COVER_SIZE: u32 = 120;
const SIDE_WIDTH: f32 = 300.;
/// What one notch of the mouse wheel changes the volume by.
const WHEEL_VOLUME_STEP: f32 = 0.05;
/// Half the width of the time shown over the progress wave, to center it on the pointer.
const PREVIEW_HALF_WIDTH: f32 = 22.;

pub enum PlayerBarEvent {
    ToggleQueue,
    ToggleNowPlaying,
}

/// Bottom bar: current track, transport controls, seekable progress and volume.
///
/// Its own view, because the wave animates every frame and should not redraw the rest of the app.
pub struct PlayerBar {
    playback: Entity<Playback>,
    covers: Entity<Covers>,
    account: Entity<Account>,
    queue_open: bool,
    now_playing_open: bool,
    /// Where the progress wave was laid out, to turn the pointer into a position.
    progress_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// While the progress is dragged: where it will seek to (0..1) when released.
    scrub: Option<f32>,
    /// Where the pointer is over the progress (0..1), to show the time there.
    hover: Option<f32>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PlayerBarEvent> for PlayerBar {}

impl PlayerBar {
    pub fn new(
        playback: Entity<Playback>,
        covers: Entity<Covers>,
        account: Entity<Account>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(&playback, |_, _, cx| cx.notify()),
            cx.observe(&covers, |_, _, cx| cx.notify()),
            // The like button follows the session and the liked songs.
            cx.observe(&account, |_, _, cx| cx.notify()),
        ];
        PlayerBar {
            playback,
            covers,
            account,
            queue_open: true,
            now_playing_open: false,
            progress_bounds: Rc::default(),
            scrub: None,
            hover: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_queue_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.queue_open = open;
        cx.notify();
    }

    pub fn set_now_playing_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.now_playing_open = open;
        cx.notify();
    }

    /// The pointer pressed, moved or was released over (or, while dragging, anywhere around) the
    /// progress wave.
    fn pointer_on_progress(
        &mut self,
        position: Point<Pixels>,
        pressed: bool,
        released: bool,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.progress_bounds.get();
        if bounds.size.width <= px(0.) {
            return;
        }
        let fraction = ((position.x - bounds.origin.x) / bounds.size.width).clamp(0., 1.);
        let hover = bounds.contains(&position).then_some(fraction);
        let scrub = if released {
            if self.scrub.is_some() {
                self.playback
                    .update(cx, |playback, cx| playback.seek_fraction(fraction, cx));
            }
            None
        } else if pressed && (self.scrub.is_some() || hover.is_some()) {
            Some(fraction)
        } else {
            None
        };
        if (scrub, hover) != (self.scrub, self.hover) {
            self.scrub = scrub;
            self.hover = hover;
            cx.notify();
        }
    }

    fn render_track(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let playback = self.playback.read(cx);
        let track = playback.current().cloned();
        let cover = track.as_ref().and_then(|track| {
            self.covers.update(cx, |covers, cx| {
                covers.get(&track.id, track.cover_url.as_deref(), COVER_SIZE, cx)
            })
        });

        let shapes = (ShapeName::Cookie9, ShapeName::Clover8);
        let shape = match &track {
            Some(track) => track_cover("bar", &track.id, cover, shapes, 56., window, cx),
            None => Shape::new(shapes.0)
                .size(px(56.))
                .color(s.surface_container_highest),
        };
        let has_track = track.is_some();
        div()
            .w(px(SIDE_WIDTH))
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .id("now-playing-cover")
                    .flex_none()
                    .when(has_track, |cover| {
                        cover
                            .cursor_pointer()
                            .tooltip(Tooltip::text(if self.now_playing_open {
                                "Cerrar Reproduciendo"
                            } else {
                                "Abrir Reproduciendo"
                            }))
                            .on_click(cx.listener(|_, _: &ClickEvent, _, cx| {
                                cx.emit(PlayerBarEvent::ToggleNowPlaying)
                            }))
                    })
                    .child(shape),
            )
            .children(track.clone().map(|track| {
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
            .children(
                track.and_then(|track| like_button("like-current", &track, &self.account, cx)),
            )
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
        let level = playback.audio_level();
        let progress = duration.map(|duration| {
            (position.as_secs_f32() / duration.as_secs_f32().max(1.)).clamp(0., 1.)
        });
        // While dragging, the wave and the time follow the pointer; the seek happens on release.
        let scrub = self.scrub.filter(|_| duration.is_some());
        let progress = scrub.or(progress);
        let shown_position = match (scrub, duration) {
            (Some(scrub), Some(duration)) => duration.mul_f32(scrub),
            _ => position,
        };
        let preview = self
            .scrub
            .or(self.hover)
            .zip(duration)
            .map(|(fraction, duration)| {
                let width = f32::from(self.progress_bounds.get().size.width);
                (fraction * width, duration.mul_f32(fraction))
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
        // Only a song with a known length can be sought.
        let bar = duration.map(|_| cx.entity().downgrade());
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
                            .child(format_time(shown_position.as_secs())),
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
                            .child(AudioWave::new("progress", progress.unwrap_or(0.), level))
                            .child(
                                canvas(
                                    move |bounds, _, _| progress_bounds.set(bounds),
                                    move |_, _, window, _| {
                                        // Listened to on the whole window, so a drag keeps
                                        // going outside the wave.
                                        let Some(bar) = bar.clone() else {
                                            return;
                                        };
                                        let (bar_moving, bar_released) = (bar.clone(), bar.clone());
                                        window.on_mouse_event(
                                            move |event: &MouseDownEvent, phase, _, cx| {
                                                if phase.bubble()
                                                    && event.button == MouseButton::Left
                                                {
                                                    bar.update(cx, |bar, cx| {
                                                        bar.pointer_on_progress(
                                                            event.position,
                                                            true,
                                                            false,
                                                            cx,
                                                        )
                                                    })
                                                    .ok();
                                                }
                                            },
                                        );
                                        let bar = bar_moving;
                                        window.on_mouse_event(
                                            move |event: &MouseMoveEvent, phase, _, cx| {
                                                if phase.bubble() {
                                                    let pressed = event.pressed_button
                                                        == Some(MouseButton::Left);
                                                    bar.update(cx, |bar, cx| {
                                                        bar.pointer_on_progress(
                                                            event.position,
                                                            pressed,
                                                            false,
                                                            cx,
                                                        )
                                                    })
                                                    .ok();
                                                }
                                            },
                                        );
                                        let bar = bar_released;
                                        window.on_mouse_event(
                                            move |event: &MouseUpEvent, phase, _, cx| {
                                                if phase.bubble()
                                                    && event.button == MouseButton::Left
                                                {
                                                    bar.update(cx, |bar, cx| {
                                                        bar.pointer_on_progress(
                                                            event.position,
                                                            false,
                                                            true,
                                                            cx,
                                                        )
                                                    })
                                                    .ok();
                                                }
                                            },
                                        );
                                    },
                                )
                                .absolute()
                                .size_full(),
                            )
                            .children(preview.map(|(x, time)| {
                                div()
                                    .absolute()
                                    .bottom(px(26.))
                                    .left(px(x - PREVIEW_HALF_WIDTH))
                                    .w(px(PREVIEW_HALF_WIDTH * 2.))
                                    .py_0p5()
                                    .flex()
                                    .justify_center()
                                    .rounded(px(6.))
                                    .bg(s.on_surface)
                                    .text_color(s.surface)
                                    .child(format_time(time.as_secs()))
                            })),
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
                div()
                    .id("volume-area")
                    // The wheel turns the volume, like most players.
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                        let delta = event.delta.pixel_delta(px(20.)).y;
                        if delta == px(0.) {
                            return;
                        }
                        let step = if delta > px(0.) {
                            WHEEL_VOLUME_STEP
                        } else {
                            -WHEEL_VOLUME_STEP
                        };
                        this.playback.update(cx, |playback, cx| {
                            let volume = if playback.is_muted() {
                                0.
                            } else {
                                playback.volume()
                            };
                            playback.set_volume(volume + step, cx)
                        });
                    }))
                    .child(
                        Slider::new("volume", volume, 0.0..=1.0)
                            .width(px(120.))
                            .on_change(move |value, _, cx| {
                                entity
                                    .update(cx, |playback, cx| playback.set_volume(value, cx))
                                    .ok();
                            }),
                    ),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(self.render_track(window, cx))
                    .child(self.render_transport(cx))
                    .child(self.render_volume(cx)),
            )
    }
}
