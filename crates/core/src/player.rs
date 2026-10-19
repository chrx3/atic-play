use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use rodio::source::SeekError;
use rodio::{ChannelCount, Decoder, MixerDeviceSink, Player, Sample, SampleRate, Source};

/// How many times per second the level is measured.
const LEVELS_PER_SECOND: u32 = 30;

type BoxedSource = Box<dyn Source + Send>;

/// A track decoded and wrapped, ready to start without delay.
struct Prepared {
    source: BoxedSource,
    fade: Arc<Fade>,
}

/// Plays tracks on the default output device, with gapless transitions and crossfades.
///
/// It has two decks (rodio players). A track plays on the active deck; the next one can be
/// prepared ahead, then either queued behind it on the same deck (gapless) or started on the
/// other deck while the first fades out (crossfade).
///
/// Not `Send` on every platform: keep it on the thread that created it.
pub struct AudioPlayer {
    // Dropping the device sink stops all sound, so it lives as long as the player.
    _sink: MixerDeviceSink,
    decks: [Player; 2],
    active: usize,
    /// The fade of the track playing on each deck.
    fades: [Option<Arc<Fade>>; 2],
    next: Option<Prepared>,
    /// The next track is queued behind the current one on the active deck.
    gapless_fade: Option<Arc<Fade>>,
    /// When the deck faded out by a crossfade can be cleared.
    fade_out_ends: Option<Instant>,
    /// The loudness of what plays, as `f32` bits, written by the audio thread.
    level: Arc<AtomicU32>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self> {
        let mut sink = rodio::DeviceSinkBuilder::open_default_sink()?;
        sink.log_on_drop(false);
        let decks = [
            Player::connect_new(sink.mixer()),
            Player::connect_new(sink.mixer()),
        ];
        Ok(AudioPlayer {
            _sink: sink,
            decks,
            active: 0,
            fades: [None, None],
            next: None,
            gapless_fade: None,
            fade_out_ends: None,
            level: Arc::default(),
        })
    }

    /// Replaces whatever is playing with an M4A/AAC track held in memory. `gain` normalizes its
    /// volume (1 leaves it as it is).
    pub fn play(&mut self, audio: Vec<u8>, gain: f32) -> Result<()> {
        let prepared = self.prepare(audio, gain)?;
        self.stop();
        let deck = &self.decks[self.active];
        deck.append(prepared.source);
        deck.play();
        self.fades[self.active] = Some(prepared.fade);
        Ok(())
    }

    /// Decodes the track that plays next, so the transition to it starts instantly. Replaces an
    /// earlier one that has not started.
    pub fn prepare_next(&mut self, audio: Vec<u8>, gain: f32) -> Result<()> {
        self.next = Some(self.prepare(audio, gain)?);
        Ok(())
    }

    pub fn has_next(&self) -> bool {
        self.next.is_some()
    }

    /// Forgets the prepared next track (the queue changed). One already queued for a gapless
    /// transition stays.
    pub fn clear_next(&mut self) {
        self.next = None;
    }

    /// Starts the transition to the prepared next track. With no crossfade it is queued behind
    /// the current one and follows it without a gap; otherwise it starts now on the other deck,
    /// fading in while the current one fades out over `crossfade`. Returns whether it started;
    /// see [`AudioPlayer::poll_transition`] for when a gapless one takes over.
    pub fn begin_transition(&mut self, crossfade: Duration) -> bool {
        let Some(next) = self.next.take() else {
            return false;
        };
        if crossfade.is_zero() {
            self.decks[self.active].append(next.source);
            self.gapless_fade = Some(next.fade);
            return true;
        }
        if let Some(fade) = &self.fades[self.active] {
            fade.ramp_to(0., crossfade);
        }
        let incoming = 1 - self.active;
        let deck = &self.decks[incoming];
        deck.clear();
        next.fade.set(0.);
        next.fade.ramp_to(1., crossfade);
        deck.append(next.source);
        deck.play();
        self.fades[incoming] = Some(next.fade);
        self.active = incoming;
        self.fade_out_ends = Some(Instant::now() + crossfade);
        true
    }

    /// Call regularly. Returns true once a gapless transition has moved on to the next track,
    /// and clears the deck a finished crossfade faded out.
    pub fn poll_transition(&mut self) -> bool {
        if self.fade_out_ends.is_some_and(|end| Instant::now() >= end) {
            self.fade_out_ends = None;
            let faded = 1 - self.active;
            self.decks[faded].clear();
            self.fades[faded] = None;
        }
        if self.gapless_fade.is_some() && self.decks[self.active].len() <= 1 {
            self.fades[self.active] = self.gapless_fade.take();
            return true;
        }
        false
    }

