//! The decoder thread.
//!
//! Symphonia demuxes and decodes, rubato converts the sample rate when the file and the device
//! disagree, and the result goes into the queue for the device callback to pick up.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Resampler, SincInterpolationParameters};
use symphonia::core::audio::AudioBufferRef;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

use crate::http::HttpSource;
use crate::queue::Block;
use crate::{dsp, state, Shared, Source};

/// How many frames the resampler produces at a time.
const RESAMPLE_CHUNK: usize = 1024;

/// A running decoder. Dropping it does not stop the thread; call `stop`.
pub struct DecoderHandle {
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl DecoderHandle {
    /// Asks the decoder to stop as soon as it can.
    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.join.take();
    }
}

impl Drop for DecoderHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Starts a decoder for `source`.
pub fn spawn(
    shared: Arc<Shared>,
    source: Source,
    epoch: u64,
    device_rate: u32,
    device_channels: usize,
) -> Result<DecoderHandle, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let thread_cancel = Arc::clone(&cancel);
    let join = std::thread::Builder::new()
        .name("atic-audio-decode".into())
        .spawn(move || {
            let decoder = AudioDecoder { shared, device_rate, device_channels };
            decoder.run(source, epoch, thread_cancel);
        })
        .map_err(|err| format!("no se pudo iniciar el decodificador: {err}"))?;
    Ok(DecoderHandle { cancel, join: Some(join) })
}

struct AudioDecoder {
    shared: Arc<Shared>,
    device_rate: u32,
    device_channels: usize,
}

impl AudioDecoder {
    fn run(&self, source: Source, mut epoch: u64, cancel: Arc<AtomicBool>) {
        let progress = self.shared.clone();
        let http = match HttpSource::new(&source.url, &source.headers, source.content_length, move || {
            progress.note_progress();
        }) {
            Ok(http) => http,
            Err(err) => {
                self.shared.report_error(err.to_string());
                return;
            }
        };
        let cancel_http = http.cancel_handle();

        let mut hint = Hint::new();
        if let Some(extension) = guess_extension(&source.url) {
            hint.with_extension(&extension);
        }

        let stream = MediaSourceStream::new(Box::new(http), MediaSourceStreamOptions::default());
        let probed = symphonia::default::get_probe().format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        );
        let mut format = match probed {
            Ok(probed) => probed.format,
            Err(err) => {
                cancel_http.cancel();
                self.shared.report_error(format!("no se pudo leer el archivo de audio: {err}"));
                return;
            }
        };

        // Pick the audio track: the one that says how many samples per second it has.
        let (track_id, params) = {
            let track = format
                .tracks()
                .iter()
                .find(|track| track.codec_params.sample_rate.is_some())
                .or_else(|| format.default_track());
            match track {
                Some(track) => (track.id, track.codec_params.clone()),
                None => {
                    cancel_http.cancel();
                    self.shared.report_error("el archivo no tiene pistas de audio".into());
                    return;
                }
            }
        };

        let file_rate = params.sample_rate.unwrap_or(self.device_rate);
        let file_channels = params.channels.map(|channels| channels.count()).unwrap_or(2).max(1);

        if let (Some(frames), Some(rate)) = (params.n_frames, params.sample_rate) {
            if rate > 0 {
                let duration = Duration::from_secs_f64(frames as f64 / rate as f64);
                self.shared.push_event(crate::Event::Duration(duration));
            }
        }

        let mut decoder = match symphonia::default::get_codecs().make(&params, &DecoderOptions::default())
        {
            Ok(decoder) => decoder,
            Err(err) => {
                cancel_http.cancel();
                self.shared.report_error(format!("no se pudo crear el decodificador: {err}"));
                return;
            }
        };

