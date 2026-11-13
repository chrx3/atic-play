//! The audio engine: streams a remote file in byte ranges, decodes it and plays it.
//!
//! It knows nothing about YouTube: it gets a URL, its request headers and (optionally) its size.
//! The public API below is the contract the app codes against; keep the names.
//!
//! # How it works
//!
//! * A **fetch thread** downloads the file in 512 KiB ranges and keeps a megabyte ready ahead of
//!   the reader. Seeking jumps straight to the byte it needs.
//! * A **decoder thread** demuxes and decodes the file with Symphonia, resamples it when the file
//!   and the device disagree, and pushes the audio into a queue.
//! * The **device callback** takes the audio out of the queue, applies the volume with a short
//!   ramp so there are no clicks, and hands it to the sound card.
//!
//! None of the public methods block: the work happens on those threads and the results come back
//! as [`Event`]s.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

mod decode;
mod dsp;
mod http;
mod position;
mod queue;
mod state;

#[cfg(test)]
mod testing;

use queue::Queue;

/// How long the engine waits for the first audio of a track before it gives up.
const LOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// What to play. Mirrors the fields of `play_ytmusic::Stream` that matter here.
#[derive(Clone, Debug)]
pub struct Source {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub content_length: Option<u64>,
    /// Extra gain in dB (volume normalization). 0 = none.
    pub gain_db: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Loading,
    Playing,
    Paused,
    /// The track played to the end.
    Ended,
}

/// What the engine reports. `Player::poll_events` drains them; the app polls on each frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    State(State),
    /// The total length, as soon as the decoder knows it.
    Duration(Duration),
    Error(String),
}

/// Handle to the engine. Its methods never block on the network or the device: the work runs on
/// the engine's own threads.
pub struct Player {
    shared: Arc<Shared>,
    commands: mpsc::Sender<Command>,
}

/// What the app asks the engine to do.
enum Command {
    Load(Source),
    Play,
    Pause,
    Stop,
    Seek(Duration),
}

/// Everything the engine's threads share.
struct Shared {
    /// The state the app sees.
    state: Mutex<State>,
    /// Events waiting for `poll_events`.
    events: Mutex<Vec<Event>>,
    /// Decoded audio waiting for the device.
    queue: Arc<Queue>,
    /// Where we are in the track.
    clock: Mutex<position::Clock>,
    /// The moment the clock counts from.
    clock_zero: Instant,
    /// The volume the app asked for, as f32 bits.
    volume: AtomicU32,
    /// The extra gain of the current track in dB, as f32 bits.
    gain_db: AtomicU32,
    /// Volume times normalization, as f32 bits. The callback ramps towards this.
    gain: AtomicU32,
    /// A seek the decoder still has to apply.
    seek: Mutex<Option<(u64, Duration)>>,
    /// Milliseconds since the engine started when the last data arrived.
    progress: AtomicU32,
}

impl Shared {
    fn new(rate: f64) -> Self {
        Shared {
            state: Mutex::new(State::Idle),
            events: Mutex::new(Vec::new()),
            queue: Arc::new(Queue::new(rate as usize * 10)),
            clock: Mutex::new(position::Clock::new(rate)),
            clock_zero: Instant::now(),
            volume: AtomicU32::new(1.0f32.to_bits()),
            gain_db: AtomicU32::new(0.0f32.to_bits()),
            gain: AtomicU32::new(1.0f32.to_bits()),
            seek: Mutex::new(None),
            progress: AtomicU32::new(0),
        }
    }

    /// The time the clock counts from.
    fn now(&self) -> Duration {
        self.clock_zero.elapsed()
    }

    /// Notes that the track is coming along: the download or the decoding moved forward.
    fn note_progress(&self) {
        let millis = self.now().as_millis() as u32;
        self.progress.store(millis, Ordering::Relaxed);
    }

    /// How long ago the track last moved forward.
    fn since_progress(&self) -> Duration {
        let last = self.progress.load(Ordering::Relaxed);
        let now = self.now().as_millis() as u32;
        Duration::from_millis(now.wrapping_sub(last) as u64)
    }

