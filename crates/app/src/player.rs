//! The mini-player bar and the "Reproduciendo" sheet, which also holds the lyrics.
//!
//! Both are small views of their own: they watch [`Clock`](crate::model::Clock) and redraw every
//! tick, leaving the rest of the app alone.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, Context, Entity, Font, FontFeatures, FontStyle, FontWeight, Hsla,
    InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, linear_color_stop,
    linear_gradient, point, px, relative,
    prelude::FluentBuilder as _,
};
use gpui_m3::{
    Icon, IconButton, ListItem, LoadingIndicator, M3ButtonSize, Scheme, Segment, SegmentedButtons,
    Shape, ShapeName, Slider, SliderSize, Theme, WavyProgress,
    motion::{animate_color, animate_f32, replay},
    theme::{
        alpha,
        type_scale::{BODY_MEDIUM, HEADLINE_SMALL, TITLE_MEDIUM, TypeScaleExt},
    },
};

use crate::covers::CoverCache;
use crate::icons::icon;
use crate::model::{Clock, LyricsState, Model, RepeatMode, line_at};
use crate::motion::{CALM, CALM_SECS, CALM_SPLIT, calm_ease};
use crate::page::art;
use crate::source::{Track, clock_text};

/// How often the engine is told about a new position while the user drags the seek bar.
const SEEK_THROTTLE: Duration = Duration::from_millis(100);

/// The bar at the bottom of the window: what plays, and the basic transport.
pub struct MiniPlayer {
    model: Entity<Model>,
    clock: Entity<Clock>,
    covers: Entity<CoverCache>,
    /// How wide the bar was the last time it was laid out: what decides what fits in it.
    width: Rc<Cell<f32>>,
    /// Where the volume popover is, to know when a click lands inside it.
    popover: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Whether the volume popover is open (in narrow windows).
    volume_open: bool,
}

impl MiniPlayer {
    pub fn new(
        model: Entity<Model>,
        clock: Entity<Clock>,
        covers: Entity<CoverCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&clock, |_, _, cx| cx.notify()).detach();
        cx.observe(&covers, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            clock,
            covers,
            width: Rc::new(Cell::new(0.)),
            popover: Rc::new(Cell::new(None)),
            volume_open: false,
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
}

impl Render for MiniPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let (track, playing, loading, volume, shuffle, repeat, dark) = {
            let model = self.model.read(cx);
            (
                model.current.clone(),
                model.playing,
                model.loading_audio,
                model.volume,
                model.shuffle,
                model.repeat,
                model.dark,
            )
        };
        let clock = {
            let clock = self.clock.read(cx);
            (clock.position, clock.duration)
        };
        let model = self.model.clone();
        let duration = if clock.1 > 0. {
            clock.1
        } else {
            track.as_ref().map_or(0., Track::secs)
        };
        let fraction = if duration > 0. {
            (clock.0 / duration).clamp(0., 1.)
        } else {
            0.
        };
        let has_track = track.is_some();
        // The bar measures itself: what fits in it depends on how much room there is.
        let width = match self.width.get() {
            width if width > 0. => width,
            // The first frame: guess from the window, so the bar does not flash narrow.
            _ => f32::from(window.viewport_size().width) - 240.,
        };
        let layout = MiniLayout::for_width(width);

        let artwork = match &track {
            Some(track) => art(
                &track.id,
                self.cover(track.thumbnail.as_deref(), cx),
                52.,
                26.,
                dark,
            ),
            None => art("silence", None, 52., 26., dark),
        };
        let title = track
            .as_ref()
            .map_or_else(|| "Nada sonando".to_string(), |t| t.title.clone());
        let subtitle = track
            .as_ref()
            .map_or_else(|| "Elige algo en Inicio".to_string(), Track::subtitle);

