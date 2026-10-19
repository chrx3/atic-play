use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Context, Hsla, SharedString, Subscription, Task, rgb};
use material_colors::image::{Image, ImageReader};
use ytm_core::{AudioPlayer, Client, Queue, RepeatMode, Track};

use crate::media_session::{MediaEvent, MediaSession, MediaState, SeekDirection};
use crate::settings;

const COVER_SIZE: u32 = 544;
/// How often the progress follows the playback position and track ends are noticed.
const TICK: Duration = Duration::from_millis(250);
/// Like most players, "previous" restarts the track unless it has only just begun.
const RESTART_THRESHOLD: Duration = Duration::from_secs(3);
/// Fetch the radio when fewer tracks than this are left, so playback never runs dry.
const RADIO_REFILL_AT: usize = 3;
/// How often (in ticks) the system media session hears the position while playing.
const MEDIA_SYNC_TICKS: u32 = 8;
/// What a media key "seek" without an amount moves.
const MEDIA_SEEK_STEP: Duration = Duration::from_secs(10);

type AudioTask = Task<anyhow::Result<Vec<u8>>>;

/// What is playing and what comes next. Views observe it.
pub struct Playback {
    client: Client,
    /// `None` when no audio device could be opened; `error` explains why.
    player: Option<AudioPlayer>,
    queue: Queue,
    loading: bool,
    /// Bumped on every load, so a slow download cannot replace a newer choice.
    generation: u64,
    /// The player holds the current track (playing or paused), as opposed to loading or ended.
    started: bool,
    /// The queue ran out while the radio was still on its way.
    ended: bool,
    /// The track whose radio was last requested, so it is requested once.
    radio_for: Option<String>,
    /// The next track, downloading while the current one plays so skipping to it is instant.
    prefetch: Option<(String, AudioTask)>,
    seed: Option<Hsla>,
    /// The current track's cover, for the system media session.
    cover: Option<PathBuf>,
    opening: Option<String>,
    volume: f32,
    muted: bool,
    /// Where to start the restored track the first time it plays.
    resume_at: Option<Duration>,
    error: Option<SharedString>,
    media: Option<MediaSession>,
    media_tried: bool,
    /// The track and state the media session last heard, to only send changes.
    media_sent: Option<(String, bool)>,
    ticks: u32,
    /// Settings changed since they were last written; written on the next tick.
    settings_dirty: bool,
    _ticker: Task<()>,
    _quit: Subscription,
}

