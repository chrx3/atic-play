//! The client of the LRCLIB public API (https://lrclib.net).
//!
//! Two endpoints do the work:
//!
//! - `GET /api/get` with the song signature: title, artist, album and duration. It answers `404`
//!   when it has nothing that matches.
//! - `GET /api/search` with keywords, up to 20 records that may match.

use std::time::Duration;

use serde::Deserialize;

use crate::clean;
use crate::lrc;
use crate::{Lyrics, Query};

/// Where the API lives.
const BASE_URL: &str = "https://lrclib.net";

/// How many times we try again when the server asks us to slow down.
const MAX_ATTEMPTS: u32 = 3;

/// The longest we ever wait for a `Retry-After`, in seconds.
const MAX_WAIT_SECS: u64 = 60;

/// A pause between two requests, as the API docs ask for.
const REQUEST_DELAY: Duration = Duration::from_millis(200);

/// How far a record can be from our song and still be a good match, in seconds.
const DURATION_TOLERANCE_SECS: f64 = 5.0;

/// Something went wrong talking to LRCLIB.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("network error: {0}")]
    Network(String),
    #[error("LRCLIB answered {status}: {message}")]
    Api { status: u16, message: String },
    #[error("LRCLIB answered something we could not read: {0}")]
    Decode(String),
}

/// One lyrics record, as LRCLIB sends it.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Record {
    #[serde(default)]
    pub id: i64,
    #[serde(rename = "trackName", default)]
    pub track_name: String,
    #[serde(rename = "artistName", default)]
    pub artist_name: String,
    #[serde(rename = "albumName", default)]
    pub album_name: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub instrumental: bool,
    #[serde(rename = "plainLyrics", default)]
    pub plain_lyrics: Option<String>,
    #[serde(rename = "syncedLyrics", default)]
    pub synced_lyrics: Option<String>,
}

impl Record {
    /// Does this record have anything we can show?
    fn has_content(&self) -> bool {
        self.instrumental || self.has_synced() || self.has_plain()
    }

    /// Are the words timed?
    fn has_synced(&self) -> bool {
        self.synced_lyrics
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
    }

    /// Are there plain words?
    fn has_plain(&self) -> bool {
        self.plain_lyrics
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
    }

    /// How far this record is from our song, in seconds. Unknown durations count as zero.
    fn distance_secs(&self, duration: Option<Duration>) -> f64 {
        match (self.duration, duration) {
            (Some(record), Some(ours)) => (record - ours.as_secs_f64()).abs(),
            _ => 0.0,
        }
    }

    /// Is this record from the album we are looking for?
    fn same_album(&self, album: Option<&str>) -> bool {
        match (album, self.album_name.as_deref()) {
            (Some(wanted), Some(found)) => wanted.eq_ignore_ascii_case(found),
            _ => false,
        }
    }

    /// The lyrics of this record, if it has any.
    pub(crate) fn lyrics(&self) -> Option<Lyrics> {
        if self.instrumental {
            return Some(Lyrics::Instrumental);
        }
        if self.has_synced() {
            let lines = lrc::parse(self.synced_lyrics.as_deref().unwrap_or_default());
            if !lines.is_empty() {
                return Some(Lyrics::Synced(lines));
            }
        }
        if self.has_plain() {
            return Some(Lyrics::Plain(
                self.plain_lyrics
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            ));
        }
        None
    }
}

/// The client that talks to LRCLIB.
pub(crate) struct Client {
    agent: ureq::Agent,
}

impl Client {
    pub(crate) fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .user_agent(user_agent())
            .timeout_global(Some(Duration::from_secs(20)))
            .http_status_as_error(false)
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    /// Looks for the song by its signature. `Ok(None)` means "no exact match".
    pub(crate) fn get(
        &self,
        track_name: &str,
        artist_name: &str,
        album_name: Option<&str>,
        duration: Option<Duration>,
    ) -> Result<Option<Record>, Error> {
        let mut params = vec![
            ("track_name", track_name.to_string()),
            ("artist_name", artist_name.to_string()),
        ];
        if let Some(album) = album_name {
            params.push(("album_name", album.to_string()));
        }
        if let Some(duration) = duration {
            // LRCLIB only accepts durations between 1 and 3600 seconds.
            let seconds = duration.as_secs_f64().round();
            if (1.0..=3600.0).contains(&seconds) {
                params.push(("duration", seconds.to_string()));
            }
        }

        let (status, body) = self.get_json("/api/get", &params)?;
        match status {
            200 => serde_json::from_str::<Record>(&body)
                .map(Some)
                .map_err(|e| Error::Decode(e.to_string())),
            404 => Ok(None),
            status => Err(Error::Api {
                status,
                message: error_message(&body),
            }),
        }
    }