    fn state(&self) -> State {
        *self.state.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Moves to `state` and tells the app about it.
    fn set_state(&self, state: State) {
        let mut current = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if *current == state {
            return;
        }
        *current = state;
        drop(current);
        self.push_event(Event::State(state));
    }

    fn push_event(&self, event: Event) {
        self.events.lock().unwrap_or_else(|err| err.into_inner()).push(event);
    }

    /// Reports a failure and stops playing.
    fn report_error(&self, message: String) {
        log::warn!("motor de audio: {message}");
        self.push_event(Event::Error(message));
        self.set_state(state::next(self.state(), state::Change::Failed));
    }

    /// The gain the callback should ramp towards.
    fn target_gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    fn set_volume(&self, volume: f32) {
        self.volume.store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        self.refresh_gain();
    }

    fn set_gain_db(&self, gain_db: f32) {
        self.gain_db.store(gain_db.to_bits(), Ordering::Relaxed);
        self.refresh_gain();
    }

    fn refresh_gain(&self) {
        let volume = f32::from_bits(self.volume.load(Ordering::Relaxed));
        let gain_db = f32::from_bits(self.gain_db.load(Ordering::Relaxed));
        let gain = volume * dsp::db_to_gain(gain_db);
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// The device took `frames` frames.
    fn render(&self, frames: u64) {
        let now = self.now();
        self.clock.lock().unwrap_or_else(|err| err.into_inner()).render(frames, now);
    }

    /// The app jumped to `to`.
    fn seek_clock(&self, to: Duration) {
        let now = self.now();
        self.clock.lock().unwrap_or_else(|err| err.into_inner()).seek(to, now);
    }

    fn set_clock_running(&self, running: bool) {
        let now = self.now();
        self.clock.lock().unwrap_or_else(|err| err.into_inner()).set_running(running, now);
    }

    fn position(&self) -> Duration {
        let now = self.now();
        self.clock.lock().unwrap_or_else(|err| err.into_inner()).position(now)
    }

    /// Asks the decoder to jump to `to`. The epoch tells it which jump this is.
    fn request_seek(&self, epoch: u64, to: Duration) {
        *self.seek.lock().unwrap_or_else(|err| err.into_inner()) = Some((epoch, to));
    }

    /// The jump the decoder has to apply, if any.
    fn take_seek(&self) -> Option<(u64, Duration)> {
        self.seek.lock().unwrap_or_else(|err| err.into_inner()).take()
    }
}

impl Player {
    /// Opens the default output device.
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device().context("no hay un dispositivo de salida de audio")?;
        let config = pick_config(&device)?;

        let shared = Arc::new(Shared::new(config.sample_rate().0 as f64));
        let (commands, receiver) = mpsc::channel();
        let engine_shared = Arc::clone(&shared);

        std::thread::Builder::new()
            .name("atic-audio-engine".into())
            .spawn(move || engine_loop(receiver, device, config, engine_shared))
            .context("no se pudo iniciar el motor de audio")?;

        Ok(Player { shared, commands })
    }

    /// Stops what is playing and starts `source`.
    pub fn load(&self, source: Source) {
        let _ = self.commands.send(Command::Load(source));
    }

    pub fn play(&self) {
        let _ = self.commands.send(Command::Play);
    }

    pub fn pause(&self) {
        let _ = self.commands.send(Command::Pause);
    }

    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    pub fn seek(&self, position: Duration) {
        let _ = self.commands.send(Command::Seek(position));
    }

    /// 0.0 ..= 1.0.
    pub fn set_volume(&self, volume: f32) {
        self.shared.set_volume(volume);
    }

    /// Current position, interpolated so it moves smoothly between audio callbacks.
    pub fn position(&self) -> Duration {
        self.shared.position()
    }

    pub fn state(&self) -> State {
        self.shared.state()
    }

    pub fn poll_events(&self) -> Vec<Event> {
        let mut events = self.shared.events.lock().unwrap_or_else(|err| err.into_inner());
        std::mem::take(&mut *events)
    }
}

/// Picks a configuration the engine can work with: f32 samples, at most two channels.
fn pick_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let default = device
        .default_output_config()
        .context("el dispositivo de audio no tiene una configuración de salida")?;
    if default.sample_format() == cpal::SampleFormat::F32 {
        return Ok(default);
    }

    let mut fallback = None;
    let ranges = device.supported_output_configs().context("no se pudieron ver los formatos del dispositivo")?;
    for range in ranges {
        if range.sample_format() != cpal::SampleFormat::F32 || range.channels() > 2 {
            continue;
        }
        let rate = range.max_sample_rate().min(cpal::SampleRate(48_000));
        let Some(config) = range.try_with_sample_rate(rate) else { continue };
        if range.channels() == 2 {
            return Ok(config);
        }
        fallback.get_or_insert(config);
    }
    Ok(fallback.unwrap_or(default))
}

