use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::Result;
use rodio::source::SeekError;
use rodio::{ChannelCount, Decoder, MixerDeviceSink, Player, Sample, SampleRate, Source};

/// How many times per second the level is measured.
const LEVELS_PER_SECOND: u32 = 30;

/// Plays one track at a time on the default output device.
///
/// Not `Send` on every platform: keep it on the thread that created it.
pub struct AudioPlayer {
    // Dropping the device sink stops all sound, so it lives as long as the player.
    _sink: MixerDeviceSink,
    player: Player,
    /// The loudness of what plays, as `f32` bits, written by the audio thread.
    level: Arc<AtomicU32>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self> {
        let mut sink = rodio::DeviceSinkBuilder::open_default_sink()?;
        sink.log_on_drop(false);
        let player = Player::connect_new(sink.mixer());
        Ok(AudioPlayer {
            _sink: sink,
            player,
            level: Arc::default(),
        })
    }

    /// Replaces whatever is playing with an M4A/AAC track held in memory.
    pub fn play(&self, audio: Vec<u8>) -> Result<()> {
        let byte_len = audio.len() as u64;
        let source = Decoder::builder()
            .with_data(Cursor::new(audio))
            .with_byte_len(byte_len)
            .with_seekable(true)
            .with_mime_type("audio/mp4")
            .build()?;
        self.player.clear();
        self.player
            .append(LevelMeter::new(source, self.level.clone()));
        self.player.play();
        Ok(())
    }

    /// The RMS level (0 to about 1) of the last ~33 ms played, before the volume is applied.
    /// It holds its last value while paused.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Silences the current track, e.g. while the next one loads.
    pub fn stop(&self) {
        self.player.clear();
    }

    pub fn toggle_pause(&self) {
        if self.player.is_paused() {
            self.player.play();
        } else {
            self.player.pause();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.player.is_paused()
    }

    pub fn is_idle(&self) -> bool {
        self.player.empty()
    }

    pub fn position(&self) -> Duration {
        self.player.get_pos()
    }

    pub fn seek(&self, position: Duration) {
        if let Err(err) = self.player.try_seek(position) {
            log::warn!("seek failed: {err}");
        }
    }

    pub fn set_volume(&self, volume: f32) {
        self.player.set_volume(volume);
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
}
