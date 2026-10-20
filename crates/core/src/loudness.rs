//! Volume normalization: one gain per track, so quiet and loud masters play at a similar level.
//!
//! It is a plain RMS estimate over the whole track (not LUFS), which is close enough to even out
//! the difference between, say, an old quiet master and a modern loud one.

use std::io::Cursor;

use rodio::Decoder;

use crate::player::AudioBytes;

/// The RMS level tracks are brought to; about -15 dBFS, a little under typical modern masters, so
/// quiet tracks are lifted and loud ones turned down.
const TARGET_RMS: f32 = 0.18;
/// Limits that keep the correction subtle and avoid clipping quiet tracks pushed too far.
const MIN_GAIN: f32 = 0.4;
const MAX_GAIN: f32 = 1.4;
/// Below this the track is (nearly) silent and has no meaningful level.
const SILENCE_RMS: f32 = 1e-4;

/// The gain that brings an M4A/AAC track to the target level, or `None` if it cannot be decoded
/// or is silent. Decodes the whole track: run it off the UI and audio threads.
pub(crate) fn normalization_gain(audio: AudioBytes) -> Option<f32> {
    let byte_len = audio.len() as u64;
    let decoder = Decoder::builder()
        .with_data(Cursor::new(audio))
        .with_byte_len(byte_len)
        .with_seekable(true)
        .with_mime_type("audio/mp4")
        .build()
        .inspect_err(|err| log::warn!("could not measure loudness: {err}"))
        .ok()?;
    gain_for_rms(rms(decoder)?)
}

fn rms(samples: impl Iterator<Item = f32>) -> Option<f32> {
    let (sum, count) = samples.fold((0f64, 0u64), |(sum, count), sample| {
        (sum + f64::from(sample) * f64::from(sample), count + 1)
    });
    (count > 0).then(|| (sum / count as f64).sqrt() as f32)
}

fn gain_for_rms(rms: f32) -> Option<f32> {
    (rms > SILENCE_RMS).then(|| (TARGET_RMS / rms).clamp(MIN_GAIN, MAX_GAIN))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_a_square_wave_is_its_amplitude() {
        let samples = (0..1000).map(|i| if i % 2 == 0 { 0.25 } else { -0.25 });
        let measured = rms(samples).unwrap();
        assert!((measured - 0.25).abs() < 1e-6, "{measured}");
        assert_eq!(rms(std::iter::empty()), None);
    }

    #[test]
    fn loud_tracks_are_turned_down_and_quiet_ones_lifted() {
        assert!(gain_for_rms(0.36).unwrap() < 1.);
        assert!(gain_for_rms(0.09).unwrap() > 1.);
        assert!((gain_for_rms(TARGET_RMS).unwrap() - 1.).abs() < 1e-6);
    }

    #[test]
    fn gain_stays_within_limits_and_silence_has_none() {
        assert_eq!(gain_for_rms(0.95), Some(MIN_GAIN));
        assert_eq!(gain_for_rms(0.01), Some(MAX_GAIN));
        assert_eq!(gain_for_rms(0.), None);
    }

    #[test]
    fn undecodable_audio_has_no_gain() {
        assert_eq!(normalization_gain(b"not audio".to_vec().into()), None);
    }
}
