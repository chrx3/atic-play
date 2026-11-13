//! A seekable source that downloads the file in byte ranges.
//!
//! Symphonia wants a `MediaSource`: something it can read and seek. We give it one that fetches
//! 512 KiB at a time with `Range: bytes=a-b`, keeps a prefetch thread running ahead of the
//! reader, and can jump to any offset without downloading what is in between.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use symphonia::core::io::MediaSource;

/// How many bytes we ask for in each range request.
pub const CHUNK: u64 = 512 * 1024;

/// How much we like to have downloaded ahead of the reader.
const PREFETCH_TARGET: usize = 1024 * 1024;

/// How many times we try again when the network hiccups.
const RETRIES: u32 = 3;

/// How long a request may take before we give up on it.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the reader waits for data before the download is declared stuck.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(15);

/// The `Range` header we send for `[start, end]`.
pub fn range_header(start: u64, end: u64) -> String {
    format!("bytes={start}-{end}")
}

/// The last byte of the range we ask for, clamped to the size of the file when we know it.
pub fn range_end(start: u64, total: Option<u64>) -> u64 {
    match total {
        Some(total) if total > 0 => start.saturating_add(CHUNK - 1).min(total - 1),
        _ => start.saturating_add(CHUNK - 1),
    }
}

/// Pulls the total size out of a `Content-Range` header (`bytes 0-1023/4096`).
pub fn total_from_content_range(value: &str) -> Option<u64> {
    let (_, total) = value.split_once('/')?;
    let total = total.trim();
    if total == "*" {
        return None;
    }
    total.parse().ok()
}

/// Pulls the total size out of a `Content-Length` header.
pub fn total_from_content_length(value: &str) -> Option<u64> {
    value.trim().parse().ok()
}

/// Why a download did not work out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    /// The server said no. Trying again will not help.
    Fatal(String),
    /// The network stumbled. Trying again might help.
    Transient(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Fatal(msg) | FetchError::Transient(msg) => f.write_str(msg),
        }
    }
}

/// The state of the download.
#[derive(Debug)]
struct State {
    /// Absolute offset of `buf[0]`.
    start: u64,
    /// The bytes we have downloaded.
    buf: Vec<u8>,
    /// Where the reader is inside `buf`.
    pos: usize,
    /// The offset of the next range request.
    next: u64,
    /// Set when the server refused or the network died.
    error: Option<String>,
    /// Set when there is nothing left to download.
    eof: bool,
    /// Bumped on every seek; downloads from an older epoch are dropped.
    epoch: u64,
}

/// A callback that is called when new data arrives.
struct OnData(Box<dyn Fn() + Send + Sync>);

impl OnData {
    fn call(&self) {
        (self.0)();
    }
}

impl std::fmt::Debug for OnData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OnData")
    }
}

#[derive(Debug)]
struct Inner {
    url: String,
    headers: Vec<(String, String)>,
    agent: ureq::Agent,
    state: Mutex<State>,
    /// Wakes the reader and the prefetcher up.
    signal: Condvar,
    /// The size of the file, 0 when we do not know it yet.
    total: AtomicU64,
    /// Set when nobody wants the data anymore.
    cancelled: AtomicBool,
    /// Called when new data arrives, so the engine knows the load is moving forward.
    on_data: OnData,
}

/// A file we read over HTTP, in ranges.
#[derive(Debug)]
pub struct HttpSource {
    inner: Arc<Inner>,
}

impl HttpSource {
    /// Opens `url` for reading.
    ///
    /// When the size is not known yet, the first range is fetched right away: the MP4 demuxer
    /// needs the total length to allow seeking.
    pub fn new(
        url: &str,
        headers: &[(String, String)],
        content_length: Option<u64>,
        on_data: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, FetchError> {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent("atic-play/0.1")
            .build()
            .new_agent();

        let inner = Arc::new(Inner {
            url: url.to_string(),
            headers: headers.to_vec(),
            agent,
            state: Mutex::new(State {
                start: 0,
                buf: Vec::new(),
                pos: 0,
                next: 0,
                error: None,
                eof: false,
                epoch: 0,
            }),
            signal: Condvar::new(),
            total: AtomicU64::new(content_length.unwrap_or(0)),
            cancelled: AtomicBool::new(false),
            on_data: OnData(Box::new(on_data)),
        });

        if content_length.is_none() {
            // Learn how big the file is before anybody asks.
            let end = range_end(0, None);
            let bytes = fetch_with_retry(&inner, 0, end)?;
            let mut state = inner.state.lock().unwrap_or_else(|err| err.into_inner());
            state.buf = bytes;
            state.next = state.buf.len() as u64;
            if state.next == 0 {
                state.eof = true;
            }
            drop(state);
            inner.on_data.call();
        }

        let weak = Arc::downgrade(&inner);
        thread::Builder::new()
            .name("atic-audio-fetch".into())
            .spawn(move || prefetch_loop(weak))
            .map_err(|err| FetchError::Fatal(format!("no se pudo iniciar la descarga: {err}")))?;

        Ok(HttpSource { inner })
    }

    /// The size of the file, when we know it.
    pub fn total_len(&self) -> Option<u64> {
        match self.inner.total.load(Ordering::Relaxed) {
            0 => None,
            total => Some(total),
        }
    }

    /// A handle that can stop the download from anywhere.
    pub fn cancel_handle(&self) -> CancelHandle {
        CancelHandle { inner: Arc::clone(&self.inner) }
    }
}

/// Stops a download that is still running.
#[derive(Clone, Debug)]
pub struct CancelHandle {
    inner: Arc<Inner>,
}

impl CancelHandle {
    /// Asks the prefetcher to stop.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Relaxed);
        self.inner.signal.notify_all();
    }
}

