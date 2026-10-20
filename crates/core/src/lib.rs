//! YouTube Music access and audio playback, independent of any UI toolkit.

mod audio_cache;
mod client;
mod loudness;
mod lyrics;
mod player;
mod queue;

pub use client::{
    AccountInfo, Album, AlbumPage, Artist, ArtistPage, Audio, Client, Genre, GenrePage,
    GenreSection, HomeFeed, Library, More, Page, Playlist, PlaylistPage, SearchSuggestions, Track,
};
pub use lyrics::{LyricLine, Lyrics, LyricsClient};
pub use player::{AudioBytes, AudioPlayer};
pub use queue::{Queue, RepeatMode};