        let info = div()
            .id("mini-info")
            .flex()
            .items_center()
            .gap(px(12.))
            .flex_shrink()
            .min_w(px(124.))
            .max_w(px(230.))
            .cursor_pointer()
            .on_click({
                let model = model.clone();
                move |_, _, cx| {
                    model.update(cx, |model, cx| {
                        model.now_open = true;
                        cx.notify();
                    })
                }
            })
            .child(artwork)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .type_style(TITLE_MEDIUM)
                            .text_size(px(14.))
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(s.on_surface_variant)
                            .truncate()
                            .child(subtitle),
                    ),
            );

        let mut controls = div().flex().items_center().gap(px(6.));
        if layout.extra {
            controls = controls.child(
                IconButton::new("mini-shuffle", icon("shuffle"))
                    .when(shuffle, |button| button.tonal())
                    .size(px(40.))
                    .disabled(!has_track)
                    .tooltip(if shuffle { "Aleatorio activo" } else { "Aleatorio" })
                    .on_click(crate::model::click(&model, |model, cx| {
                        model.toggle_shuffle(cx)
                    })),
            );
        }
        controls = controls
            .child(
                IconButton::new("mini-prev", icon("previous"))
                    .size(px(40.))
                    .disabled(!has_track)
                    .tooltip("Anterior")
                    .on_click(crate::model::click(&model, |model, cx| model.previous(cx))),
            )
            .child(play_button("mini", playing, loading, &model, window, cx))
            .child(
                IconButton::new("mini-next", icon("next"))
                    .size(px(40.))
                    .disabled(!has_track)
                    .tooltip("Siguiente")
                    .on_click(crate::model::click(&model, |model, cx| model.next(cx))),
            );
        if layout.extra {
            controls = controls.child(
                IconButton::new(
                    "mini-repeat",
                    icon(if repeat == RepeatMode::One {
                        "repeat-one"
                    } else {
                        "repeat"
                    }),
                )
                .when(repeat != RepeatMode::Off, |button| button.tonal())
                .size(px(40.))
                .disabled(!has_track)
                .tooltip(match repeat {
                    RepeatMode::Off => "Repetir",
                    RepeatMode::All => "Repetiendo todo",
                    RepeatMode::One => "Repetiendo una",
                })
                .on_click(crate::model::click(&model, |model, cx| {
                    model.cycle_repeat(cx)
                })),
            );
        }

        // The progress: the groove is always visible, the wave only while it plays.
        let wave = animate_f32(
            "mini-wave",
            CALM,
            if playing { 1. } else { 0. },
            window,
            cx,
        );
        // The wave flattens as it goes, the way it does in M3 Expressive.
        let amplitude = animate_f32(
            "mini-wave-amp",
            CALM,
            if playing { WavyProgress::DEFAULT_AMPLITUDE } else { 0. },
            window,
            cx,
        );
        let progress = div()
            .flex()
            .flex_1()
            .min_w(px(96.))
            .items_center()
            .gap(px(8.))
            .child(time_label(clock.0, false))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h(px(12.))
                    .child(
                        div()
                            .absolute()
                            .inset(px(0.))
                            .opacity(1. - wave)
                            .child(progress_track(fraction, &s)),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset(px(0.))
                            .opacity(wave)
                            .child(
                                WavyProgress::new("mini-wave-bar", Some(fraction))
                                    .amplitude(amplitude),
                            ),
                    ),
            )
            .when(layout.total, |d| d.child(duration_label(duration, true)));

        let volume_box = match layout.volume {
            Volume::Slider(width) => div()
                .id("mini-volume")
                .flex()
                .items_center()
                .gap(px(8.))
                .child(volume_icon(volume, &s))
                .child(
                    Slider::new("mini-volume-slider", volume, 0.0..=100.)
                        .size(SliderSize::S)
                        .width(px(width))
                        .on_change(crate::model::change_volume(&model)),
                ),
            Volume::Button => {
                let panel = cx.entity();
                div()
                    .id("mini-volume")
                    .relative()
                    .child(
                        IconButton::new("mini-volume-button", icon(volume_icon_name(volume)))
                            .size(px(40.))
                            .tooltip("Volumen")
                            .on_click({
                                let panel = panel.clone();
                                move |_, _, cx| {
                                    panel.update(cx, |panel, cx| {
                                        panel.volume_open = !panel.volume_open;
                                        cx.notify();
                                    })
                                }
                            }),
                    )
            }
        };

        // The popover lives outside the pill: the pill clips whatever does not fit in it.
        let popover = match layout.volume {
            Volume::Button if self.volume_open => Some(
                div()
                    .absolute()
                    .bottom(px(84.))
                    .right(px(0.))
                    .w(px(180.))
                    .p(px(14.))
                    .rounded(px(16.))
                    .bg(s.surface_container_highest)
                    .shadow(gpui_m3::Elevation::Level2.shadow(&s))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    // The popover leaves its own bounds behind: a click inside it is not a
                    // click outside.
                    .child({
                        let popover = self.popover.clone();
                        div().absolute().inset(px(0.)).child(canvas(
                            move |here, _, _| {
                                popover.set(Some(here));
                            },
                            |_, _, _, _| {},
                        ))
                    })
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(s.on_surface_variant)
                            .child(format!("Volumen {volume:.0}%")),
                    )
                    .child(
                        Slider::new("mini-volume-pop", volume, 0.0..=100.)
                            .size(SliderSize::S)
                            .width(px(152.))
                            .on_change(crate::model::change_volume(&model)),
                    ),
            ),
            _ => None,
        };

        // The bar measures itself: the canvas fills it and leaves its width behind.
        let measure = {
            let width = self.width.clone();
            div().absolute().inset(px(0.)).child(canvas(
                move |bounds, _, _| {
                    width.set(f32::from(bounds.size.width));
                },
                |_, _, _, _| {},
            ))
        };

        div()
            .relative()
            .w_full()
            .on_mouse_down_out({
                let panel = cx.entity();
                move |_, window, cx| {
                    // A click inside the popover is not a click outside.
                    let inside = panel
                        .read(cx)
                        .popover
                        .get()
                        .is_some_and(|bounds| bounds.contains(&window.mouse_position()));
                    if inside {
                        return;
                    }
                    panel.update(cx, |panel, cx| {
                        if panel.volume_open {
                            panel.volume_open = false;
                            cx.notify();
                        }
                    })
                }
            })
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .w_full()
                    .h(px(72.))
                    .pl(px(10.))
                    .pr(px(20.))
                    .rounded(px(36.))
                    .bg(s.surface_container_high)
                    .shadow(gpui_m3::Elevation::Level3.shadow(&s))
                    .overflow_hidden()
                    .text_color(s.on_surface)
                    .child(measure)
                    .child(info)
                    .child(controls)
                    .child(progress)
                    .child(volume_box),
            )
            .when_some(popover, |d, popover| d.child(popover))
    }
}

