//! YouTube Music access and audio playback, independent of any UI toolkit.

mod audio_cache;
mod client;
mod lyrics;
mod player;
mod queue;

pub use client::{
    Album, AlbumPage, Artist, ArtistPage, Client, HomeFeed, Library, Playlist, PlaylistPage, Track,
};
pub use lyrics::{LyricLine, Lyrics, LyricsClient};
pub use player::AudioPlayer;
pub use queue::{Queue, RepeatMode};
