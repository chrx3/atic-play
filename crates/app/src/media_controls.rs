//! The media panel of the system: what is playing, and the buttons and media keys that drive it.
//!
//! On Windows this is the SMTC (`SystemMediaTransportControls`), reached through the window it
//! belongs to (`ISystemMediaTransportControlsInterop::GetForWindow`), which is what makes the
//! panel, the lock screen and the play/pause key of a headset point at this app.
//!
//! The buttons arrive on a COM thread, so they only travel to the app through a channel: the
//! shell drains it on its tick and calls the model from there. On macOS nothing is wired yet; the
//! type stays, so the rest of the app does not care.
//!
//! Nothing here starts or stops sound by itself: it only reports, and forwards presses.

use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use crate::source::Track;

/// How often the position is pushed to the panel: it interpolates between updates, and a write
/// per tick would keep the UI thread busy for nothing.
const TIMELINE_EVERY: Duration = Duration::from_secs(5);

/// A button pressed in the media panel, a media key the system routed to us, or a jump to a
/// position asked for on the panel's timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    Play,
    Pause,
    Next,
    Previous,
    Stop,
    Seek(Duration),
}

/// What the media panel should say the app is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelState {
    Stopped,
    Paused,
    Playing,
}

/// What the panel should show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Now {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub thumbnail: Option<String>,
    pub playing: bool,
    pub position: Duration,
    pub duration: Duration,
}

impl Now {
    /// Builds it from the current track, when there is one.
    pub fn of(track: Option<&Track>, playing: bool, position: Duration, duration: Duration) -> Now {
        match track {
            Some(track) => Now {
                title: track.title.clone(),
                artist: track.artist.clone(),
                album: track.album.clone().unwrap_or_default(),
                thumbnail: track.thumbnail.clone(),
                playing,
                position,
                duration,
            },
            None => Now {
                title: String::new(),
                artist: String::new(),
                album: String::new(),
                thumbnail: None,
                playing: false,
                position: Duration::ZERO,
                duration: Duration::ZERO,
            },
        }
    }

    fn same_track(&self, other: &Now) -> bool {
        self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.thumbnail == other.thumbnail
            && self.duration == other.duration
    }

    /// Stopped when there is no song at all: that is what the panel shows as "nothing".
    pub fn state(&self) -> PanelState {
        if self.title.is_empty() {
            PanelState::Stopped
        } else if self.playing {
            PanelState::Playing
        } else {
            PanelState::Paused
        }
    }
}

pub struct MediaControls {
    sender: Sender<MediaCommand>,
    receiver: Receiver<MediaCommand>,
    /// What was pushed last, so the same title is not written again every tick.
    last: Option<Now>,
    last_timeline: Option<Instant>,
    #[cfg(target_os = "windows")]
    attached: Option<windows_smtc::Controls>,
}