        let mut resampler: Option<Async<f32>> = None;
        let mut pending: Vec<f32> = Vec::new();
        let mut pending_frames = 0usize;
        let mut interleaved: Vec<f32> = Vec::new();
        let mut block: Vec<f32> = Vec::new();
        let mut started = false;
        let mut failed = false;

        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }

            // Did the app ask to jump somewhere?
            if let Some((new_epoch, target)) = self.shared.take_seek() {
                epoch = new_epoch;
                match seek(&mut format, target) {
                    Ok(()) => {
                        decoder.reset();
                        resampler = None;
                        pending.clear();
                        pending_frames = 0;
                    }
                    Err(err) => {
                        failed = true;
                        self.shared
                            .report_error(format!("no se pudo saltar a {target:?}: {err}"));
                        break;
                    }
                }
            }

            let packet = match format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(err))
                    if err.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(err) => {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    failed = true;
                    self.shared.report_error(format!("error leyendo el archivo: {err}"));
                    break;
                }
            };

            if packet.track_id() != track_id {
                continue;
            }

            // The track is moving forward: the engine knows it is not stuck.
            self.shared.note_progress();

            let decoded = match decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(err) => {
                    // A single broken packet is not the end of the song.
                    log::warn!("no se pudo decodificar un paquete: {err}");
                    continue;
                }
            };

            let planes = to_planar(&decoded);
            let frames = planes.first().map_or(0, |plane| plane.len());
            if frames == 0 {
                continue;
            }
            let planes = fit_channels(planes, file_channels);
            dsp::interleave(&planes, &mut interleaved);

            if file_rate == self.device_rate {
                block.clear();
                dsp::remix(&interleaved, file_channels, self.device_channels, &mut block);
                let frames = block.len() / self.device_channels;
                if !self.push(epoch, std::mem::take(&mut block), frames, &mut started) {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                continue;
            }

            if resampler.is_none() {
                match Async::<f32>::new_sinc(
                    self.device_rate as f64 / file_rate as f64,
                    1.0,
                    &SincInterpolationParameters::default(),
                    RESAMPLE_CHUNK,
                    file_channels,
                    FixedAsync::Output,
                ) {
                    Ok(created) => resampler = Some(created),
                    Err(err) => {
                        self.shared
                            .report_error(format!("no se pudo crear el conversor de audio: {err}"));
                        break;
                    }
                }
            }

            let resampler = resampler.as_mut().expect("the resampler is there");
            pending.extend_from_slice(&interleaved);
            pending_frames += frames;

            let mut stale = false;
            loop {
                let needed = resampler.input_frames_next();
                if pending_frames < needed {
                    break;
                }
                let out_frames = resampler.output_frames_next();
                let mut resampled = vec![0.0f32; out_frames * file_channels];
                {
                    let input = InterleavedSlice::new(&pending, file_channels, pending_frames)
                        .expect("the input buffer has the frames it should");
                    let mut output = InterleavedSlice::new_mut(&mut resampled, file_channels, out_frames)
                        .expect("the output buffer has the frames it should");
                    if let Err(err) = resampler.process_into_buffer(&input, &mut output, None) {
                        self.shared
                            .report_error(format!("no se pudo convertir la tasa de muestreo: {err}"));
                        stale = true;
                        break;
                    }
                }
                pending.drain(..needed * file_channels);
                pending_frames -= needed;

                block.clear();
                dsp::remix(&resampled, file_channels, self.device_channels, &mut block);
                let frames = block.len() / self.device_channels;
                if !self.push(epoch, std::mem::take(&mut block), frames, &mut started) {
                    stale = true;
                    break;
                }
            }

            if stale {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        cancel_http.cancel();
        self.shared.queue.finish(epoch);

        if !failed && !started && !cancel.load(Ordering::Relaxed) {
            // The file ended without giving a single sample of audio: the app would wait forever.
            self.shared
                .report_error("el archivo no tiene audio que reproducir".into());
        }
    }

    /// Hands a block of audio to the device. Returns false when the block was too old.
    fn push(&self, epoch: u64, samples: Vec<f32>, frames: usize, started: &mut bool) -> bool {
        if frames == 0 {
            return true;
        }
        let block = Block { samples, frames, channels: self.device_channels };
        if !self.shared.queue.push(epoch, block) {
            return false;
        }
        if !*started {
            *started = true;
            // The audio is here: the engine is playing.
            let next = state::next(self.shared.state(), state::Change::AudioStarted);
            self.shared.set_state(next);
        }
        true
    }
}

/// Moves the demuxer to `target`.
fn seek(format: &mut Box<dyn FormatReader>, target: Duration) -> Result<(), SymphoniaError> {
    let seconds = target.as_secs_f64();
    let time = Time::new(seconds as u64, seconds.fract());
    format.seek(SeekMode::Accurate, SeekTo::Time { time, track_id: None })?;
    Ok(())
}

/// Copies a decoded buffer into plain f32 planes.
fn to_planar(buffer: &AudioBufferRef<'_>) -> Vec<Vec<f32>> {
    match buffer {
        AudioBufferRef::F32(planes) => planes.planes().planes().iter().map(|plane| plane.to_vec()).collect(),
        other => {
            let mut converted = other.make_equivalent::<f32>();
            other.convert(&mut converted);
            converted.planes().planes().iter().map(|plane| plane.to_vec()).collect()
        }
    }
}

/// Makes the planes have exactly `channels` channels.
///
/// It follows the same rules as [`dsp::remix`]: stereo to mono averages, mono to stereo copies.
fn fit_channels(planes: Vec<Vec<f32>>, channels: usize) -> Vec<Vec<f32>> {
    if planes.len() == channels {
        return planes;
    }
    let frames = planes.first().map_or(0, |plane| plane.len());
    let mut out = vec![vec![0.0f32; frames]; channels];
    match planes.len() {
        0 => {}
        1 => {
            for channel in 0..channels {
                out[channel].copy_from_slice(&planes[0]);
            }
        }
        source => {
            for frame in 0..frames {
                let average: f32 = planes.iter().map(|plane| plane[frame]).sum::<f32>() / source as f32;
                for channel in 0..channels {
                    out[channel][frame] = if channels == 1 {
                        average
                    } else if channel < source {
                        planes[channel][frame]
                    } else {
                        0.0
                    };
                }
            }
        }
    }
    out
}

