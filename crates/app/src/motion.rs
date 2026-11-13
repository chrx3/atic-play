//! The app's motion vocabulary. One rhythm for every transition: about 620 ms, settling without
//! a visible bounce, the same in and out.

use gpui_m3::motion::{Spring, cubic_bezier};

/// Critically damped spring that reaches 99 % in about 620 ms and can be interrupted midway.
/// Use it for anything that moves or resizes: sheets, the queue, scroll, the lyrics follow.
pub const CALM: Spring = Spring {
    stiffness: 120.,
    damping_ratio: 1.,
};

/// Length of a timed transition (view changes, fades), in seconds.
pub const CALM_SECS: f32 = 0.62;

/// Where the first segment ends: the outgoing content leaves before 0.48, the incoming one
/// arrives after it.
pub const CALM_SPLIT: f32 = 0.48;

/// Easing for timed transitions.
pub fn calm_ease() -> impl Fn(f32) -> f32 + Copy {
    cubic_bezier(0.45, 0., 0.2, 1.)
}