    /// Looks for the song by keywords.
    pub(crate) fn search(&self, track_name: &str, artist_name: &str) -> Result<Vec<Record>, Error> {
        let params = vec![
            ("track_name", track_name.to_string()),
            ("artist_name", artist_name.to_string()),
        ];

        let (status, body) = self.get_json("/api/search", &params)?;
        match status {
            200 => {
                serde_json::from_str::<Vec<Record>>(&body).map_err(|e| Error::Decode(e.to_string()))
            }
            status => Err(Error::Api {
                status,
                message: error_message(&body),
            }),
        }
    }

    /// Does a GET and gives back the status and the body, waiting when the server asks us to.
    fn get_json(&self, path: &str, params: &[(&str, String)]) -> Result<(u16, String), Error> {
        for attempt in 1..=MAX_ATTEMPTS {
            let mut request = self.agent.get(format!("{BASE_URL}{path}"));
            for (key, value) in params {
                request = request.query(*key, value.as_str());
            }

            let mut response = request.call().map_err(|e| Error::Network(e.to_string()))?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok());

            if matches!(status, 429 | 503) && attempt < MAX_ATTEMPTS {
                let wait = retry_after.unwrap_or(if status == 503 { 1 } else { 5 });
                log::warn!("LRCLIB answered {status}; waiting {wait}s before trying again");
                std::thread::sleep(Duration::from_secs(wait.min(MAX_WAIT_SECS)));
                continue;
            }

            let body = response
                .body_mut()
                .read_to_string()
                .map_err(|e| Error::Network(e.to_string()))?;
            return Ok((status, body));
        }

        Err(Error::Api {
            status: 429,
            message: "too many requests".to_string(),
        })
    }
}

/// The `User-Agent` we send, so LRCLIB knows who is calling.
fn user_agent() -> String {
    format!(
        "atic-play/{} (https://github.com/chrx3/atic-play)",
        env!("CARGO_PKG_VERSION")
    )
}

/// The `message` field of the JSON error body, when there is one.
fn error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .and_then(|message| message.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(200).collect())
}

/// Looks for the lyrics of a song in LRCLIB.
pub(crate) fn fetch(query: &Query) -> Result<Option<Lyrics>, Error> {
    let title = clean::title(query.title);
    let artist = clean::artist(query.artist);
    if title.is_empty() || artist.is_empty() {
        return Ok(None);
    }

    let album = query
        .album
        .map(clean::album)
        .filter(|album| !album.is_empty());
    // LRCLIB matches one artist per song, so we only keep the first one.
    let artist = clean::first_artist(&artist).to_string();
    let client = Client::new();

    // 1. The exact match: title, artist, album and duration.
    if let Some(record) = client.get(&title, &artist, album.as_deref(), query.duration)? {
        return Ok(record.lyrics());
    }

    // 2. The fuzzy match: the same words, but we pick the closest record ourselves.
    std::thread::sleep(REQUEST_DELAY);
    let records = client.search(&title, &artist)?;
    let best = pick_best(&records, query.duration, album.as_deref());
    if let Some(record) = best {
        log::debug!(
            "LRCLIB matched \"{}\" by {} (id {})",
            record.track_name,
            record.artist_name,
            record.id
        );
    }
    Ok(best.and_then(Record::lyrics))
}

