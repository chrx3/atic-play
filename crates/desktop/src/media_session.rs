//! The system media session: Windows SMTC (MPRIS on Linux) through souvlaki. It is what puts the
//! track in Atic's "Ahora suena", the Windows volume flyout and the media keys.
//!
//! The session runs on a thread of its own. Its calls cross into another process, and on
//! Windows a blocking COM call made from the UI thread pumps window messages while it waits: GPUI
//! would then run queued tasks in the middle of an update, which fails them ("RefCell already
//! borrowed") and can silently drop work such as starting the next track.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
};
use ytm_core::Track;

pub use souvlaki::{MediaControlEvent as MediaEvent, SeekDirection};

pub enum MediaState {
    Playing(Duration),
    Paused(Duration),
    Stopped,
}

/// What the app tells the session thread.
enum Command {
    SetTrack {
        title: String,
        artist: String,
        album: Option<String>,
        cover: Option<PathBuf>,
        duration: Option<Duration>,
    },
    SetState(MediaState),
}

pub struct MediaSession {
    commands: Sender<Command>,
    events: Receiver<MediaControlEvent>,
}

impl MediaSession {
    /// Registers the app with the OS. Windows ties the session to the app's main window.
    pub fn new(window: &Window) -> Option<Self> {
        // The window handle crosses to the session thread as a number: `HWND`s are valid from
        // any thread of the process.
        let hwnd = match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
            _ => None,
        };
        let (commands, command_queue) = channel();
        let (event_sender, events) = channel();
        let (started, started_result) = channel();
        std::thread::Builder::new()
            .name("media-session".into())
            .spawn(move || run(hwnd, command_queue, event_sender, started))
            .inspect_err(|err| log::warn!("media session thread: {err}"))
            .ok()?;
        // Wait for registration, so a failure shows up as no session at all.
        started_result
            .recv()
            .ok()?
            .then_some(MediaSession { commands, events })
    }

    /// Events from the media keys and other apps since the last call.
    pub fn poll(&self) -> Vec<MediaControlEvent> {
        self.events.try_iter().collect()
    }

    pub fn set_track(&mut self, track: &Track, cover: Option<&Path>) {
        self.send(Command::SetTrack {
            title: track.title.clone(),
            artist: track.artists.clone(),
            album: track.album.clone(),
            cover: cover.map(Path::to_path_buf),
            duration: track
                .duration_secs
                .map(|secs| Duration::from_secs(secs.into())),
        });
    }

    pub fn set_state(&mut self, state: MediaState) {
        self.send(Command::SetState(state));
    }

    fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            log::warn!("media session thread is gone");
        }
    }
}

/// The session thread: registers, reports whether it worked, then applies commands until the
/// app drops its [`MediaSession`].
fn run(
    hwnd: Option<isize>,
    commands: Receiver<Command>,
    events: Sender<MediaControlEvent>,
    started: Sender<bool>,
) {
    let config = PlatformConfig {
        display_name: crate::APP_NAME,
        dbus_name: "atic_play",
        hwnd: hwnd.map(|hwnd| hwnd as *mut c_void),
    };
    let controls = MediaControls::new(config).and_then(|mut controls| {
        // Called from an OS thread; the app picks the events up on its next tick.
        controls
            .attach(move |event| {
                events.send(event).ok();
            })
            .map(|()| controls)
    });
    let mut controls = match controls {
        Ok(controls) => controls,
        Err(err) => {
            log::warn!("media session: {err:?}");
            started.send(false).ok();
            return;
        }
    };
    started.send(true).ok();

    for command in commands {
        let result = match command {
            Command::SetTrack {
                title,
                artist,
                album,
                cover,
                duration,
            } => {
                let cover_url = cover.map(|path| format!("file://{}", path.display()));
                controls.set_metadata(MediaMetadata {
                    title: Some(&title),
                    artist: Some(&artist),
                    album: album.as_deref(),
                    cover_url: cover_url.as_deref(),
                    duration,
                })
            }
            Command::SetState(state) => controls.set_playback(match state {
                MediaState::Playing(position) => MediaPlayback::Playing {
                    progress: Some(MediaPosition(position)),
                },
                MediaState::Paused(position) => MediaPlayback::Paused {
                    progress: Some(MediaPosition(position)),
                },
                MediaState::Stopped => MediaPlayback::Stopped,
            }),
        };
        if let Err(err) = result {
            log::warn!("media session: {err:?}");
        }
    }
}
