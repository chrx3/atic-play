//! Where we are inside the track.
//!
//! The clock is pure arithmetic on top of a device sample counter: the callback tells it how
//! many frames the device took, and it interpolates between callbacks so the UI sees a position
//! that moves smoothly instead of jumping once per buffer.

use std::time::Duration;

/// Tracks the position of the track being played.
#[derive(Clone, Debug)]
pub struct Clock {
    /// Sample rate of the output device.
    rate: f64,
    /// Position of the first frame of the current run.
    base: Duration,
    /// Frames the device took since the current run started.
    rendered: u64,
    /// When the last frame was taken, measured from the clock's own zero.
    tick: Duration,
    /// Whether the timeline is moving.
    running: bool,
}

impl Clock {
    /// A clock for a device running at `rate` samples per second.
    pub fn new(rate: f64) -> Self {
        Clock { rate: rate.max(1.0), base: Duration::ZERO, rendered: 0, tick: Duration::ZERO, running: false }
    }

    /// Starts over at `position`. The timeline may jump: that is what seeking is.
    pub fn seek(&mut self, position: Duration, now: Duration) {
        self.base = position;
        self.rendered = 0;
        self.tick = now;
    }

    /// The device took `frames` frames. `now` is the time since the clock's zero.
    pub fn render(&mut self, frames: u64, now: Duration) {
        if !self.running {
            return;
        }
        self.rendered = self.rendered.saturating_add(frames);
        self.tick = now;
    }

    /// Pauses or resumes the timeline. `now` is the time since the clock's zero.
    pub fn set_running(&mut self, running: bool, now: Duration) {
        if running == self.running {
            return;
        }
        if !running {
            // Freeze the interpolation into the base so the position does not jump back.
            let extra = now.saturating_sub(self.tick).as_secs_f64() * self.rate;
            self.base += Duration::from_secs_f64((self.rendered as f64 + extra) / self.rate);
            self.rendered = 0;
        }
        self.running = running;
        self.tick = now;
    }

    /// Where the audio is right now. `now` is the time since the clock's zero.
    pub fn position(&self, now: Duration) -> Duration {
        let extra = if self.running {
            now.saturating_sub(self.tick).as_secs_f64() * self.rate
        } else {
            0.0
        };
        let frames = self.rendered as f64 + extra;
        self.base + Duration::from_secs_f64(frames / self.rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    #[test]
    fn starts_at_zero() {
        let clock = Clock::new(RATE);
        assert_eq!(clock.position(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn follows_the_frames_the_device_took() {
        let mut clock = Clock::new(RATE);
        clock.set_running(true, Duration::ZERO);
        clock.render(48_000, Duration::from_millis(1000));
        // One second of audio, and no time passed since the callback.
        assert_eq!(clock.position(Duration::from_millis(1000)), Duration::from_secs(1));
    }

    #[test]
    fn interpolates_between_callbacks() {
        let mut clock = Clock::new(RATE);
        clock.set_running(true, Duration::ZERO);
        clock.render(48_000, Duration::from_millis(1000));
        let half = clock.position(Duration::from_millis(1500));
        assert_eq!(half, Duration::from_millis(1500));
    }

    #[test]
    fn never_goes_backwards() {
        let mut clock = Clock::new(RATE);
        clock.set_running(true, Duration::ZERO);
        let mut last = Duration::ZERO;
        for step in 0..100 {
            let now = Duration::from_millis(step * 10);
            clock.render(480, now);
            let position = clock.position(now);
            assert!(position >= last, "{position:?} went back from {last:?}");
            last = position;
        }
    }

    #[test]
    fn pausing_freezes_the_position() {
        let mut clock = Clock::new(RATE);
        clock.set_running(true, Duration::ZERO);
        clock.render(24_000, Duration::from_millis(500));
        clock.set_running(false, Duration::from_millis(500));
        let frozen = clock.position(Duration::from_millis(500));
        // Time passes but the position does not.
        assert_eq!(clock.position(Duration::from_secs(30)), frozen);
        assert_eq!(frozen, Duration::from_millis(500));
    }

    #[test]
    fn resuming_continues_from_the_same_place() {
        let mut clock = Clock::new(RATE);
        clock.set_running(true, Duration::ZERO);
        clock.render(24_000, Duration::from_millis(500));
        clock.set_running(false, Duration::from_millis(500));
        clock.set_running(true, Duration::from_millis(800));
        let position = clock.position(Duration::from_millis(800));
        assert_eq!(position, Duration::from_millis(500));
    }

    #[test]
    fn seeking_moves_the_base() {
        let mut clock = Clock::new(RATE);
        clock.set_running(true, Duration::ZERO);
        clock.seek(Duration::from_secs(30), Duration::from_millis(10));
        assert_eq!(clock.position(Duration::from_millis(10)), Duration::from_secs(30));
        clock.render(48_000, Duration::from_millis(1010));
        assert_eq!(clock.position(Duration::from_millis(1010)), Duration::from_secs(31));
    }
}