/// Builds the output stream and the callback that feeds it.
fn build_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream> {
    let channels = config.channels() as usize;
    let rate = config.sample_rate().0 as f32;

    let data_shared = Arc::clone(shared);
    let error_shared = Arc::clone(shared);
    let mut ramp = dsp::Ramp::fixed(0.0);

    let stream = device
        .build_output_stream::<f32, _, _>(
            &config.config(),
            move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let frames = out.len() / channels.max(1);
                out.fill(0.0);

                // Take whatever the decoder has ready.
                let (samples, count) = data_shared.queue.take(frames);
                if count > 0 {
                    let wanted = count * channels;
                    out[..wanted].copy_from_slice(&samples[..wanted]);
                }

                // Apply the volume with a short ramp so there are no clicks.
                let target = data_shared.target_gain();
                ramp.set_target(target, rate, 15.0);
                for sample in out.iter_mut() {
                    *sample *= ramp.next();
                }

                // The device took these frames, the position moves on.
                data_shared.render(frames as u64);
            },
            move |err| {
                error_shared.report_error(format!("el dispositivo de audio falló: {err}"));
            },
            None,
        )
        .context("no se pudo abrir el dispositivo de audio")?;

    Ok(stream)
}

/// The engine's main loop: it owns the device and reacts to what the app asks for.
fn engine_loop(
    commands: mpsc::Receiver<Command>,
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    shared: Arc<Shared>,
) {
    let channels = config.channels() as usize;
    let rate = config.sample_rate().0;

    let stream = match build_stream(&device, &config, &shared) {
        Ok(stream) => stream,
        Err(err) => {
            shared.report_error(format!("{err:#}"));
            return;
        }
    };

    let mut decoder: Option<decode::DecoderHandle> = None;
    let mut running = false;

    loop {
        // Did the song finish?
        if running && shared.queue.is_drained() {
            shared.set_state(state::next(shared.state(), state::Change::Ended));
            shared.set_clock_running(false);
            running = false;
        }

        // Is the load stuck? The engine must never stay in `Loading` silently.
        if shared.state() == State::Loading && shared.since_progress() > LOAD_TIMEOUT {
            if let Some(mut handle) = decoder.take() {
                handle.stop();
            }
            shared.report_error("la carga se tardó demasiado: no llegó el audio a tiempo".into());
        }

        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(Command::Load(source)) => {
                if let Some(mut handle) = decoder.take() {
                    handle.stop();
                }
                shared.set_gain_db(source.gain_db);
                let epoch = shared.queue.reset();
                shared.seek_clock(Duration::ZERO);
                shared.set_clock_running(false);
                shared.set_state(State::Loading);
                shared.note_progress();

                match decode::spawn(shared.clone(), source, epoch, rate, channels) {
                    Ok(handle) => decoder = Some(handle),
                    Err(err) => {
                        shared.report_error(err);
                        continue;
                    }
                }

                let _ = stream.play();
                shared.set_clock_running(true);
                running = true;
            }
            Ok(Command::Play) => {
                if decoder.is_some() {
                    let _ = stream.play();
                    shared.set_clock_running(true);
                    running = true;
                    // While the track is loading the decoder says when the audio arrives.
                    shared.set_state(state::next(shared.state(), state::Change::Play));
                }
            }
            Ok(Command::Pause) => {
                let _ = stream.pause();
                shared.set_clock_running(false);
                shared.set_state(state::next(shared.state(), state::Change::Pause));
                running = false;
            }
            Ok(Command::Stop) => {
                if let Some(mut handle) = decoder.take() {
                    handle.stop();
                }
                shared.queue.reset();
                shared.set_clock_running(false);
                shared.seek_clock(Duration::ZERO);
                shared.set_state(State::Idle);
                running = false;
                let _ = stream.pause();
            }
            Ok(Command::Seek(to)) => {
                if decoder.is_none() {
                    continue;
                }
                let epoch = shared.queue.reset();
                shared.request_seek(epoch, to);
                shared.seek_clock(to);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if let Some(mut handle) = decoder.take() {
        handle.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine has to know when the track stops moving forward, to give up on it.
    #[test]
    fn the_engine_sees_the_track_moving_forward() {
        let shared = Shared::new(44_100.0);
        shared.note_progress();

        std::thread::sleep(Duration::from_millis(30));
        assert!(shared.since_progress() >= Duration::from_millis(30));

        shared.note_progress();
        assert!(shared.since_progress() < Duration::from_millis(30));
    }
}
