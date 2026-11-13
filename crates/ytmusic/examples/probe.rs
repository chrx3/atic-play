//! Manual probe of the client.
//!
//! ```text
//! cargo run -p play-ytmusic --example probe -- "daft punk"
//! ```

use std::error::Error;

use play_ytmusic::{ArtistRef, Client, Item, Song};

fn main() -> Result<(), Box<dyn Error>> {
    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "daft punk".to_string());

    let client = Client::new("es", "CL")?;
    println!("query: {query}\n");

    match client.search_suggestions(&query) {
        Ok(suggestions) => println!("suggestions: {suggestions:?}\n"),
        Err(error) => println!("suggestions failed: {error}\n"),
    }

    let results = client.search(&query)?;
    println!(
        "songs: {}, albums: {}, artists: {}, playlists: {}\n",
        results.songs.len(),
        results.albums.len(),
        results.artists.len(),
        results.playlists.len()
    );

    for (index, song) in results.songs.iter().take(10).enumerate() {
        println!(
            "{:>2}. {} — {} [{}]",
            index + 1,
            song.title,
            artists(&song.artists),
            length(song)
        );
    }
    for album in results.albums.iter().take(5) {
        println!(
            "    album: {} — {} [{}]",
            album.title,
            artists(&album.artists),
            album.browse_id
        );
    }
    for artist in results.artists.iter().take(5) {
        println!("    artist: {} [{}]", artist.name, artist.browse_id);
    }
    for playlist in results.playlists.iter().take(5) {
        println!("    playlist: {} [{}]", playlist.title, playlist.id);
    }

    if let Some(album) = results.albums.first() {
        match client.album(&album.browse_id) {
            Ok(page) => println!(
                "\nalbum page: {} ({:?}) with {} tracks",
                page.album.title,
                page.album.year,
                page.tracks.len()
            ),
            Err(error) => println!("\nalbum page failed: {error}"),
        }
    }

    if let Some(song) = results.songs.first() {
        match client.up_next(&song.video_id) {
            Ok(queue) => println!("up next: {} songs", queue.len()),
            Err(error) => println!("up next failed: {error}"),
        }

        println!("\nstream of {:?} ({}):", song.title, song.video_id);
        match client.stream(&song.video_id) {
            Ok(stream) => {
                println!("  url: {}", stream.url);
                println!("  mime: {}", stream.mime_type);
                println!("  bitrate: {} kbps", stream.bitrate / 1000);
                println!("  content-length: {:?}", stream.content_length);
                println!("  duration: {:?}", stream.duration);
                println!("  loudness: {:?}", stream.loudness_db);
                println!("  headers: {:?}", stream.headers);
            }
            Err(error) => println!("  failed: {error}"),
        }
    }

    let shelves = client.home()?;
    println!("\nhome:");
    for shelf in shelves.iter().take(5) {
        let kinds: Vec<&str> = shelf.items.iter().map(kind).collect();
        println!("  {} ({})", shelf.title, kinds.join(", "));
    }

    Ok(())
}

fn artists(artists: &[ArtistRef]) -> String {
    if artists.is_empty() {
        "(unknown artist)".to_string()
    } else {
        artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn length(song: &Song) -> String {
    match song.duration {
        Some(duration) => format!("{}:{:02}", duration.as_secs() / 60, duration.as_secs() % 60),
        None => "--:--".to_string(),
    }
}

fn kind(item: &Item) -> &'static str {
    match item {
        Item::Song(_) => "song",
        Item::Album(_) => "album",
        Item::Artist(_) => "artist",
        Item::Playlist(_) => "playlist",
    }
}
