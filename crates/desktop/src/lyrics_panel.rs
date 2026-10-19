use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    ClickEvent, Context, Entity, FontWeight, ScrollHandle, SharedString, Subscription, Window, div,
    point, prelude::*, px,
};
use gpui_m3::{Spring, Theme, motion};
use ytm_core::{Lyrics, LyricsClient, Track};

use crate::playback::Playback;
use crate::ui::{placeholder, skeleton};

/// Seeking lands just after the line starts, so the clicked line is the one highlighted.
const SEEK_LEAD: Duration = Duration::from_millis(50);

enum Load {
    Loading,
    Ready(Lyrics),
    Failed(SharedString),
}

/// The lyrics of the current track, from LRCLIB. Synced lyrics follow the song: the line being
/// sung stands out and stays centered, and clicking a line seeks to it.
pub struct LyricsPanel {
    playback: Entity<Playback>,
    /// `None` when its runtime could not start; the panel then says so.
    client: Option<LyricsClient>,
    /// Per track id, so going back to a song does not fetch its lyrics again.
    lyrics: HashMap<String, Load>,
    track: Option<Track>,
    active_line: Option<usize>,
    scroll: ScrollHandle,
    /// Bumped when the active line changes, to start a new scroll toward it.
    scroll_generation: usize,
    /// The scroll animation in flight: from and to offsets.
    scroll_motion: Option<(f32, f32)>,
    /// A scroll is due but the line positions are only known after the next layout.
    scroll_pending: bool,
    _subscriptions: Vec<Subscription>,
}

impl LyricsPanel {
    pub fn new(playback: Entity<Playback>, cx: &mut Context<Self>) -> Self {
        let client = LyricsClient::new()
            .inspect_err(|err| log::warn!("could not start the lyrics client: {err:#}"))
            .ok();
        let subscriptions = vec![cx.observe(&playback, |this, _, cx| this.follow_playback(cx))];
        let mut panel = LyricsPanel {
            playback,
            client,
            lyrics: HashMap::new(),
            track: None,
            active_line: None,
            scroll: ScrollHandle::new(),
            scroll_generation: 0,
            scroll_motion: None,
            scroll_pending: false,
            _subscriptions: subscriptions,
        };
        panel.follow_playback(cx);
        panel
    }

    /// Fetches the lyrics of a new track and moves the highlight. Redraws only when either
    /// changes, not on every progress tick.
    fn follow_playback(&mut self, cx: &mut Context<Self>) {
        let playback = self.playback.read(cx);
        let track = playback.current().cloned();
        let position = playback.position();

        if track.as_ref().map(|track| &track.id) != self.track.as_ref().map(|track| &track.id) {
            self.track = track.clone();
            self.active_line = None;
            self.scroll_motion = None;
            self.scroll.set_offset(point(px(0.), px(0.)));
            if let Some(track) = track {
                self.fetch(track, cx);
            }
            cx.notify();
        }

        let active_line = self
            .current_lyrics()
            .and_then(|lyrics| lyrics.active_line(position));
        if active_line != self.active_line {
            self.active_line = active_line;
            self.scroll_generation += 1;
            self.scroll_pending = active_line.is_some();
            cx.notify();
        }
    }