    /// The RMS level (0 to about 1) of the last ~33 ms played, before the volume is applied.
    /// It holds its last value while paused.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Silences everything, e.g. while the next track loads.
    pub fn stop(&mut self) {
        for deck in &self.decks {
            deck.clear();
        }
        self.fades = [None, None];
        self.next = None;
        self.gapless_fade = None;
        self.fade_out_ends = None;
    }

    pub fn toggle_pause(&self) {
        let pause = !self.is_paused();
        // During a crossfade both decks sound; they pause and resume together.
        for deck in self.decks.iter().filter(|deck| !deck.empty()) {
            if pause {
                deck.pause();
            } else {
                deck.play();
            }
        }
    }

    pub fn is_paused(&self) -> bool {
        self.decks[self.active].is_paused()
    }

    /// Nothing left to play: the current track ended and no transition is queued.
    pub fn is_idle(&self) -> bool {
        self.decks[self.active].empty()
    }

    pub fn position(&self) -> Duration {
        self.decks[self.active].get_pos()
    }

    pub fn seek(&self, position: Duration) {
        if let Err(err) = self.decks[self.active].try_seek(position) {
            log::warn!("seek failed: {err}");
        }
    }

    pub fn set_volume(&self, volume: f32) {
        for deck in &self.decks {
            deck.set_volume(volume);
        }
    }

    fn prepare(&self, audio: Vec<u8>, gain: f32) -> Result<Prepared> {
        let byte_len = audio.len() as u64;
        let decoder = Decoder::builder()
            .with_data(Cursor::new(audio))
            .with_byte_len(byte_len)
            .with_seekable(true)
            .with_mime_type("audio/mp4")
            .build()?;
        let fade = Arc::new(Fade::new(1.));
        let source = LevelMeter::new(
            Faded::new(decoder.amplify(gain), fade.clone()),
            self.level.clone(),
        );
        Ok(Prepared {
            source: Box::new(source),
            fade,
        })
    }
}

/// A gain the UI thread sets and the audio thread follows smoothly.
struct Fade {
    /// The gain to reach, as `f32` bits.
    target: AtomicU32,
    /// How long a full 0 to 1 ramp takes, in milliseconds; 0 jumps at once.
    ramp_ms: AtomicU32,
    /// The gain the last `set` jumped to, kept apart from `target` so that a `set` followed at
    /// once by a `ramp_to` starts the ramp from it.
    jump_to: AtomicU32,
    /// Bumped by `set`, so the audio thread jumps instead of ramping.
    jumps: AtomicU32,
}

impl Fade {
    fn new(gain: f32) -> Self {
        Fade {
            target: AtomicU32::new(gain.to_bits()),
            ramp_ms: AtomicU32::new(0),
            jump_to: AtomicU32::new(gain.to_bits()),
            jumps: AtomicU32::new(0),
        }
    }

    /// Jumps to `gain`; a following `ramp_to` starts from it.
    fn set(&self, gain: f32) {
        self.target.store(gain.to_bits(), Ordering::Relaxed);
        self.jump_to.store(gain.to_bits(), Ordering::Relaxed);
        self.jumps.fetch_add(1, Ordering::Release);
    }

    /// Moves to `gain` over `duration` (for a full 0 to 1 change).
    fn ramp_to(&self, gain: f32, duration: Duration) {
        let ms = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
        self.ramp_ms.store(ms, Ordering::Relaxed);
        self.target.store(gain.to_bits(), Ordering::Release);
    }
}

/// Scales samples by a [`Fade`], moving toward its target a little every sample.
struct Faded<S> {
    inner: S,
    fade: Arc<Fade>,
    gain: f32,
    jumps_seen: u32,
    samples_per_ms: f32,
}

impl<S: Source> Faded<S> {
    fn new(inner: S, fade: Arc<Fade>) -> Self {
        let samples_per_second =
            inner.sample_rate().get() as f32 * f32::from(inner.channels().get());
        Faded {
            gain: f32::from_bits(fade.jump_to.load(Ordering::Relaxed)),
            jumps_seen: fade.jumps.load(Ordering::Relaxed),
            samples_per_ms: samples_per_second / 1000.,
            inner,
            fade,
        }
    }
}