/// What fits in the bar at a given width, in the order things give up their space.
struct MiniLayout {
    /// Whether the shuffle and repeat buttons are there.
    extra: bool,
    /// How the volume is shown.
    volume: Volume,
    /// Whether the total time is shown.
    total: bool,
}

/// The volume control of the bar: a slider of the given width, or just a button.
enum Volume {
    Slider(f32),
    Button,
}

impl MiniLayout {
    /// The layout for a bar of `width` pixels. The volume gives up its space first, then the
    /// shuffle and repeat buttons, and the total time is the last to go.
    fn for_width(width: f32) -> Self {
        if width >= 740. {
            Self {
                extra: true,
                volume: Volume::Slider(96.),
                total: true,
            }
        } else if width >= 700. {
            Self {
                extra: true,
                volume: Volume::Slider(64.),
                total: true,
            }
        } else if width >= 640. {
            Self {
                extra: true,
                volume: Volume::Button,
                total: true,
            }
        } else if width >= 560. {
            Self {
                extra: false,
                volume: Volume::Button,
                total: true,
            }
        } else {
            Self {
                extra: false,
                volume: Volume::Button,
                total: false,
            }
        }
    }
}

pub struct NowPlaying {
    model: Entity<Model>,
    clock: Entity<Clock>,
    covers: Entity<CoverCache>,
    tab: usize,
    /// Bumped when the tab changes, to play the transition.
    tab_generation: u64,
    lyrics_scroll: ScrollHandle,
    /// Where the lyrics should be scrolled to, in pixels.
    lyrics_target: f32,
    /// The line being sung, to know when to scroll.
    last_line: Option<usize>,
    /// Bumped when the user scrolls the lyrics by hand, to start the follow over.
    scroll_generation: u64,
    /// Whether the user scrolled the lyrics by hand.
    manual_scroll: bool,
    /// Where the user is dragging the seek bar to.
    seek_value: Option<f32>,
    /// When the engine was last told about a new position.
    last_seek: Instant,
    /// Bumped on every move of the seek bar, to drop the stale trailing seeks.
    seek_generation: u64,
}

