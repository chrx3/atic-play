//! YouTube Music access and audio playback, independent of any UI toolkit.

mod client;
mod player;
mod queue;

pub use client::{Album, Artist, Client, HomeFeed, Library, Playlist, PlaylistPage, Track};
pub use player::AudioPlayer;
pub use queue::{Queue, RepeatMode};
