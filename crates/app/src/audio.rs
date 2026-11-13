//! The audio engine, wrapped so that a missing, broken or unfinished engine never takes the app
//! down: the UI keeps working and says what happened.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::time::Duration;

use play_audio::{Event, Player, Source};

use crate::source::StreamInfo;

pub struct Engine {
    player: Option<Rc<Player>>,
    error: Option<String>,
}

impl Engine {
    pub fn new() -> Self {
        match catch_unwind(|| Player::new()) {
            Ok(Ok(player)) => {
                log::info!("audio engine ready");
                Self {
                    player: Some(Rc::new(player)),
                    error: None,
                }
            }
            Ok(Err(err)) => {
                log::error!("audio engine could not start: {err}");
                Self {
                    player: None,
                    error: Some(format!("{err}")),
                }
            }
            Err(_) => {
                log::error!("audio engine panicked while starting");
                Self {
                    player: None,
                    error: Some("el motor de audio todavía no está disponible".into()),
                }
            }
        }
    }

    pub fn available(&self) -> bool {
        self.player.is_some()
    }

    /// Why there is no sound, in words the UI can show.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn with(&self, f: impl FnOnce(&Player)) -> bool {
        match &self.player {
            Some(player) => catch_unwind(AssertUnwindSafe(|| f(player))).is_ok(),
            None => false,
        }
    }

    pub fn load(&self, stream: &StreamInfo) -> bool {
        let source = Source {
            url: stream.url.clone(),
            headers: stream.headers.clone(),
            content_length: stream.content_length,
            gain_db: stream.gain_db,
        };
        self.with(|player| player.load(source))
    }

    pub fn play(&self) -> bool {
        self.with(|player| player.play())
    }

    pub fn pause(&self) -> bool {
        self.with(|player| player.pause())
    }

    pub fn stop(&self) -> bool {
        self.with(|player| player.stop())
    }

    pub fn seek(&self, position: Duration) -> bool {
        self.with(|player| player.seek(position))
    }

    pub fn set_volume(&self, volume: f32) -> bool {
        self.with(|player| player.set_volume(volume.clamp(0., 1.)))
    }

    pub fn position(&self) -> Duration {
        match &self.player {
            Some(player) => {
                catch_unwind(AssertUnwindSafe(|| player.position())).unwrap_or(Duration::ZERO)
            }
            None => Duration::ZERO,
        }
    }

    /// Drains what the engine reported since the last call. Never panics.
    pub fn poll_events(&self) -> Vec<Event> {
        match &self.player {
            Some(player) => {
                catch_unwind(AssertUnwindSafe(|| player.poll_events())).unwrap_or_default()
            }
            None => Vec::new(),
        }
    }
}