impl NowPlaying {
    pub fn new(
        model: Entity<Model>,
        clock: Entity<Clock>,
        covers: Entity<CoverCache>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&clock, |_, _, cx| cx.notify()).detach();
        cx.observe(&covers, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            clock,
            covers,
            tab: 0,
            tab_generation: 0,
            lyrics_scroll: ScrollHandle::new(),
            lyrics_target: 0.,
            last_line: None,
            scroll_generation: 0,
            manual_scroll: false,
            seek_value: None,
            last_seek: Instant::now(),
            seek_generation: 0,
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

    /// Development hook (`ATIC_PLAY_OPEN=lyrics`): opens the sheet on the lyrics tab.
    pub fn show_lyrics(&mut self, cx: &mut Context<Self>) {
        self.tab = 1;
        self.tab_generation += 1;
        self.last_line = None;
        cx.notify();
    }

    /// Where the lyrics should be to sing the line `index`: it sits in the middle.
    fn scroll_to_line(&mut self, index: usize) -> bool {
        let Some(bounds) = self.lyrics_scroll.bounds_for_item(index) else {
            return false;
        };
        let viewport = self.lyrics_scroll.bounds();
        if f32::from(viewport.size.height) <= 0. {
            return false;
        }
        // The line is drawn at `bounds.top() + offset`, so this is where the offset has to be
        // for it to land in the middle of the panel.
        let delta = f32::from(
            (viewport.top() - bounds.top()) + (viewport.size.height - bounds.size.height) * 0.5,
        );
        let max = f32::from(self.lyrics_scroll.max_offset().height).max(0.);
        let target = delta.clamp(-max, 0.);
        if (target - f32::from(self.lyrics_scroll.offset().y)).abs() < 1. {
            return true;
        }
        self.lyrics_target = target;
        true
    }

    /// A move of the seek bar: the position follows the mouse, but the engine only hears about
    /// it every 100 ms, and the last value is always sent.
    fn drag_seek(&mut self, value: f32, cx: &mut Context<Self>) {
        self.seek_value = Some(value);
        self.seek_generation += 1;
        let generation = self.seek_generation;
        let now = Instant::now();
        if now.duration_since(self.last_seek) >= SEEK_THROTTLE {
            self.last_seek = now;
            self.seek_value = None;
            self.apply_seek(value, cx);
        }
        // Whatever the last move was, it reaches the engine a moment later.
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEEK_THROTTLE).await;
            this.update(cx, |view, cx| {
                if view.seek_generation == generation {
                    view.commit_seek(cx);
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The user let go of the seek bar: the engine gets the position right away.
    fn commit_seek(&mut self, cx: &mut Context<Self>) {
        let Some(value) = self.seek_value.take() else {
            return;
        };
        self.last_seek = Instant::now();
        self.apply_seek(value, cx);
        cx.notify();
    }

    fn apply_seek(&mut self, value: f32, cx: &mut Context<Self>) {
        let model = self.model.clone();
        model.update(cx, |model, cx| model.seek(value, cx));
    }

    fn lyrics(&mut self, s: &Scheme, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.model.read(cx).lyrics.clone();
        let position = Duration::from_secs_f32(self.clock.read(cx).position.max(0.));
        let content: AnyElement = match state {
            LyricsState::Idle | LyricsState::Loading => div()
                .flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .child(LoadingIndicator::new().size(px(20.)))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(s.on_surface_variant)
                        .child("Buscando la letra…"),
                )
                .into_any_element(),
            LyricsState::None => note(s, "Sin letra para esta canción.").into_any_element(),
            LyricsState::Failed(err) => note(s, err.as_ref()).into_any_element(),
            LyricsState::Ready(lyrics) => match &*lyrics {
                play_lyrics::Lyrics::Instrumental => {
                    note(s, "Es instrumental: no hay letra que cantar.").into_any_element()
                }
                play_lyrics::Lyrics::Plain(text) => {
                    let mut body = div().flex().flex_col().gap(px(10.));
                    for line in text.lines().filter(|line| !line.trim().is_empty()) {
                        body = body.child(
                            div()
                                .text_size(px(15.))
                                .text_color(s.on_surface)
                                .child(line.trim().to_string()),
                        );
                    }
                    div()
                        .id("lyrics-plain")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.lyrics_scroll)
                        .child(body)
                        .into_any_element()
                }
                play_lyrics::Lyrics::Synced(lines) => {
                    let active = line_at(lines, position);
                    // A scroll by hand wins: the follow waits for the next line.
                    if self.manual_scroll {
                        self.manual_scroll = false;
                        self.scroll_generation += 1;
                        self.last_line = active;
                        self.lyrics_target = f32::from(self.lyrics_scroll.offset().y);
                    }
                    if let Some(index) = active {
                        // The line is only followed when the panel knows where it is.
                        if self.last_line != Some(index) && self.scroll_to_line(index) {
                            self.last_line = Some(index);
                        }
                    }
                    // The follow is aligned to the frame, not to a timer.
                    let y = animate_f32(
                        ("lyrics-y", self.scroll_generation),
                        CALM,
                        self.lyrics_target,
                        window,
                        cx,
                    );
                    let x = f32::from(self.lyrics_scroll.offset().x);
                    self.lyrics_scroll.set_offset(point(px(x), px(y)));

                    let mut body = div().flex().flex_col().gap(px(12.)).py(px(120.));
                    for (index, line) in lines.iter().enumerate() {
                        let is_active = active == Some(index);
                        let color = animate_color(
                            ("lyric", index),
                            CALM,
                            if is_active {
                                s.on_surface
                            } else {
                                s.on_surface_variant
                            },
                            window,
                            cx,
                        );
                        body = body.child(
                            div()
                                .text_size(px(16.))
                                .font_weight(FontWeight(500.))
                                .text_color(color)
                                .child(line.text.clone()),
                        );
                    }
                    div()
                        .id("lyrics-synced")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.lyrics_scroll)
                        .on_scroll_wheel(cx.listener(|view, _: &gpui::ScrollWheelEvent, _, cx| {
                            // A manual scroll wins over the follow-the-song animation.
                            view.manual_scroll = true;
                            view.last_line = None;
                            cx.notify();
                        }))
                        .child(body)
                        .into_any_element()
                }
            },
        };
        // The panel: the lyrics fade into it at the top and at the bottom.
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .rounded(px(24.))
            .bg(s.surface_container)
            .p(px(16.))
            .relative()
            .child(content)
            .child(fade(s.surface_container, true))
            .child(fade(s.surface_container, false))
    }
}

impl Render for NowPlaying {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let (track, playing, loading, volume, shuffle, repeat, dark, queue) = {
            let model = self.model.read(cx);
            (
                model.current.clone(),
                model.playing,
                model.loading_audio,
                model.volume,
                model.shuffle,
                model.repeat,
                model.dark,
                model.queue.iter().take(6).cloned().collect::<Vec<Track>>(),
            )
        };
        let queue_len = self.model.read(cx).queue.len();
        let (position, duration) = {
            let clock = self.clock.read(cx);
            (clock.position, clock.duration)
        };
        let model = self.model.clone();
        let duration = if duration > 0. {
            duration
        } else {
            track.as_ref().map_or(0., Track::secs)
        };
        // While the user drags the bar, the position is the one under the mouse.
        let position = self.seek_value.unwrap_or(position).min(duration.max(0.5));
        let has_track = track.is_some();

