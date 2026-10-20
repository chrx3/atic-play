//! Checks search suggestions: `cargo run -p ytm-core --example suggest -- "bad bun"`

use ytm_core::Client;

fn main() -> anyhow::Result<()> {
    let query = std::env::args().nth(1).unwrap_or_else(|| "bad bun".into());
    let client = Client::new(std::env::temp_dir().join("ytm-core-examples"))?;
    let suggestions = tokio::runtime::Builder::new_current_thread()
        .build()?
        .block_on(client.search_suggestions(query))?;
    println!("terms: {:?}", suggestions.terms);
    for track in &suggestions.tracks {
        println!("track: {} — {}", track.title, track.artists);
    }
    for album in &suggestions.albums {
        println!("album: {} — {}", album.title, album.artists);
    }
    for artist in &suggestions.artists {
        println!("artist: {}", artist.name);
    }
    Ok(())
}
