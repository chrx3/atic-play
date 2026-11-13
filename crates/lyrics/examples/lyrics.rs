//! Prints the lyrics of a song, with the time each line starts.
//!
//! ```text
//! cargo run -p play-lyrics --example lyrics -- "título" "artista" [segundos]
//! ```

use std::time::Duration;

use play_lyrics::{Lyrics, Query};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let title = args.next().unwrap_or_default();
    let artist = args.next().unwrap_or_default();
    let duration = args.next().and_then(|seconds| seconds.parse::<f64>().ok());

    if title.is_empty() || artist.is_empty() {
        eprintln!("uso: lyrics <título> <artista> [segundos]");
        return Ok(());
    }

    let query = Query {
        title: &title,
        artist: &artist,
        album: None,
        duration: duration.map(Duration::from_secs_f64),
    };

    match play_lyrics::fetch(&query)? {
        Some(Lyrics::Synced(lines)) => {
            println!("{} líneas sincronizadas", lines.len());
            for line in lines {
                println!("[{}] {}", stamp(line.start), line.text);
            }
        }
        Some(Lyrics::Plain(text)) => {
            println!("letra simple:");
            for line in text.lines() {
                println!("{line}");
            }
        }
        Some(Lyrics::Instrumental) => println!("instrumental"),
        None => println!("no hay letra en LRCLIB"),
    }

    Ok(())
}

/// `mm:ss.mmm`, the way LRC writes it.
fn stamp(time: Duration) -> String {
    let total = time.as_millis();
    let minutes = total / 60_000;
    let seconds = (total % 60_000) / 1_000;
    let millis = total % 1_000;
    format!("{minutes:02}:{seconds:02}.{millis:03}")
}
