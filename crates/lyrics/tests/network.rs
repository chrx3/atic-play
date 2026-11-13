//! Tests that talk to the real LRCLIB. They need the network, so they are ignored by default:
//!
//! ```text
//! cargo test -p play-lyrics --test network -- --ignored --nocapture
//! ```

use std::time::Duration;

use play_lyrics::{Lyrics, Query};

#[test]
#[ignore = "needs the network"]
fn fetches_a_known_song() {
    let query = Query {
        title: "I Want to Live",
        artist: "Borislav Slavov",
        album: Some("Baldur's Gate 3 (Original Game Soundtrack)"),
        duration: Some(Duration::from_secs(233)),
    };

    let lyrics = play_lyrics::fetch(&query).expect("the request should work");
    match lyrics {
        Some(Lyrics::Synced(lines)) => {
            assert!(!lines.is_empty(), "the song has synced lyrics");
            assert!(
                lines.windows(2).all(|pair| pair[0].start <= pair[1].start),
                "the lines must be sorted"
            );
            println!("{} synced lines", lines.len());
            for line in lines.iter().take(3) {
                println!("[{:?}] {}", line.start, line.text);
            }
        }
        Some(Lyrics::Plain(text)) => println!("plain lyrics:\n{text}"),
        Some(Lyrics::Instrumental) => panic!("this song is not instrumental"),
        None => panic!("LRCLIB should know this song"),
    }
}

#[test]
#[ignore = "needs the network"]
fn finds_a_song_that_needs_the_search() {
    let query = Query {
        title: "Bohemian Rhapsody - Remastered 2011",
        artist: "Queen",
        album: None,
        duration: Some(Duration::from_secs(355)),
    };

    let lyrics = play_lyrics::fetch(&query).expect("the request should work");
    assert!(lyrics.is_some(), "LRCLIB should know this song");
}
