//! Plays a song and shows what the engine is doing.
//!
//! ```text
//! cargo run -p play-audio --example play -- https://example.test/song.m4a
//! cargo run -p play-audio --example play -- ./song.m4a
//! ```
//!
//! A local file is served over HTTP by a tiny server in this example, because the engine only
//! knows how to read files in byte ranges over the network.
//!
//! The sound is muted: this is a diagnostic, the state and the position are what it shows. Set
//! `PLAY_VOLUME` (say `0.8`) to listen anyway.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use play_audio::{Event, Player, Source};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(what) = args.next() else {
        eprintln!("uso: play <url o archivo .m4a>");
        std::process::exit(2);
    };

    // A local file needs a server: the engine reads in ranges over HTTP.
    let (url, _server) = if what.starts_with("http://") || what.starts_with("https://") {
        (what, None)
    } else {
        let (url, server) = serve_file(Path::new(&what))?;
        (url, Some(server))
    };
    println!("reproduciendo {url}");

    let player = Player::new()?;
    // Muted by default: the example shows the state and the position, not the music.
    let volume = std::env::var("PLAY_VOLUME")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0);
    player.set_volume(volume);
    player.load(Source {
        url: url.clone(),
        headers: Vec::new(),
        content_length: None,
        gain_db: 0.0,
    });

    // Ten seconds of music.
    wait(&player, Duration::from_secs(10));

    // Jump to the middle of the song.
    println!("\n-- saltando a 30 s --");
    player.seek(Duration::from_secs(30));
    wait(&player, Duration::from_secs(2));

    // Pause for two seconds: the position must not move.
    println!("\n-- pausando --");
    player.pause();
    wait(&player, Duration::from_secs(2));

    // And on.
    println!("\n-- continuando --");
    player.play();
    wait(&player, Duration::from_secs(3));

    println!("\n-- terminando --");
    player.stop();
    Ok(())
}

/// Waits, printing the position and whatever the engine reports.
fn wait(player: &Player, how_long: Duration) {
    let until = Instant::now() + how_long;
    let mut last = Duration::ZERO;
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(200));
        let position = player.position();
        if position.abs_diff(last) >= Duration::from_millis(200) {
            println!("  {:>6.1?}  {:?}", position, player.state());
            last = position;
        }
        for event in player.poll_events() {
            match event {
                Event::State(state) => println!("  estado: {state:?}"),
                Event::Duration(duration) => println!("  duración: {duration:?}"),
                Event::Error(error) => println!("  error: {error}"),
            }
        }
    }
}

/// Serves `path` over HTTP with support for range requests. Returns the URL and the thread.
fn serve_file(path: &Path) -> anyhow::Result<(String, thread::JoinHandle<()>)> {
    let bytes = std::fs::read(path)?;
    let bytes = Arc::new(bytes);
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;

    let handle = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let bytes = Arc::clone(&bytes);
            thread::spawn(move || serve_one(stream, &bytes));
        }
    });

    Ok((format!("http://{address}/audio.m4a"), handle))
}

/// Answers one request, honouring `Range: bytes=a-b`.
fn serve_one(mut stream: std::net::TcpStream, bytes: &[u8]) {
    let mut buffer = [0u8; 4096];
    let Ok(read) = stream.read(&mut buffer) else { return };
    let request = String::from_utf8_lossy(&buffer[..read]).to_string();

    let mut start = 0usize;
    let mut end = bytes.len().saturating_sub(1);
    for line in request.lines() {
        let Some((name, value)) = line.split_once(':') else { continue };
        if !name.eq_ignore_ascii_case("range") {
            continue;
        }
        let value = value.trim();
        let Some(value) = value.strip_prefix("bytes=") else { continue };
        let (from, to) = value.trim().split_once('-').unwrap_or((value, ""));
        if let Ok(from) = from.trim().parse::<usize>() {
            start = from;
        }
        if let Ok(to) = to.trim().parse::<usize>() {
            end = to.min(bytes.len().saturating_sub(1));
        }
    }
    if start > end {
        start = 0;
    }
    let length = end - start + 1;

    let head = format!(
        "HTTP/1.1 206 Partial Content\r\n\
         Content-Type: audio/mp4\r\n\
         Accept-Ranges: bytes\r\n\
         Content-Range: bytes {start}-{end}/{}\r\n\
         Content-Length: {length}\r\n\
         Connection: close\r\n\r\n",
        bytes.len()
    );
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    let _ = stream.write_all(&bytes[start..=end]);
    let _ = stream.flush();
}
