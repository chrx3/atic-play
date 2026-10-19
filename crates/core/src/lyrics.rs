//! Song lyrics from LRCLIB (https://lrclib.net), synced line by line when available.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use serde_json::Value;
use tokio::runtime::Runtime;

use crate::Track;

const API: &str = "https://lrclib.net/api";
/// LRCLIB asks clients to identify themselves.
const USER_AGENT: &str = concat!("Atic-play/", env!("CARGO_PKG_VERSION"));
/// How far a search result's length may be from the track's to count as the same recording.
const DURATION_TOLERANCE_SECS: f64 = 3.;

/// One line of synced lyrics and when it starts.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricLine {
    pub start: Duration,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Lyrics {
    /// Lines with their start times, in order.
    Synced(Vec<LyricLine>),
    /// Lines without timing.
    Plain(Vec<String>),
    Instrumental,
    NotFound,
}

impl Lyrics {
    /// The synced line playing at `position`: the last one that has started.
    pub fn active_line(&self, position: Duration) -> Option<usize> {
        let Lyrics::Synced(lines) = self else {
            return None;
        };
        lines
            .partition_point(|line| line.start <= position)
            .checked_sub(1)
    }
}

/// Fetches lyrics. Requests run on their own small runtime, so the futures can be awaited
/// from any executor.
#[derive(Clone)]
pub struct LyricsClient {
    runtime: Arc<Runtime>,
    http: reqwest::Client,
}

impl LyricsClient {
    pub fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("lyrics")
            .enable_all()
            .build()?;
        let http = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
        Ok(LyricsClient {
            runtime: Arc::new(runtime),
            http,
        })
    }

    /// The lyrics of `track`: an exact match by artist, title and length first, then the
    /// closest search result.
    pub fn fetch(&self, track: &Track) -> impl Future<Output = Result<Lyrics>> + use<> {
        let http = self.http.clone();
        let title = track.title.clone();
        let artists = track.artists.clone();
        let album = track.album.clone();
        let duration = track.duration_secs;
        let handle = self.runtime.spawn(async move {
            let mut query = vec![
                ("track_name", title.clone()),
                ("artist_name", artists.clone()),
            ];
            if let Some(album) = album {
                query.push(("album_name", album));
            }
            if let Some(duration) = duration {
                query.push(("duration", duration.to_string()));
            }
            if let Some(record) = get_json(&http, "get", &query).await? {
                let lyrics = lyrics_from(&record);
                if lyrics != Lyrics::NotFound {
                    return Ok(lyrics);
                }
            }

            // The search is fuzzy, but a list of artists ("A, B & C") still throws it off.
            let artist = main_artist(&artists).to_string();
            let query = [("track_name", title), ("artist_name", artist)];
            let Some(Value::Array(results)) = get_json(&http, "search", &query).await? else {
                return Ok(Lyrics::NotFound);
            };
            Ok(best_match(&results, duration.map(f64::from)))
        });
        async move { handle.await.context("lyrics task panicked")? }
    }
}

/// `None` when LRCLIB has no such record (404).
async fn get_json(
    http: &reqwest::Client,
    endpoint: &str,
    query: &[(&str, String)],
) -> Result<Option<Value>> {
    let response = http
        .get(format!("{API}/{endpoint}"))
        .query(query)
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let body = response.error_for_status()?.text().await?;
    Ok(Some(serde_json::from_str(&body)?))
}

/// The search result closest in length, preferring synced lyrics.
fn best_match(results: &[Value], duration: Option<f64>) -> Lyrics {
    let close = |record: &&Value| match (duration, record["duration"].as_f64()) {
        (Some(wanted), Some(found)) => (wanted - found).abs() <= DURATION_TOLERANCE_SECS,
        _ => true,
    };
    let candidates: Vec<_> = results.iter().filter(close).collect();
    let synced = candidates.iter().find(|record| {
        record["syncedLyrics"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    });
    match synced.or(candidates.first()) {
        Some(record) => lyrics_from(record),
        None => Lyrics::NotFound,
    }
}

fn lyrics_from(record: &Value) -> Lyrics {
    if record["instrumental"].as_bool() == Some(true) {
        return Lyrics::Instrumental;
    }
    if let Some(synced) = record["syncedLyrics"].as_str() {
        let lines = parse_lrc(synced);
        if !lines.is_empty() {
            return Lyrics::Synced(lines);
        }
    }
    if let Some(plain) = record["plainLyrics"].as_str() {
        let lines: Vec<String> = plain.lines().map(|line| line.trim().to_string()).collect();
        if lines.iter().any(|line| !line.is_empty()) {
            return Lyrics::Plain(lines);
        }
    }
    Lyrics::NotFound
}

/// Parses LRC text: `[mm:ss.xx]line`, possibly with several timestamps per line. Metadata
/// tags (`[ar:...]`) and untimed lines are skipped.
pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut starts = Vec::new();
        while let Some(tag) = rest.strip_prefix('[') {
            let Some(end) = tag.find(']') else {
                break;
            };
            match parse_timestamp(&tag[..end]) {
                Some(start) => starts.push(start),
                None => break,
            }
            rest = tag[end + 1..].trim_start();
        }
        let text = rest.trim().to_string();
        lines.extend(starts.into_iter().map(|start| LyricLine {
            start,
            text: text.clone(),
        }));
    }
    lines.sort_by_key(|line| line.start);
    lines
}

