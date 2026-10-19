//! The system media session: Windows SMTC (MPRIS on Linux) through souvlaki. It is what puts the
//! track in Atic's "Ahora suena", the Windows volume flyout and the media keys.

use std::ffi::c_void;
use std::path::Path;
use std::sync::mpsc::{Receiver, channel};
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

pub struct MediaSession {
    controls: MediaControls,
    events: Receiver<MediaControlEvent>,
}

impl MediaSession {
    /// Registers the app with the OS. Windows ties the session to the app's main window.
    pub fn new(window: &Window) -> Option<Self> {
        let hwnd = match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut c_void),
            _ => None,
        };
        let config = PlatformConfig {
            display_name: crate::APP_NAME,
            dbus_name: "atic_play",
            hwnd,
        };
        let mut controls = MediaControls::new(config)
            .inspect_err(|err| log::warn!("media session: {err:?}"))
            .ok()?;
        let (sender, events) = channel();
        // Called from an OS thread; the app picks the events up on its next tick.
        controls
            .attach(move |event| {
                sender.send(event).ok();
            })
            .inspect_err(|err| log::warn!("media session events: {err:?}"))
            .ok()?;
        Some(MediaSession { controls, events })
    }

    /// Events from the media keys and other apps since the last call.
    pub fn poll(&self) -> Vec<MediaControlEvent> {
        self.events.try_iter().collect()
    }

    pub fn set_track(&mut self, track: &Track, cover: Option<&Path>) {
        let cover_url = cover.map(|path| format!("file://{}", path.display()));
        let metadata = MediaMetadata {
            title: Some(&track.title),
            artist: Some(&track.artists),
            album: track.album.as_deref(),
            cover_url: cover_url.as_deref(),
            duration: track
                .duration_secs
                .map(|secs| Duration::from_secs(secs.into())),
        };
        if let Err(err) = self.controls.set_metadata(metadata) {
            log::warn!("media session metadata: {err:?}");
        }
    }

    pub fn set_state(&mut self, state: MediaState) {
        let playback = match state {
            MediaState::Playing(position) => MediaPlayback::Playing {
                progress: Some(MediaPosition(position)),
            },
            MediaState::Paused(position) => MediaPlayback::Paused {
                progress: Some(MediaPosition(position)),
            },
            MediaState::Stopped => MediaPlayback::Stopped,
        };
        if let Err(err) = self.controls.set_playback(playback) {
            log::warn!("media session state: {err:?}");
        }
    }
}
