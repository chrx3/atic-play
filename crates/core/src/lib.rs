//! YouTube Music access and audio playback, independent of any UI toolkit.

mod audio_cache;
mod client;
mod loudness;
mod lyrics;
mod player;
mod queue;

pub use client::{
    AccountInfo, Album, AlbumPage, Artist, ArtistPage, Audio, Client, HomeFeed, Library, More,
    Page, Playlist, PlaylistPage, SearchSuggestions, Track,
};
pub use lyrics::{LyricLine, Lyrics, LyricsClient};
pub use player::AudioPlayer;
pub use queue::{Queue, RepeatMode};