impl Playback {
    pub fn new(client: Client, cx: &mut Context<Self>) -> Self {
        let (player, error) = match AudioPlayer::new() {
            Ok(player) => (Some(player), None),
            Err(err) => (
                None,
                Some(format!("No se pudo abrir el audio: {err:#}").into()),
            ),
        };
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        });
        let quit = cx.on_app_quit(|this, cx| {
            this.save_settings(cx);
            async {}
        });

        // Pick up where the last session left off, paused.
        let saved = settings::get(cx);
        let mut queue = Queue::default();
        queue.restore(saved.queue.clone(), saved.queue_index, saved.shuffle);
        queue.set_repeat(saved.repeat);
        let resume_at =
            (saved.position_secs > 1.).then(|| Duration::from_secs_f32(saved.position_secs));

        let playback = Playback {
            client,
            player,
            queue,
            loading: false,
            generation: 0,
            started: false,
            ended: false,
            radio_for: None,
            prefetch: None,
            seed: None,
            cover: None,
            opening: None,
            volume: saved.volume.unwrap_or(1.),
            muted: saved.muted,
            resume_at,
            error,
            media: None,
            media_tried: false,
            media_sent: None,
            ticks: 0,
            settings_dirty: false,
            _ticker: ticker,
            _quit: quit,
        };
        playback.apply_volume();
        playback
    }

    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    pub fn current(&self) -> Option<&Track> {
        self.queue.current()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_playing(&self) -> bool {
        self.started
            && self
                .player
                .as_ref()
                .is_some_and(|player| !player.is_paused())
    }

    pub fn position(&self) -> Duration {
        match &self.player {
            Some(player) if self.started => player.position(),
            // A restored track shows where it will resume.
            _ => self.resume_at.unwrap_or_default(),
        }
    }

    pub fn duration(&self) -> Option<Duration> {
        let secs = self.current()?.duration_secs?;
        Some(Duration::from_secs(secs.into()))
    }

    /// How loud the music is right now (0 to about 1), for visualizers; 0 unless playing.
    /// The volume setting does not affect it.
    pub fn audio_level(&self) -> f32 {
        match &self.player {
            Some(player) if self.is_playing() => player.level(),
            _ => 0.,
        }
    }

    /// The theme seed taken from the current cover.
    pub fn seed(&self) -> Option<Hsla> {
        self.seed
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// The album or artist whose tracks are being fetched to play.
    pub fn opening(&self) -> Option<&str> {
        self.opening.as_deref()
    }

    pub fn play_album(&mut self, album_id: String, cx: &mut Context<Self>) {
        let tracks = self.client.album_tracks(album_id.clone());
        self.play_fetched(album_id, tracks, cx);
    }

    pub fn play_artist(&mut self, artist_id: String, cx: &mut Context<Self>) {
        let tracks = self.client.artist_top_tracks(artist_id.clone());
        self.play_fetched(artist_id, tracks, cx);
    }

    fn play_fetched(
        &mut self,
        id: String,
        tracks: impl Future<Output = anyhow::Result<Vec<Track>>> + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.opening.is_some() {
            return;
        }
        self.opening = Some(id.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let tracks = tracks.await;
            this.update(cx, |this, cx| {
                this.opening = None;
                match tracks {
                    Ok(tracks) if !tracks.is_empty() => this.play_tracks(tracks, 0, cx),
                    Ok(_) => this.error = Some("No hay canciones para reproducir".into()),
                    Err(err) => {
                        log::warn!("could not open {id}: {err:#}");
                        this.error = Some(format!("No se pudo abrir: {err:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Replaces the queue and plays `tracks[start]`.
    pub fn play_tracks(&mut self, tracks: Vec<Track>, start: usize, cx: &mut Context<Self>) {
        self.queue.replace(tracks, start);
        self.radio_for = None;
        self.load_current(cx);
    }

    /// Turns shuffle on and plays `tracks` from a random one, the rest shuffled after it.
    pub fn play_shuffled(&mut self, mut tracks: Vec<Track>, cx: &mut Context<Self>) {
        if tracks.is_empty() {
            return;
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos() as usize);
        // The queue shuffles only what follows the current track, so start from the front.
        let first = nanos % tracks.len();
        tracks.swap(0, first);
        self.queue.set_shuffle(true);
        self.play_tracks(tracks, 0, cx);
    }

    pub fn play_index(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.queue.jump_to(index).is_some() {
            self.load_current(cx);
        }
    }

    pub fn next(&mut self, cx: &mut Context<Self>) {
        if self.queue.advance(false).is_some() {
            self.load_current(cx);
        }
    }

    pub fn previous(&mut self, cx: &mut Context<Self>) {
        let before = self.queue.current_index();
        if self.position() > RESTART_THRESHOLD || self.queue.previous().is_none() {
            self.seek(Duration::ZERO, cx);
        } else if self.queue.current_index() == before {
            // First track without repeat: there is nothing before it, so restart it.
            self.seek(Duration::ZERO, cx);
        } else {
            self.load_current(cx);
        }
    }

    pub fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        let Some(player) = &self.player else {
            return;
        };
        if self.started {
            player.toggle_pause();
            self.sync_media(true);
            cx.notify();
        } else if !self.loading && self.current().is_some() {
            // Restored from the last session, or the queue ended: play the current track.
            self.load_current(cx);
        }
    }

    pub fn seek_fraction(&mut self, fraction: f32, cx: &mut Context<Self>) {
        if let Some(duration) = self.duration() {
            self.seek(duration.mul_f32(fraction.clamp(0., 1.)), cx);
        }
    }

    fn seek(&mut self, position: Duration, cx: &mut Context<Self>) {
        if let Some(player) = &self.player
            && self.started
        {
            player.seek(position);
            self.sync_media(true);
            cx.notify();
        }
    }

    pub fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.volume = volume.clamp(0., 1.);
        self.muted = false;
        self.apply_volume();
        self.settings_dirty = true;
        cx.notify();
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        self.apply_volume();
        self.settings_dirty = true;
        cx.notify();
    }

    pub fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        self.queue.set_shuffle(!self.queue.shuffle());
        self.settings_dirty = true;
        self.prefetch_next(cx);
        cx.notify();
    }

    pub fn cycle_repeat(&mut self, cx: &mut Context<Self>) {
        self.queue.cycle_repeat();
        self.settings_dirty = true;
        self.refill_radio(cx);
        self.prefetch_next(cx);
        cx.notify();
    }

    fn apply_volume(&self) {
        if let Some(player) = &self.player {
            player.set_volume(if self.muted { 0. } else { self.volume });
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        self.ticks = self.ticks.wrapping_add(1);
        self.connect_media(cx);
        self.handle_media_events(cx);

        let finished = self.started && self.player.as_ref().is_some_and(AudioPlayer::is_idle);
        if finished {
            self.started = false;
            if self.queue.advance(true).is_some() {
                self.load_current(cx);
            } else {
                self.ended = true;
                self.sync_media(true);
            }
            cx.notify();
        } else if self.is_playing() {
            self.sync_media(self.ticks.is_multiple_of(MEDIA_SYNC_TICKS));
            cx.notify();
        }

        if self.settings_dirty {
            self.save_settings(cx);
        }
    }

    fn load_current(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.current().cloned() else {
            return;
        };
        let Some(player) = &self.player else {
            return;
        };
        player.stop();
        self.generation += 1;
        let generation = self.generation;
        self.started = false;
        self.ended = false;
        self.loading = true;
        self.error = None;
        self.settings_dirty = true;
        cx.notify();

        // The prefetched download is used when it is for this track; any other is dropped
        // (it still finishes in the background and lands in the disk cache).
        let audio = match self.prefetch.take() {
            Some((id, task)) if id == track.id => task,
            _ => cx
                .background_executor()
                .spawn(self.client.fetch_audio(track.id.clone())),
        };
        let cover = track
            .cover_url
            .clone()
            .map(|url| self.client.fetch_cover(track.id.clone(), url, COVER_SIZE));
        cx.spawn(async move |this, cx| {
            let audio = audio.await;
            let cover = match cover {
                Some(cover) => cover.await.ok(),
                None => None,
            };
            let seed = match cover.clone() {
                Some(path) => {
                    cx.background_executor()
                        .spawn(async move { seed_color(&path) })
                        .await
                }
                None => None,
            };

            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                let started = audio.and_then(|bytes| match &this.player {
                    Some(player) => player.play(bytes),
                    None => Ok(()),
                });
                match started {
                    Ok(()) => {
                        this.started = true;
                        this.apply_volume();
                        if let (Some(at), Some(player)) = (this.resume_at.take(), &this.player) {
                            player.seek(at);
                        }
                        if seed.is_some() {
                            this.seed = seed;
                        }
                        this.cover = cover;
                        this.sync_media(true);
                        this.refill_radio(cx);
                        this.prefetch_next(cx);
                    }
                    Err(err) => {
                        this.error = Some(format!("No se pudo reproducir: {err:#}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts downloading the track that plays after the current one.
    fn prefetch_next(&mut self, cx: &mut Context<Self>) {
        let next = match self.queue.repeat() {
            RepeatMode::One => None,
            RepeatMode::Off => self.queue.upcoming().first(),
            RepeatMode::All => self
                .queue
                .upcoming()
                .first()
                .or_else(|| self.queue.tracks().first()),
        };
        let Some(next) = next.map(|track| track.id.clone()) else {
            return;
        };
        if self.current().is_some_and(|track| track.id == next)
            || self.prefetch.as_ref().is_some_and(|(id, _)| *id == next)
        {
            return;
        }
        let fetch = self.client.fetch_audio(next.clone());
        self.prefetch = Some((next, cx.background_executor().spawn(fetch)));
    }

    /// Keeps the queue going with the radio of the current track, as YouTube Music does.
    fn refill_radio(&mut self, cx: &mut Context<Self>) {
        if self.queue.repeat() != RepeatMode::Off || self.queue.upcoming().len() >= RADIO_REFILL_AT
        {
            return;
        }
        let Some(track_id) = self.current().map(|track| track.id.clone()) else {
            return;
        };
        if self.radio_for.as_ref() == Some(&track_id) {
            return;
        }
        self.radio_for = Some(track_id.clone());

        let radio = self.client.radio(track_id.clone());
        cx.spawn(async move |this, cx| match radio.await {
            Ok(tracks) => {
                this.update(cx, |this, cx| {
                    this.queue.append(tracks);
                    this.settings_dirty = true;
                    if this.ended && this.queue.advance(false).is_some() {
                        this.load_current(cx);
                    } else {
                        this.prefetch_next(cx);
                    }
                    cx.notify();
                })
                .ok();
            }
            Err(err) => log::warn!("radio for {track_id}: {err:#}"),
        })
        .detach();
    }

    /// Registers with the system media session once the main window exists.
    fn connect_media(&mut self, cx: &mut Context<Self>) {
        if self.media_tried {
            return;
        }
        let Some(window) = cx.windows().into_iter().next() else {
            return;
        };
        self.media_tried = true;
        self.media = window
            .update(cx, |_, window, _| MediaSession::new(window))
            .ok()
            .flatten();
        self.media_sent = None;
        self.sync_media(true);
    }

    fn handle_media_events(&mut self, cx: &mut Context<Self>) {
        let Some(media) = &self.media else {
            return;
        };
        for event in media.poll() {
            match event {
                MediaEvent::Play if !self.is_playing() => self.toggle_pause(cx),
                MediaEvent::Pause | MediaEvent::Stop if self.is_playing() => self.toggle_pause(cx),
                MediaEvent::Toggle => self.toggle_pause(cx),
                MediaEvent::Next => self.next(cx),
                MediaEvent::Previous => self.previous(cx),
                MediaEvent::SetPosition(position) => self.seek(position.0, cx),
                MediaEvent::Seek(direction) => self.seek_by(direction, MEDIA_SEEK_STEP, cx),
                MediaEvent::SeekBy(direction, amount) => self.seek_by(direction, amount, cx),
                MediaEvent::Raise => cx.activate(true),
                _ => {}
            }
        }
    }

    fn seek_by(&mut self, direction: SeekDirection, amount: Duration, cx: &mut Context<Self>) {
        let position = match direction {
            SeekDirection::Forward => self.position() + amount,
            SeekDirection::Backward => self.position().saturating_sub(amount),
        };
        self.seek(position, cx);
    }

    /// Tells the system media session what is playing. Without `force`, only changes are sent.
    fn sync_media(&mut self, force: bool) {
        let state = match self.current() {
            Some(track) if self.started => Some((track.id.clone(), self.is_playing())),
            _ => None,
        };
        if !force && state == self.media_sent {
            return;
        }
        let Some(media) = &mut self.media else {
            return;
        };
        match (&state, self.queue.current()) {
            (Some((id, playing)), Some(track)) => {
                if self.media_sent.as_ref().map(|(sent, _)| sent) != Some(id) {
                    media.set_track(track, self.cover.as_deref());
                }
                let position = self
                    .player
                    .as_ref()
                    .map(AudioPlayer::position)
                    .unwrap_or_default();
                media.set_state(if *playing {
                    MediaState::Playing(position)
                } else {
                    MediaState::Paused(position)
                });
            }
            _ => media.set_state(MediaState::Stopped),
        }
        self.media_sent = state;
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_dirty = false;
        let position = self.position().as_secs_f32();
        let queue = self.queue.tracks().to_vec();
        let queue_index = self.queue.current_index();
        let (volume, muted) = (self.volume, self.muted);
        let (shuffle, repeat) = (self.queue.shuffle(), self.queue.repeat());
        settings::update(cx, |settings| {
            settings.volume = Some(volume);
            settings.muted = muted;
            settings.shuffle = shuffle;
            settings.repeat = repeat;
            settings.queue = queue;
            settings.queue_index = queue_index;
            settings.position_secs = position;
        });
    }
}

/// The cover color best suited for a theme, as Material You picks it from a wallpaper.
fn seed_color(cover: &Path) -> Option<Hsla> {
    // Decoded here rather than with `ImageReader::open`, which panics on a damaged file; on a
    // GPUI worker thread that panic aborts the whole app.
    let decoded = image::ImageReader::open(cover)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(image::ImageError::IoError)
        .and_then(|reader| reader.decode())
        .inspect_err(|err| log::warn!("could not read {}: {err}", cover.display()))
        .ok()?;
    // Quantizing a small copy gives the same seed and is much faster.
    let small = image::imageops::thumbnail(&decoded.into_rgba8(), 128, 128);
    let argb = ImageReader::extract_color(&Image::new(small));
    let packed = (u32::from(argb.red) << 16) | (u32::from(argb.green) << 8) | u32::from(argb.blue);
    Some(rgb(packed).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damaged_cover_gives_no_seed_instead_of_panicking() {
        let path = std::env::temp_dir().join(format!("atic-play-test-{}.jpg", std::process::id()));
        // A JPEG header cut short, as a cover read while still being written looks.
        std::fs::write(&path, [0xFF, 0xD8, 0xFF, 0xE0, 0x00]).unwrap();

        assert!(seed_color(&path).is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn seed_comes_from_the_cover_colors() {
        let path = std::env::temp_dir().join(format!("atic-play-test-{}.png", std::process::id()));
        image::RgbImage::from_pixel(32, 32, image::Rgb([200, 30, 30]))
            .save(&path)
            .unwrap();

        let seed = seed_color(&path).expect("a valid cover has a seed");
        std::fs::remove_file(&path).ok();
        // A red cover gives a red seed (hue near 0 or 1).
        assert!(seed.h < 0.05 || seed.h > 0.95, "hue {}", seed.h);
    }
}