        let artwork = match &track {
            Some(track) => art(
                &track.id,
                self.cover(track.thumbnail.as_deref(), cx),
                176.,
                88.,
                dark,
            ),
            None => art("silence", None, 176., 88., dark),
        };
        let title = track
            .as_ref()
            .map_or_else(|| "Nada sonando".to_string(), |t| t.title.clone());
        let subtitle = track
            .as_ref()
            .map_or_else(|| "Elige una canción para empezar".to_string(), Track::subtitle);

        let backdrop = Shape::new(ShapeName::Cookie12)
            .size(px(206.))
            .color(alpha(s.tertiary_container, 0.7));
        let backdrop = if playing { backdrop.spin(70.) } else { backdrop };
        let stage = div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h(px(206.))
            .child(backdrop)
            .child(div().absolute().child(artwork));

        let title_block = div()
            .flex()
            .flex_col()
            .pt(px(10.))
            .child(div().type_emphasized(HEADLINE_SMALL).child(title))
            .child(
                div()
                    .type_style(BODY_MEDIUM)
                    .text_color(s.on_surface_variant)
                    .child(subtitle),
            );

        // The heart of the song that is playing: right there, next to its name.
        let title_row = div()
            .flex()
            .items_start()
            .gap(px(8.))
            .child(div().flex_1().min_w_0().child(title_block))
            .child(crate::library::heart(
                &model,
                track.as_ref(),
                "now-like",
                40.,
                cx,
            ));