/// Downloads `start..=end`, retrying a couple of times when the network stumbles.
fn fetch_with_retry(inner: &Inner, start: u64, end: u64) -> Result<Vec<u8>, FetchError> {
    let mut last = None;
    for attempt in 0..RETRIES {
        match fetch(inner, start, end) {
            Ok(bytes) => return Ok(bytes),
            Err(FetchError::Fatal(msg)) => return Err(FetchError::Fatal(msg)),
            Err(FetchError::Transient(msg)) => {
                last = Some(msg);
                if attempt + 1 < RETRIES {
                    thread::sleep(Duration::from_millis(250 * (attempt as u64 + 1)));
                }
            }
        }
    }
    Err(FetchError::Transient(last.unwrap_or_else(|| "la descarga falló".into())))
}

/// One range request.
fn fetch(inner: &Inner, start: u64, end: u64) -> Result<Vec<u8>, FetchError> {
    let mut request = inner.agent.get(&inner.url).header("Range", range_header(start, end));
    for (name, value) in &inner.headers {
        request = request.header(name.as_str(), value.as_str());
    }

    let mut response = request.call().map_err(|err| match err {
        ureq::Error::StatusCode(code) => FetchError::Fatal(format!("HTTP {code}")),
        other => FetchError::Transient(other.to_string()),
    })?;

    let status = response.status().as_u16();
    let content_range = response
        .headers()
        .get("content-range")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    match status {
        200 | 206 => {}
        403 => {
            return Err(FetchError::Fatal(format!(
                "el servidor no autorizó la descarga (403) del rango {start}-{end}"
            )));
        }
        416 => return Ok(Vec::new()),
        code => return Err(FetchError::Fatal(format!("HTTP {code}"))),
    }

    if status == 200 && start != 0 {
        return Err(FetchError::Fatal("el servidor no respeta los rangos pedidos".into()));
    }

    // Learn the size of the file if we can.
    let total = content_range
        .as_deref()
        .and_then(total_from_content_range)
        .or_else(|| {
            if status == 200 { content_length.as_deref().and_then(total_from_content_length) } else { None }
        });
    if let Some(total) = total {
        inner.total.store(total, Ordering::Relaxed);
    }

    let wanted = end.saturating_sub(start).saturating_add(1);
    let mut buf = Vec::with_capacity(wanted.min(1024 * 1024) as usize);
    let reader = response.body_mut().as_reader();
    reader
        .take(wanted)
        .read_to_end(&mut buf)
        .map_err(|err| FetchError::Transient(err.to_string()))?;

    Ok(buf)
}