    fn fetch(&mut self, track: Track, cx: &mut Context<Self>) {
        if self.lyrics.contains_key(&track.id) {
            return;
        }
        let Some(client) = &self.client else {
            self.lyrics.insert(
                track.id,
                Load::Failed("No se pudo iniciar la búsqueda de letras".into()),
            );
            return;
        };
        let fetch = client.fetch(&track);
        self.lyrics.insert(track.id.clone(), Load::Loading);
        cx.spawn(async move |this, cx| {
            let result = fetch.await;
            this.update(cx, |this, cx| {
                let load = match result {
                    Ok(lyrics) => Load::Ready(lyrics),
                    Err(err) => {
                        log::warn!("lyrics for {}: {err:#}", track.id);
                        Load::Failed("No se pudo cargar la letra".into())
                    }
                };
                this.lyrics.insert(track.id, load);
                this.follow_playback(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn current_lyrics(&self) -> Option<&Lyrics> {
        match self.lyrics.get(&self.track.as_ref()?.id)? {
            Load::Ready(lyrics) => Some(lyrics),
            _ => None,
        }
    }

    /// Starts scrolling so the active line sits in the middle of the panel.
    fn start_scroll(&mut self) {
        let Some(line) = self.active_line else {
            return;
        };
        let Some(bounds) = self.scroll.bounds_for_item(line) else {
            return;
        };
        let viewport = self.scroll.bounds();
        if viewport.size.height <= px(0.) {
            return;
        }
        self.scroll_pending = false;
        // Child bounds are laid out unscrolled, relative to the same origin as the viewport.
        let top = f32::from(bounds.top() - viewport.top());
        let centered = top - f32::from(viewport.size.height - bounds.size.height) / 2.;
        let max = f32::from(self.scroll.max_offset().height);
        let target = -centered.clamp(0., max.max(0.));
        let from = f32::from(self.scroll.offset().y);
        self.scroll_motion = Some((from, target));
    }

    fn animate_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scroll_pending {
            self.start_scroll();
            if self.scroll_pending {
                // The lines have not been laid out yet: try again next frame.
                window.request_animation_frame();
            }
        }
        let Some((from, target)) = self.scroll_motion else {
            return;
        };
        let [y] = motion::animate_from(
            ("lyrics-scroll", self.scroll_generation),
            Spring::SPATIAL_SLOW,
            [from],
            [target],
            window,
            cx,
        );
        // Once it lands, the panel stops driving the offset so it can be scrolled by hand.
        let y = if (y - target).abs() < 0.5 {
            self.scroll_motion = None;
            target
        } else {
            y
        };
        self.scroll.set_offset(point(px(0.), px(y)));
    }

    fn render_synced(
        &self,
        lines: &[ytm_core::LyricLine],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let s = *Theme::of(cx);
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let color = match self.active_line {
                    Some(active) if index == active => s.on_surface,
                    Some(active) if index < active => s.on_surface_variant.opacity(0.5),
                    _ => s.on_surface_variant.opacity(0.8),
                };
                let color = motion::animate_color(
                    ("lyric-color", index),
                    Spring::EFFECTS,
                    color,
                    window,
                    cx,
                );
                let scale = motion::animate_f32(
                    ("lyric-scale", index),
                    Spring::SPATIAL,
                    if self.active_line == Some(index) {
                        1.
                    } else {
                        0.
                    },
                    window,
                    cx,
                );
                let text: SharedString = if line.text.is_empty() {
                    "♪".into()
                } else {
                    line.text.clone().into()
                };
                let start = line.start + SEEK_LEAD;
                let playback = self.playback.clone();
                div()
                    .id(("lyric", index))
                    .py_2()
                    .px_3()
                    .rounded(px(12.))
                    .cursor_pointer()
                    .hover(move |style| style.bg(s.surface_container_high))
                    .text_size(px(20. + 4. * scale))
                    .line_height(px(30.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color)
                    .child(text)
                    .on_click(move |_: &ClickEvent, _, cx| {
                        playback.update(cx, |playback, cx| {
                            if let Some(duration) = playback.duration() {
                                let fraction = start.as_secs_f32() / duration.as_secs_f32();
                                playback.seek_fraction(fraction, cx);
                            }
                        });
                    })
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for LyricsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = *Theme::of(cx);
        let Some(track) = &self.track else {
            return placeholder("Letra", "Reproduce algo para ver su letra", cx).into_any_element();
        };
        let lines = match self.lyrics.get(&track.id) {
            None | Some(Load::Loading) => {
                return div()
                    .size_full()
                    .p_6()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .children((0..8).map(|index| {
                        let width = 140. + (index * 61 % 130) as f32;
                        skeleton(Some(width), 22., 11.).order(index)
                    }))
                    .into_any_element();
            }
            Some(Load::Failed(error)) => {
                return placeholder("Letra", error.clone(), cx).into_any_element();
            }
            Some(Load::Ready(Lyrics::Instrumental)) => {
                return placeholder("Instrumental", "Esta canción no tiene letra", cx)
                    .into_any_element();
            }
            Some(Load::Ready(Lyrics::NotFound)) => {
                return placeholder("Sin letra", "No encontramos la letra de esta canción", cx)
                    .into_any_element();
            }
            Some(Load::Ready(Lyrics::Plain(lines))) => lines
                .iter()
                .map(|line| {
                    div()
                        .px_3()
                        .min_h(px(16.))
                        .text_size(px(18.))
                        .line_height(px(28.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(s.on_surface_variant)
                        .child(line.clone())
                        .into_any_element()
                })
                .collect(),
            Some(Load::Ready(Lyrics::Synced(lines))) => {
                let lines = lines.clone();
                self.animate_scroll(window, cx);
                self.render_synced(&lines, window, cx)
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("lyrics")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px_3()
                    .py_6()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(lines),
            )
            .child(
                div()
                    .px_6()
                    .py_2()
                    .text_xs()
                    .text_color(s.on_surface_variant)
                    .child("Letra de LRCLIB"),
            )
            .into_any_element()
    }
}