        let seek = div()
            .id("now-seek-box")
            .flex()
            .flex_col()
            .pt(px(4.))
            .on_mouse_up(MouseButton::Left, {
                let now = cx.entity();
                move |_, _, cx| {
                    now.update(cx, |view, cx| view.commit_seek(cx));
                }
            })
            .child(
                Slider::new("now-seek", position, 0.0..=duration.max(0.5))
                    .disabled(!has_track)
                    .width(px(340.))
                    .on_change({
                        let now = cx.entity();
                        move |value, _, cx| {
                            now.update(cx, |view, cx| view.drag_seek(value, cx));
                        }
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(px(12.))
                    .text_color(s.on_surface_variant)
                    .child(time_label(position, false))
                    .child(duration_label(duration, true)),
            );

        let transport = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(10.))
            .pt(px(8.))
            .child(
                IconButton::new("now-shuffle", icon("shuffle"))
                    .when(shuffle, |button| button.tonal())
                    .size(px(44.))
                    .disabled(!has_track)
                    .tooltip(if shuffle { "Aleatorio activo" } else { "Aleatorio" })
                    .on_click(crate::model::click(&model, |model, cx| {
                        model.toggle_shuffle(cx)
                    })),
            )
            .child(
                IconButton::new("now-prev", icon("previous"))
                    .size(px(44.))
                    .disabled(!has_track)
                    .tooltip("Anterior")
                    .on_click(crate::model::click(&model, |model, cx| model.previous(cx))),
            )
            .child(play_button("now", playing, loading, &model, window, cx))
            .child(
                IconButton::new("now-next", icon("next"))
                    .size(px(44.))
                    .disabled(!has_track)
                    .tooltip("Siguiente")
                    .on_click(crate::model::click(&model, |model, cx| model.next(cx))),
            )
            .child(
                IconButton::new(
                    "now-repeat",
                    icon(if repeat == RepeatMode::One {
                        "repeat-one"
                    } else {
                        "repeat"
                    }),
                )
                .when(repeat != RepeatMode::Off, |button| button.tonal())
                .size(px(44.))
                .disabled(!has_track)
                .tooltip(match repeat {
                    RepeatMode::Off => "Repetir",
                    RepeatMode::All => "Repetiendo todo",
                    RepeatMode::One => "Repetiendo una",
                })
                .on_click(crate::model::click(&model, |model, cx| {
                    model.cycle_repeat(cx)
                })),
            );

        let volume_row = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .pt(px(10.))
            .child(volume_icon(volume, &s))
            .child(
                Slider::new("now-volume", volume, 0.0..=100.)
                    .size(SliderSize::S)
                    .width(px(180.))
                    .on_change(crate::model::change_volume(&model)),
            );

        let left = div()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(340.))
            .child(stage)
            .child(title_row)
            .child(seek)
            .child(transport)
            .child(volume_row)
            .child(
                div()
                    .pt(px(6.))
                    .text_size(px(12.))
                    .text_color(s.on_surface_variant)
                    .child(format!("{queue_len} en la cola")),
            );