/// Keeps the download ahead of the reader.
///
/// The thread lives as long as the source does: when the file ends it waits for the reader to
/// jump somewhere else instead of quitting, otherwise the next seek would wait forever for data
/// that nobody is going to download.
fn prefetch_loop(weak: Weak<Inner>) {
    loop {
        let Some(inner) = weak.upgrade() else { return };
        if inner.cancelled.load(Ordering::Relaxed) {
            return;
        }

        let (start, end, epoch) = {
            let mut state = inner.state.lock().unwrap_or_else(|err| err.into_inner());
            loop {
                if inner.cancelled.load(Ordering::Relaxed) {
                    return;
                }
                if state.error.is_some() {
                    return;
                }
                let buffered = state.buf.len().saturating_sub(state.pos);
                // At the end of the file there is nothing to download: wait for a seek.
                if !state.eof && buffered < PREFETCH_TARGET {
                    break;
                }
                let (guard, _) = inner
                    .signal
                    .wait_timeout(state, Duration::from_millis(100))
                    .unwrap_or_else(|err| err.into_inner());
                state = guard;
            }
            let total = match inner.total.load(Ordering::Relaxed) {
                0 => None,
                total => Some(total),
            };
            (state.next, range_end(state.next, total), state.epoch)
        };

        let bytes = match fetch_with_retry(&inner, start, end) {
            Ok(bytes) => bytes,
            Err(err) => {
                if let Some(inner) = weak.upgrade() {
                    let mut state = inner.state.lock().unwrap_or_else(|err| err.into_inner());
                    if state.epoch == epoch && state.error.is_none() {
                        state.error = Some(err.to_string());
                    }
                    inner.signal.notify_all();
                }
                return;
            }
        };

        let Some(inner) = weak.upgrade() else { return };
        let mut state = inner.state.lock().unwrap_or_else(|err| err.into_inner());
        if state.epoch != epoch {
            // The reader jumped somewhere else while we were downloading: throw this away.
            continue;
        }
        let received = bytes.len() as u64;
        state.buf.extend_from_slice(&bytes);
        state.next = start + received;
        let total = inner.total.load(Ordering::Relaxed);
        if received == 0 || (total > 0 && state.next >= total) || received < end - start + 1 {
            state.eof = true;
        }
        drop(state);
        inner.on_data.call();
        inner.signal.notify_all();
    }
}

impl Read for HttpSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut state = self.inner.state.lock().unwrap_or_else(|err| err.into_inner());
        let waiting_since = Instant::now();
        loop {
            if state.pos < state.buf.len() {
                let count = (state.buf.len() - state.pos).min(buf.len());
                buf[..count].copy_from_slice(&state.buf[state.pos..state.pos + count]);
                state.pos += count;
                self.inner.signal.notify_all();
                return Ok(count);
            }
            if let Some(error) = state.error.clone() {
                return Err(io::Error::other(error));
            }
            if state.eof {
                return Ok(0);
            }
            if self.inner.cancelled.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "descarga cancelada"));
            }
            if waiting_since.elapsed() > STALL_TIMEOUT {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "la descarga se estancó: el servidor no mandó más datos",
                ));
            }
            let (guard, _) = self
                .inner
                .signal
                .wait_timeout(state, Duration::from_millis(100))
                .unwrap_or_else(|err| err.into_inner());
            state = guard;
        }
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let mut state = self.inner.state.lock().unwrap_or_else(|err| err.into_inner());
        let target = match pos {
            SeekFrom::Start(offset) => offset,
            SeekFrom::Current(delta) => {
                let current = state.start + state.pos as u64;
                if delta < 0 {
                    current.saturating_sub(delta.unsigned_abs())
                } else {
                    current.saturating_add(delta as u64)
                }
            }
            SeekFrom::End(delta) => {
                let total = self.inner.total.load(Ordering::Relaxed);
                if total == 0 {
                    return Err(io::Error::new(io::ErrorKind::Unsupported, "tamaño desconocido"));
                }
                let total = total as i64;
                (total + delta).max(0) as u64
            }
        };

        // The bytes may already be here: moving the cursor is much cheaper than asking the
        // server for them again.
        let buffered_end = state.start + state.buf.len() as u64;
        if target >= state.start && target < buffered_end {
            state.pos = (target - state.start) as usize;
            self.inner.signal.notify_all();
            return Ok(target);
        }

        state.epoch = state.epoch.wrapping_add(1);
        state.buf.clear();
        state.pos = 0;
        state.start = target;
        state.next = target;
        state.eof = false;
        self.inner.signal.notify_all();
        Ok(target)
    }
}

impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.total_len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_range_header_is_the_one_the_server_expects() {
        assert_eq!(range_header(0, 524_287), "bytes=0-524287");
        assert_eq!(range_header(1_000_000, 1_524_287), "bytes=1000000-1524287");
    }

    #[test]
    fn ranges_are_512_kib_big() {
        assert_eq!(range_end(0, None), 512 * 1024 - 1);
        assert_eq!(range_end(4096, None), 4096 + 512 * 1024 - 1);
    }

    #[test]
    fn the_last_range_stops_at_the_end_of_the_file() {
        assert_eq!(range_end(0, Some(1000)), 999);
        assert_eq!(range_end(900, Some(1000)), 999);
        assert_eq!(range_end(1000, Some(1000)), 999);
    }

    #[test]
    fn the_size_comes_from_the_content_range() {
        assert_eq!(total_from_content_range("bytes 0-524287/1234567"), Some(1_234_567));
        assert_eq!(total_from_content_range("bytes 100-199/200"), Some(200));
        assert_eq!(total_from_content_range("bytes 0-1/*"), None);
        assert_eq!(total_from_content_range("nonsense"), None);
    }

    #[test]
    fn the_size_comes_from_the_content_length() {
        assert_eq!(total_from_content_length("4096"), Some(4096));
        assert_eq!(total_from_content_length("nope"), None);
    }
}

