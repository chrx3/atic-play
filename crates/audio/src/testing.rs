//! Helpers for the tests: a tiny HTTP server that honours range requests, and the fixtures.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

/// The bytes of a file in `tests/fixtures`.
pub fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|err| panic!("no se pudo leer {path}: {err}"))
}

/// A file served over HTTP.
pub struct Server {
    /// The URL to play.
    pub url: String,
    /// How many requests the server answered.
    requests: Arc<AtomicUsize>,
    _handle: JoinHandle<()>,
}

impl Server {
    /// How many requests the server answered so far.
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }
}

/// Serves `bytes` over HTTP with support for range requests.
pub fn serve(bytes: Vec<u8>) -> Server {
    let bytes = Arc::new(bytes);
    let requests = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").expect("the socket opens");
    let address = listener.local_addr().expect("the socket has an address");

    let handle = {
        let requests = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let bytes = Arc::clone(&bytes);
                let requests = Arc::clone(&requests);
                std::thread::spawn(move || {
                    requests.fetch_add(1, Ordering::Relaxed);
                    serve_one(stream, &bytes);
                });
            }
        })
    };

    Server { url: format!("http://{address}/audio.m4a"), requests, _handle: handle }
}

/// Answers one request, honouring `Range: bytes=a-b`.
fn serve_one(mut stream: std::net::TcpStream, bytes: &[u8]) {
    let mut buffer = [0u8; 4096];
    let Ok(read) = stream.read(&mut buffer) else { return };
    let request = String::from_utf8_lossy(&buffer[..read]).to_string();

    let mut start = 0usize;
    let mut end = bytes.len().saturating_sub(1);
    for line in request.lines() {
        // The header names are case insensitive, and the client sends them in lower case.
        let Some((name, value)) = line.split_once(':') else { continue };
        if !name.eq_ignore_ascii_case("range") {
            continue;
        }
        let value = value.trim();
        let Some(value) = value.strip_prefix("bytes=") else { continue };
        let (from, to) = value.split_once('-').unwrap_or((value, ""));
        if let Ok(from) = from.trim().parse() {
            start = from;
        }
        if let Ok(to) = to.trim().parse::<usize>() {
            end = to.min(bytes.len().saturating_sub(1));
        }
    }
    if start >= bytes.len() {
        let head = "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        return;
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