/// Picks the record that fits our song the best.
///
/// When we know how long the song is, records that are more than five seconds apart are set
/// aside: they are usually another version of the song. Then we prefer the ones with timed
/// words, and finally the one closest in time and in album.
pub(crate) fn pick_best<'a>(
    records: &'a [Record],
    duration: Option<Duration>,
    album: Option<&str>,
) -> Option<&'a Record> {
    let candidates: Vec<&Record> = records.iter().filter(|r| r.has_content()).collect();
    if candidates.is_empty() {
        return None;
    }

    let close: Vec<&Record> = match duration {
        Some(duration) => {
            let close: Vec<&Record> = candidates
                .iter()
                .copied()
                .filter(|r| r.distance_secs(Some(duration)) <= DURATION_TOLERANCE_SECS)
                .collect();
            if close.is_empty() {
                // Nothing is close enough: we take the closest one we have, it is still better
                // than nothing.
                candidates
            } else {
                close
            }
        }
        None => candidates,
    };

    let mut close = close;
    close.sort_by(|a, b| {
        b.has_synced()
            .cmp(&a.has_synced())
            .then_with(|| {
                a.distance_secs(duration)
                    .partial_cmp(&b.distance_secs(duration))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| b.same_album(album).cmp(&a.same_album(album)))
            .then_with(|| a.id.cmp(&b.id))
    });
    close.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(json: &str) -> Vec<Record> {
        serde_json::from_str(json).expect("the fixture must be valid JSON")
    }

    const SEARCH_RESULTS: &str = include_str!("../tests/fixtures/search_results.json");

    #[test]
    fn reads_the_search_results() {
        let records = records(SEARCH_RESULTS);
        assert_eq!(records.len(), 5);
        assert_eq!(records[0].track_name, "Bohemian Rhapsody");
        assert_eq!(records[0].artist_name, "Queen");
        assert_eq!(records[0].duration, Some(355.0));
        assert!(records[0].has_synced());
        assert!(records[1].has_plain());
        assert!(!records[1].has_synced());
        assert!(records[2].instrumental);
    }

    #[test]
    fn picks_the_closest_record() {
        let records = records(SEARCH_RESULTS);
        let best = pick_best(&records, Some(Duration::from_secs(355)), None).expect("a record");
        assert_eq!(best.id, 1);
    }

    #[test]
    fn prefers_timed_words_over_a_closer_one() {
        let records = records(SEARCH_RESULTS);
        // The synced record is two seconds farther away, but it is the one we want.
        let best = pick_best(&records, Some(Duration::from_secs(357)), None).expect("a record");
        assert_eq!(best.id, 1);
    }

    #[test]
    fn drops_records_that_are_too_far_away() {
        let records = records(SEARCH_RESULTS);
        // Every record is more than five seconds away from this one, so the closest wins.
        let best = pick_best(&records, Some(Duration::from_secs(121)), None).expect("a record");
        assert_eq!(best.id, 3);
        assert!(best.distance_secs(Some(Duration::from_secs(121))) <= DURATION_TOLERANCE_SECS);
    }

    #[test]
    fn without_a_duration_it_takes_the_first_that_has_words() {
        let records = records(SEARCH_RESULTS);
        let best = pick_best(&records, None, None).expect("a record");
        assert_eq!(best.id, 1);
    }

    #[test]
    fn the_album_breaks_the_tie() {
        let all = records(SEARCH_RESULTS);
        // Only the records without timed words, so the album is what decides.
        let plain: Vec<Record> = all.into_iter().filter(|r| !r.has_synced()).collect();
        let best = pick_best(&plain, Some(Duration::from_secs(356)), Some("Queen Rocks"))
            .expect("a record");
        assert_eq!(best.id, 5);
    }

    #[test]
    fn a_record_without_words_is_not_chosen() {
        let records = vec![Record {
            id: 9,
            track_name: "Nothing".to_string(),
            artist_name: "Nobody".to_string(),
            album_name: None,
            duration: Some(100.0),
            instrumental: false,
            plain_lyrics: None,
            synced_lyrics: None,
        }];
        assert!(pick_best(&records, None, None).is_none());
    }

    #[test]
    fn the_user_agent_says_who_we_are() {
        let agent = user_agent();
        assert!(agent.starts_with("atic-play/"));
        assert!(agent.contains("https://github.com/chrx3/atic-play"));
    }
}
