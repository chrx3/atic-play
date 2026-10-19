//! Proof of concept: `cargo run -p ytm-core --example play -- "song name"`

use std::time::Duration;

use ytm_core::{AudioPlayer, Client};

fn main() -> anyhow::Result<()> {
    let query = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let query = if query.is_empty() {
        "daft punk one more time".to_string()
    } else {
        query
    };

    let client = Client::new(std::env::temp_dir().join("ytm-core-examples"))?;
    let tracks = block_on(client.search_tracks(query))?;
    for (index, track) in tracks.iter().take(5).enumerate() {
        println!("{index}. {} — {}", track.title, track.artists);
    }
    let track = tracks
        .first()
        .ok_or_else(|| anyhow::anyhow!("no results"))?;

    println!("Downloading {}…", track.title);
    let audio = block_on(client.fetch_audio(track.id.clone()))?;
    println!("{} KB, playing for 20 s", audio.len() / 1024);

    let player = AudioPlayer::new()?;
    player.play(audio)?;
    std::thread::sleep(Duration::from_secs(20));
    Ok(())
}

/// The client futures run on their own runtime; a throwaway one is enough to wait here.
fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(future)
}
