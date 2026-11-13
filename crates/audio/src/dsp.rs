//! Volume math and channel juggling.
//!
//! Everything here is pure: no threads, no device, no clock. That keeps the fiddly parts
//! (decibels, mono/stereo conversions) easy to test.

/// Turns decibels into a linear gain. 0 dB is 1.0, -6 dB is about 0.5.
pub fn db_to_gain(db: f32) -> f32 {
    if !db.is_finite() {
        return 1.0;
    }
    if db <= -60.0 {
        // Below this it is inaudible anyway and the power keeps the numbers tidy.
        return 0.0;
    }
    10f32.powf(db / 20.0)
}

/// Lays planar channels out one after the other (interleaved).
pub fn interleave(planes: &[Vec<f32>], out: &mut Vec<f32>) {
    let channels = planes.len();
    let frames = planes.first().map_or(0, |plane| plane.len());
    out.clear();
    out.reserve(frames * channels);
    for frame in 0..frames {
        for plane in planes {
            out.push(plane[frame]);
        }
    }
}

/// Turns interleaved audio into `dst_channels` channels.
///
/// * stereo to mono averages both sides,
/// * mono to stereo copies the channel,
/// * anything else keeps the first channels and pads with silence.
pub fn remix(src: &[f32], src_channels: usize, dst_channels: usize, out: &mut Vec<f32>) {
    out.clear();
    if src_channels == 0 || dst_channels == 0 {
        return;
    }
    let frames = src.len() / src_channels;
    out.reserve(frames * dst_channels);
    match (src_channels, dst_channels) {
        (1, 1) => out.extend_from_slice(&src[..frames]),
        (1, target) => {
            for frame in 0..frames {
                let value = src[frame];
                for _ in 0..target {
                    out.push(value);
                }
            }
        }
        (source, 1) => {
            for frame in 0..frames {
                let base = frame * source;
                let sum: f32 = src[base..base + source].iter().sum();
                out.push(sum / source as f32);
            }
        }
        (source, target) => {
            for frame in 0..frames {
                let base = frame * source;
                for channel in 0..target {
                    out.push(if channel < source { src[base + channel] } else { 0.0 });
                }
            }
        }
    }
}

/// Walks the gain from one value to another over `frames` samples, so the volume never jumps.
///
/// `step` is how much the gain moves per frame; the caller keeps the running value.
#[derive(Clone, Copy, Debug)]
pub struct Ramp {
    current: f32,
    target: f32,
    step: f32,
}

impl Ramp {
    /// A ramp that jumps to `value` right away (used when nothing is playing yet).
    pub fn fixed(value: f32) -> Self {
        Ramp { current: value, target: value, step: 0.0 }
    }

    /// The gain to apply to the next sample.
    pub fn next(&mut self) -> f32 {
        if (self.current - self.target).abs() < f32::EPSILON {
            self.current = self.target;
            return self.current;
        }
        if self.step > 0.0 {
            self.current = (self.current + self.step).min(self.target);
        } else {
            self.current = (self.current + self.step).max(self.target);
        }
        self.current
    }

    /// Sets a new target, keeping the current value so the move stays smooth.
    pub fn set_target(&mut self, target: f32, rate: f32, millis: f32) {
        if (self.target - target).abs() < f32::EPSILON {
            return;
        }
        let frames = (millis * 0.001 * rate).max(1.0);
        self.step = (target - self.current) / frames;
        self.target = target;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_db_is_unity() {
        assert!((db_to_gain(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn minus_six_db_is_half() {
        assert!((db_to_gain(-6.0) - 0.501_187).abs() < 1e-5);
    }

    #[test]
    fn very_quiet_is_silence() {
        assert_eq!(db_to_gain(-80.0), 0.0);
    }

    #[test]
    fn nan_does_not_break_the_gain() {
        assert_eq!(db_to_gain(f32::NAN), 1.0);
    }

    #[test]
    fn interleave_lays_channels_side_by_side() {
        let planes = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let mut out = Vec::new();
        interleave(&planes, &mut out);
        assert_eq!(out, vec![1.0, 3.0, 2.0, 4.0]);
    }

    #[test]
    fn stereo_to_mono_averages() {
        let mut out = Vec::new();
        remix(&[1.0, 3.0, 2.0, 4.0], 2, 1, &mut out);
        assert_eq!(out, vec![2.0, 3.0]);
    }

    #[test]
    fn mono_to_stereo_copies() {
        let mut out = Vec::new();
        remix(&[1.0, 2.0], 1, 2, &mut out);
        assert_eq!(out, vec![1.0, 1.0, 2.0, 2.0]);
    }

    #[test]
    fn ramp_reaches_the_target_without_overshooting() {
        let mut ramp = Ramp::fixed(1.0);
        ramp.set_target(0.0, 1000.0, 10.0);
        let mut last = 1.0;
        for _ in 0..1000 {
            last = ramp.next();
            assert!(last >= 0.0 && last <= 1.0);
        }
        assert_eq!(last, 0.0);
    }}