        let tabs = SegmentedButtons::new(
            "now-tabs",
            [Segment::label("Ahora"), Segment::label("Letra")],
            self.tab,
        )
        .on_change(cx.processor(|view, index: usize, _, cx| {
            view.tab = index;
            view.tab_generation += 1;
            view.last_line = None;
            cx.notify();
        }));

        // The tab change: the panel that arrives fades in and comes from below.
        let change = replay("now-tab", self.tab_generation, CALM_SECS, window, cx);
        let (_, arrive) = split_transition(change);

        let right = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(340.))
            .gap(px(12.))
            .child(tabs)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .opacity(arrive)
                    .top(px(-6. * (1. - arrive)))
                    .child(if self.tab == 1 {
                        self.lyrics(&s, window, cx).into_any_element()
                    } else {
                        let mut next = div().flex().flex_col().gap(px(2.));
                        for (index, track) in queue.iter().enumerate() {
                            let m = model.clone();
                            let image = self.cover(track.thumbnail.as_deref(), cx);
                            next = next.child(
                                ListItem::new(("next", index), track.title.clone())
                                    .subtitle(track.artist.clone())
                                    .leading(art(&track.id, image, 36., 8., dark))
                                    .trailing(crate::dialogs::song_menu_button(
                                        &model,
                                        track.clone(),
                                        SharedString::from(format!("next-menu-{index}")),
                                    ))
                                    .on_click(move |_, _, cx| {
                                        m.update(cx, |model, cx| model.play_queue_item(index, cx))
                                    }),
                            );
                        }
                        let body = if queue.is_empty() {
                            note(&s, "No hay nada más en la cola.").into_any_element()
                        } else {
                            next.into_any_element()
                        };
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .gap(px(10.))
                            .rounded(px(24.))
                            .bg(s.surface_container)
                            .p(px(16.))
                            .child(
                                div()
                                    .type_emphasized(TITLE_MEDIUM)
                                    .child("A continuación"),
                            )
                            .child(body)
                            .into_any_element()
                    }),
            );

        div()
            .flex()
            .gap(px(24.))
            .w_full()
            .child(left)
            .child(right)
    }
}

/// The play/pause button: the icon swaps with a crossfade and the spinner takes its place while
/// the audio loads. Everything lives in the same box, so nothing around it moves.
fn play_button(
    prefix: &'static str,
    playing: bool,
    loading: bool,
    model: &Entity<Model>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let icon_id: SharedString = format!("{prefix}-icon").into();
    let load_id: SharedString = format!("{prefix}-load").into();
    let play_id: SharedString = format!("{prefix}-play").into();
    let pause_id: SharedString = format!("{prefix}-pause").into();
    let icon_p = animate_f32(
        icon_id,
        CALM,
        if playing { 1. } else { 0. },
        window,
        cx,
    );
    let load_p = animate_f32(
        load_id,
        CALM,
        if loading { 1. } else { 0. },
        window,
        cx,
    );
    let label = if playing { "Pausar" } else { "Reproducir" };
    let button = |id: SharedString, name: &'static str| {
        IconButton::new(id, icon(name))
            .filled()
            .m3_size(M3ButtonSize::M)
            .tooltip(label)
    };
    let play_icon = div()
        .absolute()
        .opacity((1. - icon_p) * (1. - load_p))
        .child(button(play_id, "play"));
    let pause_icon = div()
        .absolute()
        .opacity(icon_p * (1. - load_p))
        .child(button(pause_id, "pause"));
    // The icon that shows is the one on top: the other one is invisible anyway.
    let (under, over) = if playing {
        (play_icon, pause_icon)
    } else {
        (pause_icon, play_icon)
    };
    div()
        .id(SharedString::from(format!("{prefix}-box")))
        .relative()
        .size(px(56.))
        .cursor_pointer()
        .on_click(crate::model::click(model, |model, cx| model.toggle_play(cx)))
        .child(under)
        .child(over)
        .child(
            div()
                .absolute()
                .size(px(56.))
                .flex()
                .items_center()
                .justify_center()
                .opacity(load_p)
                .child(LoadingIndicator::new().size(px(24.))),
        )
}