/// `mm:ss`, `mm:ss.xx` or `mm:ss.xxx`.
fn parse_timestamp(tag: &str) -> Option<Duration> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes: u64 = minutes.trim().parse().ok()?;
    let seconds: f64 = seconds.trim().parse().ok()?;
    if !(0. ..60.).contains(&seconds) {
        return None;
    }
    Some(Duration::from_secs(minutes * 60) + Duration::from_secs_f64(seconds))
}

/// The first of the joined artist names.
fn main_artist(artists: &str) -> &str {
    [", ", " & ", " y ", " and ", " feat. ", " ft. "]
        .iter()
        .filter_map(|separator| artists.find(separator))
        .min()
        .map_or(artists, |end| &artists[..end])
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(ms: u64, text: &str) -> LyricLine {
        LyricLine {
            start: Duration::from_millis(ms),
            text: text.to_string(),
        }
    }

    #[test]
    fn parses_timed_lines_and_skips_metadata() {
        let lrc = "[ar:Someone]\n[00:01.50] first line\n[00:03.25]second line\n\nno time\n";
        assert_eq!(
            parse_lrc(lrc),
            vec![line(1500, "first line"), line(3250, "second line")]
        );
    }

    #[test]
    fn repeats_lines_with_several_timestamps_in_order() {
        let lrc = "[00:10.00][00:30.00]chorus\n[00:20.000]verse\n[01:02]outro";
        assert_eq!(
            parse_lrc(lrc),
            vec![
                line(10_000, "chorus"),
                line(20_000, "verse"),
                line(30_000, "chorus"),
                line(62_000, "outro"),
            ]
        );
    }

    #[test]
    fn keeps_empty_timed_lines_as_pauses() {
        assert_eq!(parse_lrc("[00:05.00]"), vec![line(5000, "")]);
    }

    #[test]
    fn active_line_is_the_last_started() {
        let lyrics = Lyrics::Synced(vec![line(1000, "a"), line(2000, "b"), line(3000, "c")]);
        assert_eq!(lyrics.active_line(Duration::from_millis(500)), None);
        assert_eq!(lyrics.active_line(Duration::from_millis(1000)), Some(0));
        assert_eq!(lyrics.active_line(Duration::from_millis(2999)), Some(1));
        assert_eq!(lyrics.active_line(Duration::from_secs(60)), Some(2));
        assert_eq!(Lyrics::Instrumental.active_line(Duration::ZERO), None);
    }

    #[test]
    fn prefers_synced_results_of_the_same_length() {
        let results: Vec<Value> = serde_json::from_str(
            r#"[
                {"duration": 120.0, "syncedLyrics": "[00:01.00]other length"},
                {"duration": 200.0, "plainLyrics": "plain only"},
                {"duration": 201.0, "syncedLyrics": "[00:02.00]placeholder line"}
            ]"#,
        )
        .unwrap();
        assert_eq!(
            best_match(&results, Some(200.)),
            Lyrics::Synced(vec![line(2000, "placeholder line")])
        );
        assert_eq!(best_match(&results, Some(400.)), Lyrics::NotFound);
    }

    #[test]
    fn instrumental_and_plain_records() {
        let instrumental: Value = serde_json::from_str(r#"{"instrumental": true}"#).unwrap();
        assert_eq!(lyrics_from(&instrumental), Lyrics::Instrumental);
        let plain: Value = serde_json::from_str(r#"{"plainLyrics": "one\ntwo"}"#).unwrap();
        assert_eq!(
            lyrics_from(&plain),
            Lyrics::Plain(vec!["one".into(), "two".into()])
        );
    }

    #[test]
    fn main_artist_takes_the_first_name() {
        assert_eq!(main_artist("Alpha, Beta & Gamma"), "Alpha");
        assert_eq!(main_artist("Alpha & Beta"), "Alpha");
        assert_eq!(main_artist("Solo"), "Solo");
    }
}