/// Guesses the container from the extension of the URL.
fn guess_extension(url: &str) -> Option<String> {
    const KNOWN: [&str; 7] = ["m4a", "mp4", "m4b", "m4v", "mov", "aac", "mp3"];
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let extension = path.rsplit('/').next()?.rsplit_once('.').map(|(_, ext)| ext)?;
    let extension = extension.to_ascii_lowercase();
    if KNOWN.contains(&extension.as_str()) {
        Some(extension)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use crate::State;
    use std::time::Instant;

    /// The extension comes from the URL.
    #[test]
    fn the_extension_comes_from_the_url() {
        assert_eq!(guess_extension("https://x.test/audio.m4a"), Some("m4a".into()));
        assert_eq!(guess_extension("https://x.test/audio.m4a?token=1"), Some("m4a".into()));
        assert_eq!(guess_extension("https://x.test/audio"), None);
        assert_eq!(guess_extension("https://x.test/audio."), None);
        assert_eq!(guess_extension("https://x.test/audio.exe"), None);
    }

    #[test]
    fn planes_are_fitted_to_the_channel_count() {
        let mono = vec![vec![1.0, 2.0]];
        let stereo = fit_channels(mono, 2);
        assert_eq!(stereo, vec![vec![1.0, 2.0], vec![1.0, 2.0]]);

        let stereo = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let mono = fit_channels(stereo, 1);
        assert_eq!(mono, vec![vec![2.0, 3.0]]);
    }

    /// A fragmented MP4 (the format YouTube sends) plays, and the state says so.
    ///
    /// It needs a socket, so it is ignored by default. Run it by hand with
    /// `cargo test -p play-audio -- --ignored`.
    #[test]
    #[ignore]
    fn a_fragmented_mp4_plays_over_http() {
        let server = testing::serve(testing::fixture("fragmented.m4a"));

        let shared = Arc::new(Shared::new(44_100.0));
        let epoch = shared.queue.reset();
        shared.set_state(State::Loading);

        let mut handle = spawn(
            shared.clone(),
            Source {
                url: server.url.clone(),
                headers: Vec::new(),
                content_length: None,
                gain_db: 0.0,
            },
            epoch,
            44_100,
            2,
        )
        .expect("el decodificador arranca");

        // The audio arrives and the engine says it is playing.
        let deadline = Instant::now() + Duration::from_secs(30);
        while shared.state() != State::Playing {
            assert!(
                Instant::now() < deadline,
                "el audio nunca llegó: {:?}",
                events(&shared)
            );
            std::thread::sleep(Duration::from_millis(50));
        }

        // The audio is not silence.
        let (samples, frames) = shared.queue.take(44_100);
        assert!(frames > 0, "no llegó audio");
        assert!(
            samples.iter().any(|sample| sample.abs() > 0.01),
            "el audio llegó mudo"
        );

        // The duration of the file is known.
        let duration = events(&shared).iter().find_map(|event| match event {
            crate::Event::Duration(duration) => Some(*duration),
            _ => None,
        });
        assert_eq!(duration, Some(Duration::from_millis(2000)), "duración incorrecta");

        // Nothing went wrong on the way.
        let errors: Vec<String> = events(&shared)
            .iter()
            .filter_map(|event| match event {
                crate::Event::Error(message) => Some(message.clone()),
                _ => None,
            })
            .collect();
        assert!(errors.is_empty(), "hubo errores: {errors:?}");

        handle.stop();
    }

    /// A jump works with a fragmented MP4 too.
    ///
    /// It needs a socket, so it is ignored by default. Run it by hand with
    /// `cargo test -p play-audio -- --ignored`.
    #[test]
    #[ignore]
    fn a_fragmented_mp4_can_be_jumped() {
        let server = testing::serve(testing::fixture("fragmented.m4a"));

        let shared = Arc::new(Shared::new(44_100.0));
        let epoch = shared.queue.reset();
        shared.set_state(State::Loading);

        let mut handle = spawn(
            shared.clone(),
            Source {
                url: server.url.clone(),
                headers: Vec::new(),
                content_length: None,
                gain_db: 0.0,
            },
            epoch,
            44_100,
            2,
        )
        .expect("el decodificador arranca");

        // The decoder takes the jump in its first round: the audio comes from the middle.
        let epoch = shared.queue.reset();
        shared.request_seek(epoch, Duration::from_secs(1));

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let (_, frames) = shared.queue.take(44_100);
            if frames > 0 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "el salto no trajo audio: {:?}",
                events(&shared)
            );
            std::thread::sleep(Duration::from_millis(50));
        }

        let errors: Vec<String> = events(&shared)
            .iter()
            .filter_map(|event| match event {
                crate::Event::Error(message) => Some(message.clone()),
                _ => None,
            })
            .collect();
        assert!(errors.is_empty(), "hubo errores: {errors:?}");

        handle.stop();
    }

    /// The events the engine has reported so far.
    fn events(shared: &Shared) -> Vec<crate::Event> {
        shared.events.lock().unwrap_or_else(|err| err.into_inner()).clone()
    }
}