impl MediaControls {
    pub fn new() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver,
            last: None,
            last_timeline: None,
            #[cfg(target_os = "windows")]
            attached: None,
        }
    }

    /// The buttons pressed since the last call, to be run on the UI thread.
    pub fn drain(&self) -> Vec<MediaCommand> {
        let mut commands = Vec::new();
        loop {
            match self.receiver.try_recv() {
                Ok(command) => commands.push(command),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        commands
    }

    /// Ties the panel to the window. `hwnd` is the raw window handle, in logical pixels' world:
    /// the same one the icon uses.
    ///
    /// Called on the UI thread, after the window exists. A failure is logged and the app keeps
    /// working; it just has no media panel.
    pub fn attach(&mut self, hwnd: isize) {
        #[cfg(target_os = "windows")]
        {
            if self.attached.is_some() {
                return;
            }
            match windows_smtc::Controls::attach(hwnd, self.sender.clone()) {
                Ok(controls) => {
                    log::info!("media panel: connected to the window");
                    self.attached = Some(controls);
                }
                Err(err) => log::warn!("media panel: could not connect: {err}"),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = hwnd;
            log::info!("media panel: not implemented on this system yet");
        }
    }

    pub fn attached(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            self.attached.is_some()
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    /// Reports what is playing. Only what changed reaches the system, and the position at most
    /// every [`TIMELINE_EVERY`].
    pub fn update(&mut self, now: &Now) {
        let same_track = self.last.as_ref().is_some_and(|last| last.same_track(now));
        let same_state = self
            .last
            .as_ref()
            .is_some_and(|last| last.state() == now.state());
        let timeline_due = self
            .last_timeline
            .is_none_or(|pushed| pushed.elapsed() >= TIMELINE_EVERY);
        if same_track && same_state && !timeline_due {
            return;
        }
        #[cfg(target_os = "windows")]
        {
            if let Some(controls) = &self.attached {
                if let Err(err) = controls.update(now, !same_track, !same_state) {
                    log::debug!("media panel: could not update: {err}");
                }
            }
        }
        self.last = Some(now.clone());
        self.last_timeline = Some(Instant::now());
    }

    /// Forgets what was pushed, so the next [`MediaControls::update`] writes everything again.
    pub fn invalidate(&mut self) {
        self.last = None;
        self.last_timeline = None;
    }
}

impl Default for MediaControls {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------- Windows

#[cfg(target_os = "windows")]
mod windows_smtc {
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    use windows::core::{HSTRING, Result as WinResult};
    use windows::Foundation::{TimeSpan, TypedEventHandler, Uri};
    use windows::Media::{
        MediaPlaybackStatus, MediaPlaybackType, PlaybackPositionChangeRequestedEventArgs,
        SystemMediaTransportControls, SystemMediaTransportControlsButton,
        SystemMediaTransportControlsButtonPressedEventArgs,
        SystemMediaTransportControlsTimelineProperties,
    };
    use windows::Storage::Streams::RandomAccessStreamReference;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;

    use super::{MediaCommand, Now, PanelState};

    fn ticks(time: Duration) -> TimeSpan {
        TimeSpan {
            Duration: (time.as_nanos() / 100) as i64,
        }
    }

    fn from_ticks(time: TimeSpan) -> Duration {
        Duration::from_nanos((time.Duration.max(0) as u64).saturating_mul(100))
    }

    pub struct Controls {
        controls: SystemMediaTransportControls,
        button_token: i64,
        seek_token: Option<i64>,
    }

    impl Controls {
        /// `GetForWindow` hands the window's own controller. COM is already initialized on this
        /// thread: GPUI calls `OleInitialize` when the app starts.
        pub fn attach(hwnd: isize, sender: Sender<MediaCommand>) -> WinResult<Self> {
            let interop: ISystemMediaTransportControlsInterop =
                windows::core::factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>()?;
            let controls: SystemMediaTransportControls =
                unsafe { interop.GetForWindow(HWND(hwnd as *mut core::ffi::c_void))? };

            controls.SetIsEnabled(true)?;
            controls.SetIsPlayEnabled(true)?;
            controls.SetIsPauseEnabled(true)?;
            controls.SetIsNextEnabled(true)?;
            controls.SetIsPreviousEnabled(true)?;
            controls.SetIsStopEnabled(true)?;
            controls.SetIsRecordEnabled(false)?;
            controls.SetIsFastForwardEnabled(false)?;
            controls.SetIsRewindEnabled(false)?;

            let button_sender = sender.clone();
            let handler = TypedEventHandler::new(
                move |_: windows::core::Ref<SystemMediaTransportControls>,
                      args: windows::core::Ref<
                    SystemMediaTransportControlsButtonPressedEventArgs,
                >| {
                    let Some(args) = args.as_ref() else {
                        return Ok(());
                    };
                    let button = args.Button()?;
                    let command = if button == SystemMediaTransportControlsButton::Play {
                        Some(MediaCommand::Play)
                    } else if button == SystemMediaTransportControlsButton::Pause {
                        Some(MediaCommand::Pause)
                    } else if button == SystemMediaTransportControlsButton::Stop {
                        Some(MediaCommand::Stop)
                    } else if button == SystemMediaTransportControlsButton::Next {
                        Some(MediaCommand::Next)
                    } else if button == SystemMediaTransportControlsButton::Previous {
                        Some(MediaCommand::Previous)
                    } else {
                        None
                    };
                    if let Some(command) = command {
                        // A closed receiver just means the app is closing.
                        let _ = button_sender.send(command);
                    }
                    Ok(())
                },
            );
            let button_token = controls.ButtonPressed(&handler)?;

            // The panel's timeline can ask to jump somewhere in the song; the app follows.
            let seek_handler = TypedEventHandler::new(
                move |_: windows::core::Ref<SystemMediaTransportControls>,
                      args: windows::core::Ref<PlaybackPositionChangeRequestedEventArgs>| {
                    if let Some(args) = args.as_ref() {
                        let position = args.RequestedPlaybackPosition()?;
                        let _ = sender.send(MediaCommand::Seek(from_ticks(position)));
                    }
                    Ok(())
                },
            );
            let seek_token = controls
                .PlaybackPositionChangeRequested(&seek_handler)
                .ok();

            Ok(Self {
                controls,
                button_token,
                seek_token,
            })
        }

        /// Writes the metadata (when the song changed) and the state (when it played or paused).
        pub fn update(&self, now: &Now, track_changed: bool, state_changed: bool) -> WinResult<()> {
            if state_changed {
                let status = match now.state() {
                    PanelState::Playing => MediaPlaybackStatus::Playing,
                    PanelState::Paused => MediaPlaybackStatus::Paused,
                    PanelState::Stopped => MediaPlaybackStatus::Stopped,
                };
                self.controls.SetPlaybackStatus(status)?;
            }

            if track_changed {
                let updater = self.controls.DisplayUpdater()?;
                updater.SetType(MediaPlaybackType::Music)?;
                let music = updater.MusicProperties()?;
                music.SetTitle(&HSTRING::from(now.title.as_str()))?;
                music.SetArtist(&HSTRING::from(now.artist.as_str()))?;
                music.SetAlbumArtist(&HSTRING::from(now.artist.as_str()))?;
                music.SetAlbumTitle(&HSTRING::from(now.album.as_str()))?;

                let thumbnail = now.thumbnail.as_deref().and_then(|url| {
                    // A cover that cannot be read is not a reason to fail the whole update.
                    let uri = Uri::CreateUri(&HSTRING::from(url)).ok()?;
                    RandomAccessStreamReference::CreateFromUri(&uri).ok()
                });
                if let Some(thumbnail) = thumbnail {
                    let _ = updater.SetThumbnail(&thumbnail);
                }
                updater.Update()?;
            }

            if track_changed || state_changed {
                let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
                timeline.SetStartTime(ticks(Duration::ZERO))?;
                timeline.SetMinSeekTime(ticks(Duration::ZERO))?;
                timeline.SetMaxSeekTime(ticks(now.duration))?;
                timeline.SetEndTime(ticks(now.duration))?;
                timeline.SetPosition(ticks(now.position.min(now.duration)))?;
                self.controls.UpdateTimelineProperties(&timeline)?;
            }

            Ok(())
        }
    }

    impl Drop for Controls {
        fn drop(&mut self) {
            let _ = self.controls.RemoveButtonPressed(self.button_token);
            if let Some(token) = self.seek_token {
                let _ = self.controls.RemovePlaybackPositionChangeRequested(token);
            }
            let _ = self.controls.SetIsEnabled(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now(title: &str, playing: bool) -> Now {
        Now {
            title: title.into(),
            artist: "Artist".into(),
            album: "Album".into(),
            thumbnail: Some("https://example.test/cover.jpg".into()),
            playing,
            position: Duration::from_secs(30),
            duration: Duration::from_secs(200),
        }
    }

    #[test]
    fn a_new_song_is_a_different_track_and_a_pause_is_not() {
        let first = now("One", true);
        assert!(first.same_track(&now("One", false)));
        assert!(!first.same_track(&now("Two", true)));
        assert!(now("One", true).same_track(&first));
    }

    #[test]
    fn a_track_builds_the_panel_text_and_an_empty_queue_does_not() {
        let track = Track {
            id: "abc".into(),
            title: "Glass Tide".into(),
            artist: "Nova Reyes".into(),
            album: Some("Midnight Lagoon".into()),
            album_id: None,
            duration: Some(Duration::from_secs(222)),
            thumbnail: Some("https://example.test/art.jpg".into()),
        };
        let shown = Now::of(Some(&track), true, Duration::from_secs(12), Duration::from_secs(222));
        assert_eq!(shown.title, "Glass Tide");
        assert_eq!(shown.album, "Midnight Lagoon");
        assert!(shown.playing);
        let empty = Now::of(None, false, Duration::ZERO, Duration::ZERO);
        assert!(empty.title.is_empty());
        assert!(!empty.playing);
    }

    #[test]
    fn the_panel_state_follows_the_song() {
        assert_eq!(Now::of(None, true, Duration::ZERO, Duration::ZERO).state(), PanelState::Stopped);
        assert_eq!(now("One", false).state(), PanelState::Paused);
        assert_eq!(now("One", true).state(), PanelState::Playing);
    }

    #[test]
    fn the_channel_carries_the_buttons() {
        let controls = MediaControls::new();
        controls.sender.send(MediaCommand::Next).unwrap();
        controls.sender.send(MediaCommand::Play).unwrap();
        assert_eq!(
            controls.drain(),
            vec![MediaCommand::Next, MediaCommand::Play]
        );
        assert!(controls.drain().is_empty());
        assert!(!controls.attached());
    }
}