impl<S: Source> Iterator for Faded<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let sample = self.inner.next()?;
        let jumps = self.fade.jumps.load(Ordering::Acquire);
        if jumps != self.jumps_seen {
            self.jumps_seen = jumps;
            self.gain = f32::from_bits(self.fade.jump_to.load(Ordering::Relaxed));
        }
        let target = f32::from_bits(self.fade.target.load(Ordering::Acquire));
        let ramp_ms = self.fade.ramp_ms.load(Ordering::Relaxed);
        if ramp_ms == 0 {
            self.gain = target;
        } else if self.gain != target {
            let step = 1. / (ramp_ms as f32 * self.samples_per_ms);
            self.gain = if self.gain < target {
                (self.gain + step).min(target)
            } else {
                (self.gain - step).max(target)
            };
        }
        Some(sample * self.gain)
    }
}

impl<S: Source> Source for Faded<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(position)
    }
}

/// Passes samples through unchanged while measuring their RMS level for a visualizer.
struct LevelMeter<S> {
    inner: S,
    level: Arc<AtomicU32>,
    sum_of_squares: f32,
    count: usize,
    window: usize,
}

impl<S: Source> LevelMeter<S> {
    fn new(inner: S, level: Arc<AtomicU32>) -> Self {
        let samples_per_second =
            inner.sample_rate().get() as usize * inner.channels().get() as usize;
        LevelMeter {
            window: (samples_per_second / LEVELS_PER_SECOND as usize).max(1),
            inner,
            level,
            sum_of_squares: 0.,
            count: 0,
        }
    }
}

impl<S: Source> Iterator for LevelMeter<S> {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let sample = self.inner.next()?;
        let value: f32 = sample;
        self.sum_of_squares += value * value;
        self.count += 1;
        if self.count >= self.window {
            let rms = (self.sum_of_squares / self.count as f32).sqrt();
            self.level.store(rms.to_bits(), Ordering::Relaxed);
            self.sum_of_squares = 0.;
            self.count = 0;
        }
        Some(sample)
    }
}

impl<S: Source> Source for LevelMeter<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    // Without forwarding, seeking would stop working once the meter wraps the decoder.
    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(position)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZero;

    use rodio::buffer::SamplesBuffer;

    use super::*;

    #[test]
    fn level_is_the_rms_of_the_samples() {
        // One second of a square wave at ±0.5: its RMS is 0.5.
        let samples: Vec<Sample> = (0..8_000)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect();
        let source = SamplesBuffer::new(
            NonZero::new(1).unwrap(),
            NonZero::new(8_000).unwrap(),
            samples,
        );
        let level = Arc::new(AtomicU32::new(0));
        let meter = LevelMeter::new(source, level.clone());

        let passed: Vec<Sample> = meter.collect();

        assert_eq!(passed.len(), 8_000);
        let measured = f32::from_bits(level.load(Ordering::Relaxed));
        assert!((measured - 0.5).abs() < 1e-4, "measured {measured}");
    }

    fn ones(rate: u32, seconds: u32) -> SamplesBuffer {
        let samples = vec![1.0 as Sample; (rate * seconds) as usize];
        SamplesBuffer::new(
            NonZero::new(1).unwrap(),
            NonZero::new(rate).unwrap(),
            samples,
        )
    }

    #[test]
    fn fade_ramps_over_its_duration() {
        // 1000 samples per second, so 1 ms per sample.
        let fade = Arc::new(Fade::new(0.));
        fade.ramp_to(1., Duration::from_millis(100));
        let gains: Vec<Sample> = Faded::new(ones(1000, 1), fade).collect();

        assert!((gains[49] - 0.5).abs() < 0.02, "halfway: {}", gains[49]);
        assert!((gains[99] - 1.).abs() < 1e-3, "end of ramp: {}", gains[99]);
        assert_eq!(gains[500], 1.);
    }

    #[test]
    fn crossfade_in_starts_silent_even_from_a_playing_gain() {
        // As in a crossfade: the prepared track's fade sat at 1, then is set to 0 and ramped up.
        let fade = Arc::new(Fade::new(1.));
        let faded = Faded::new(ones(1000, 1), fade.clone());
        fade.set(0.);
        fade.ramp_to(1., Duration::from_millis(100));
        let gains: Vec<Sample> = faded.collect();

        assert!(gains[0] < 0.02, "first sample: {}", gains[0]);
        assert!((gains[49] - 0.5).abs() < 0.02, "halfway: {}", gains[49]);
    }

    #[test]
    fn fade_set_jumps_without_a_ramp() {
        let fade = Arc::new(Fade::new(1.));
        fade.ramp_to(0., Duration::from_secs(10));
        fade.set(0.25);
        let first = Faded::new(ones(1000, 1), fade).next().unwrap();
        assert_eq!(first, 0.25);
    }
}
