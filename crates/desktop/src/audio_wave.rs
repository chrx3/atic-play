//! The progress wave of the player bar, which swells with the loudness of what is playing.

use std::f32::consts::TAU;

use gpui::{
    App, ElementId, Hsla, PathBuilder, Pixels, Point, RenderOnce, Window, canvas, point,
    prelude::*, px,
};
use gpui_m3::motion::{animate_f32, continuous_at, key};
use gpui_m3::{Spring, Theme};

const STROKE: f32 = 4.;
const GAP: f32 = 4.;
const WAVELENGTH: f32 = 20.;
/// Amplitude in silence (or paused) and at full level.
const QUIET_AMPLITUDE: f32 = 1.;
const LOUD_AMPLITUDE: f32 = 6.;
/// Wavelengths per second the wave drifts in silence; loud music adds up to `LOUD_SPEEDUP` more.
const DRIFT_SPEED: f32 = 0.35;
const LOUD_SPEEDUP: f32 = 0.5;
/// How fast the loudness ripple travels along the wave (radians per second).
const RIPPLE_SPEED: f32 = 0.6;

/// Like gpui-m3's `WavyProgress`, but the wave's height follows `level` (0..1) with a fast
/// spring, and loud passages ripple along it.
#[derive(IntoElement)]
pub struct AudioWave {
    id: ElementId,
    value: f32,
    level: f32,
}

impl AudioWave {
    pub fn new(id: impl Into<ElementId>, value: f32, level: f32) -> Self {
        AudioWave {
            id: id.into(),
            value,
            level,
        }
    }
}

impl RenderOnce for AudioWave {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let s = *Theme::of(cx);
        let t = continuous_at(30., window, cx);
        let value = animate_f32(
            key(&self.id, "value"),
            Spring::SPATIAL,
            self.value.clamp(0., 1.),
            window,
            cx,
        );
        // A slow, bounce-free spring: the level changes ~30 times a second, and following it
        // closely makes the wave twitch.
        let level = animate_f32(
            key(&self.id, "level"),
            Spring::EFFECTS_SLOW,
            self.level.clamp(0., 1.),
            window,
            cx,
        )
        .max(0.);
        let (active, track) = (s.primary, s.secondary_container);

        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let x0 = f32::from(bounds.origin.x);
                let w = f32::from(bounds.size.width);
                let cy = f32::from(bounds.origin.y) + f32::from(bounds.size.height) / 2.;
                let end = (w - STROKE) * value;

                let track_start = if end > 0.5 {
                    end + GAP + STROKE / 2.
                } else {
                    0.
                };
                let track_end = w - STROKE - GAP;
                if track_end > track_start {
                    paint_line(
                        &[
                            point(px(x0 + track_start), px(cy)),
                            point(px(x0 + track_end), px(cy)),
                        ],
                        track,
                        window,
                    );
                }
                paint_dot(point(px(x0 + w - STROKE / 2.), px(cy)), active, window);

                if end > 0.5 {
                    let base = QUIET_AMPLITUDE + (LOUD_AMPLITUDE - QUIET_AMPLITUDE) * level;
                    // Short waves fit a smaller swing, as in gpui-m3.
                    let amplitude = base.min(end / 6.);
                    // Louder music also drifts a little faster.
                    let phase = t * WAVELENGTH * DRIFT_SPEED * (1. + LOUD_SPEEDUP * level);
                    let steps = (end / 1.5).ceil().max(2.) as usize;
                    let points: Vec<Point<Pixels>> = (0..=steps)
                        .map(|i| {
                            let x = end * i as f32 / steps as f32;
                            // A slow ripple of ±30 % travels along the wave when it is loud.
                            let ripple = 1. + 0.3 * level * (x / 90. - t * RIPPLE_SPEED).sin();
                            let y =
                                cy - amplitude * ripple * ((x + phase) / WAVELENGTH * TAU).sin();
                            point(px(x0 + x), px(y))
                        })
                        .collect();
                    paint_line(&points, active, window);
                }
            },
        )
        .w_full()
        .h(px(2. * LOUD_AMPLITUDE * 1.3 + STROKE))
    }
}

/// A line with rounded ends.
fn paint_line(points: &[Point<Pixels>], color: Hsla, window: &mut Window) {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return;
    };
    let mut builder = PathBuilder::stroke(px(STROKE));
    builder.move_to(*first);
    for p in &points[1..] {
        builder.line_to(*p);
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
    paint_dot(*first, color, window);
    paint_dot(*last, color, window);
}

fn paint_dot(center: Point<Pixels>, color: Hsla, window: &mut Window) {
    let radius = STROKE / 2.;
    let points: Vec<Point<Pixels>> = (0..20)
        .map(|i| {
            let a = i as f32 / 20. * TAU;
            point(
                center.x + px(radius * a.cos()),
                center.y + px(radius * a.sin()),
            )
        })
        .collect();
    let mut builder = PathBuilder::fill();
    builder.add_polygon(&points, true);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}
