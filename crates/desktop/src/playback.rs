use std::future::Future;
use std::path::Path;
use std::time::Duration;

use gpui::{Context, Hsla, SharedString, Task, rgb};
use material_colors::image::{FilterType, ImageReader};
use ytm_core::{AudioPlayer, Client, Queue, RepeatMode, Track};

const COVER_SIZE: u32 = 544;
/// How often the progress follows the playback position and track ends are noticed.
const TICK: Duration = Duration::from_millis(250);
/// Like most players, "previous" restarts the track unless it has only just begun.
const RESTART_THRESHOLD: Duration = Duration::from_secs(3);
/// Fetch the radio when fewer tracks than this are left, so playback never runs dry.
const RADIO_REFILL_AT: usize = 3;

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
    seed: Option<Hsla>,
    opening: Option<String>,
    volume: f32,
    muted: bool,
    error: Option<SharedString>,
    _ticker: Task<()>,
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
        Playback {
            client,
            player,
            queue: Queue::default(),
            loading: false,
            generation: 0,
            started: false,
            ended: false,
            radio_for: None,
            seed: None,
            opening: None,
            volume: 1.,
            muted: false,
            error,
            _ticker: ticker,
        }
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
            _ => Duration::ZERO,
        }
    }

    pub fn duration(&self) -> Option<Duration> {
        let secs = self.current()?.duration_secs?;
        Some(Duration::from_secs(secs.into()))
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
            cx.notify();
        } else if !self.loading && self.current().is_some() {
            // The queue ended: play the last track again.
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
            cx.notify();
        }
    }

    pub fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.volume = volume.clamp(0., 1.);
        self.muted = false;
        self.apply_volume();
        cx.notify();
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        self.apply_volume();
        cx.notify();
    }

    pub fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        self.queue.set_shuffle(!self.queue.shuffle());
        cx.notify();
    }

    pub fn cycle_repeat(&mut self, cx: &mut Context<Self>) {
        self.queue.cycle_repeat();
        self.refill_radio(cx);
        cx.notify();
    }

    fn apply_volume(&self) {
        if let Some(player) = &self.player {
            player.set_volume(if self.muted { 0. } else { self.volume });
        }
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let finished = self.started && self.player.as_ref().is_some_and(AudioPlayer::is_idle);
        if finished {
            self.started = false;
            if self.queue.advance(true).is_some() {
                self.load_current(cx);
            } else {
                self.ended = true;
            }
            cx.notify();
        } else if self.is_playing() {
            cx.notify();
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
        cx.notify();

        let audio = self.client.fetch_audio(track.id.clone());
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
            let seed = match cover {
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
                        if seed.is_some() {
                            this.seed = seed;
                        }
                        this.refill_radio(cx);
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
                    if this.ended && this.queue.advance(false).is_some() {
                        this.load_current(cx);
                    }
                    cx.notify();
                })
                .ok();
            }
            Err(err) => log::warn!("radio for {track_id}: {err:#}"),
        })
        .detach();
    }
}

/// The cover color best suited for a theme, as Material You picks it from a wallpaper.
fn seed_color(cover: &Path) -> Option<Hsla> {
    let mut image = ImageReader::open(cover)
        .inspect_err(|err| log::warn!("could not read {}: {err}", cover.display()))
        .ok()?;
    // Quantizing a small copy gives the same seed and is much faster.
    image.resize(128, 128, FilterType::Triangle);
    let argb = ImageReader::extract_color(&image);
    let packed = (u32::from(argb.red) << 16) | (u32::from(argb.green) << 8) | u32::from(argb.blue);
    Some(rgb(packed).into())
}