/// The groove of a progress bar: visible even in the light theme, with the part already played.
fn progress_track(fraction: f32, s: &Scheme) -> impl IntoElement {
    div()
        .relative()
        .h(px(12.))
        .w_full()
        .child(
            div()
                .absolute()
                .top(px(4.))
                .h(px(4.))
                .w_full()
                .rounded(px(2.))
                .bg(s.outline_variant),
        )
        .child(
            div()
                .absolute()
                .top(px(4.))
                .h(px(4.))
                .w(relative(fraction.clamp(0., 1.)))
                .rounded(px(2.))
                .bg(s.primary),
        )
}

/// A clock with tabular figures, so the numbers do not dance while they change.
fn time_label(secs: f32, right: bool) -> impl IntoElement {
    div()
        .w(px(38.))
        .font(tabular(FontWeight::NORMAL))
        .text_size(px(12.))
        .when(right, |d| d.text_right())
        .child(clock_text(secs.max(0.)))
}

/// The total time of a song: a dash while it is not known yet.
fn duration_label(secs: f32, right: bool) -> impl IntoElement {
    div()
        .w(px(38.))
        .font(tabular(FontWeight::NORMAL))
        .text_size(px(12.))
        .when(right, |d| d.text_right())
        .child(if secs > 0. { clock_text(secs) } else { "—:—".to_string() })
}

/// The font of the clocks: the same width for every digit.
fn tabular(weight: FontWeight) -> Font {
    Font {
        family: gpui_m3::theme::FONT_FAMILY.into(),
        features: FontFeatures(Arc::new(vec![("tnum".into(), 1)])),
        fallbacks: None,
        weight,
        style: FontStyle::Normal,
    }
}

/// The name of the volume icon, according to how loud it is.
fn volume_icon_name(volume: f32) -> &'static str {
    if volume <= 0. {
        "volume-off"
    } else if volume < 55. {
        "volume-low"
    } else {
        "volume"
    }
}

/// The volume icon, in the muted color of the scheme.
fn volume_icon(volume: f32, s: &Scheme) -> impl IntoElement {
    Icon::new(icon(volume_icon_name(volume)))
        .size(px(20.))
        .color(s.on_surface_variant)
}

/// The two halves of a timed transition: what leaves and what arrives.
fn split_transition(t: Option<f32>) -> (f32, f32) {
    let Some(t) = t else {
        return (1., 1.);
    };
    let p = (t / CALM_SECS).clamp(0., 1.);
    let ease = calm_ease();
    (
        ease((p / CALM_SPLIT).min(1.)),
        ease(((p - CALM_SPLIT) / (1. - CALM_SPLIT)).max(0.)),
    )
}

/// A band that fades the lyrics into the panel, at the top or at the bottom.
fn fade(color: Hsla, top: bool) -> impl IntoElement {
    // The gradient always points down: at the top it goes from the panel color to nothing, at
    // the bottom from nothing to the panel color.
    let (from, to) = if top {
        (color, alpha(color, 0.))
    } else {
        (alpha(color, 0.), color)
    };
    div()
        .absolute()
        .left_0()
        .right_0()
        .h(px(40.))
        .when(top, |d| d.top_0())
        .when(!top, |d| d.bottom_0())
        .bg(linear_gradient(
            180.,
            linear_color_stop(from, 0.),
            linear_color_stop(to, 1.),
        ))
}

fn note(s: &Scheme, text: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .text_size(px(13.5))
        .text_color(s.on_surface_variant)
        .child(text.to_string())
}