#[cfg(test)]
mod network_tests {
    use super::*;
    use crate::testing;
    use std::io::Write;

    /// Reads a file over HTTP using range requests.
    ///
    /// It needs a socket, so it is ignored by default. Run it by hand with
    /// `cargo test -p play-audio -- --ignored`.
    #[test]
    #[ignore]
    fn reads_ranges_over_http() {
        // A file of 1000 bytes, served with range support.
        let body: Vec<u8> = (0..1000u32).map(|value| (value % 251) as u8).collect();
        let server = testing::serve(body.clone());

        let mut source =
            HttpSource::new(&server.url, &[], None, || {}).expect("the download starts");
        assert_eq!(source.total_len(), Some(1000));

        // Reading from the start gives the whole file.
        let mut all = Vec::new();
        source.read_to_end(&mut all).expect("the file arrives");
        assert_eq!(all, body);

        // Seeking jumps to another range without downloading what is in between.
        let mut source =
            HttpSource::new(&server.url, &[], Some(1000), || {}).expect("the download starts");
        source.seek(SeekFrom::Start(500)).expect("the seek works");
        let mut tail = Vec::new();
        source.read_to_end(&mut tail).expect("the tail arrives");
        assert_eq!(tail, body[500..]);
    }

    /// The download may not die when the file ends: a seek back has to work.
    ///
    /// This is what kept the engine in `Loading` forever: the download stopped at the end of the
    /// file and nobody was left to fetch the bytes the demuxer asked for after the jump.
    #[test]
    #[ignore]
    fn the_download_survives_a_jump_back_to_the_beginning() {
        let body: Vec<u8> = (0..1000u32).map(|value| (value % 251) as u8).collect();
        let server = testing::serve(body.clone());

        let mut source =
            HttpSource::new(&server.url, &[], Some(1000), || {}).expect("the download starts");

        // Read the whole file: the download reaches the end.
        let mut all = Vec::new();
        source.read_to_end(&mut all).expect("the file arrives");
        assert_eq!(all, body);

        // Jump back to the beginning: the bytes have to arrive again.
        source.seek(SeekFrom::Start(0)).expect("the seek works");
        let mut again = Vec::new();
        source.read_to_end(&mut again).expect("the file arrives again");
        assert_eq!(again, body);
    }

    /// The bytes that are already downloaded are not asked for again.
    #[test]
    #[ignore]
    fn a_jump_inside_the_buffer_does_not_download_again() {
        let body: Vec<u8> = (0..1000u32).map(|value| (value % 251) as u8).collect();
        let server = testing::serve(body.clone());

        let mut source =
            HttpSource::new(&server.url, &[], Some(1000), || {}).expect("the download starts");

        // Read the first bytes: the download goes on in the background.
        let mut head = Vec::new();
        source.read_to_end(&mut head).expect("the file arrives");
        assert_eq!(head, body);

        let before = server.requests();

        // Jump inside the downloaded data: no new request is needed.
        source.seek(SeekFrom::Start(10)).expect("the seek works");
        let mut middle = vec![0u8; 100];
        source.read_exact(&mut middle).expect("the bytes arrive");
        assert_eq!(middle, body[10..110]);
        assert_eq!(server.requests(), before, "no hace falta descargar otra vez");
    }

    /// A reader that waits too long gives up with an error instead of hanging.
    #[test]
    #[ignore]
    fn a_stalled_download_gives_up() {
        // A server that promises a thousand bytes, sends ten and then goes quiet.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("the socket opens");
        let address = listener.local_addr().expect("the socket has an address");
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let head = "HTTP/1.1 206 Partial Content\r\n\
                            Content-Range: bytes 0-1023/1024\r\n\
                            Content-Length: 1024\r\n\
                            Connection: close\r\n\r\n";
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&[0u8; 10]);
                let _ = stream.flush();
                // And then nothing: the reader waits for the rest of the file.
                thread::sleep(Duration::from_secs(60));
            }
        });

        let url = format!("http://{address}/audio.m4a");
        let mut source = HttpSource::new(&url, &[], Some(1024), || {}).expect("the download starts");

        let mut buf = [0u8; 16];
        let started = Instant::now();
        let result = loop {
            match source.read(&mut buf) {
                Ok(0) => break Ok(0),
                Ok(_) => continue,
                Err(err) => break Err(err),
            }
        };
        let elapsed = started.elapsed();

        let err = result.expect_err("la descarga should've fallen");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(elapsed >= STALL_TIMEOUT, "falló demasiado pronto: {elapsed:?}");
    }
}

