//! Lyrics from LRCLIB (https://lrclib.net), synced when possible.
//!
//! Blocking (`ureq`): call it from a background thread. The public API below is the contract the
//! app codes against; keep the names.

mod clean;
mod lrc;
mod lrclib;

use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub start: Duration,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lyrics {
    /// Lines sorted by `start`.
    Synced(Vec<Line>),
    Plain(String),
    Instrumental,
}

impl Lyrics {
    /// Index of the line being sung at `position`, for synced lyrics.
    pub fn line_at(&self, position: Duration) -> Option<usize> {
        let Lyrics::Synced(lines) = self else {
            return None;
        };
        if lines.is_empty() {
            return None;
        }

        // The last line that already started: binary search over the sorted lines.
        let started = lines.partition_point(|line| line.start <= position);
        if started == 0 {
            None
        } else {
            Some(started - 1)
        }
    }
}

/// What we know about the song; `album` and `duration` make the match much better.
#[derive(Clone, Debug)]
pub struct Query<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: Option<&'a str>,
    pub duration: Option<Duration>,
}

/// `Ok(None)` when LRCLIB has nothing for this song.
pub fn fetch(query: &Query) -> anyhow::Result<Option<Lyrics>> {
    lrclib::fetch(query).map_err(anyhow::Error::from)
}

/// Parses LRC text (`[mm:ss.xx]line`).
pub fn parse_lrc(text: &str) -> Vec<Line> {
    lrc::parse(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synced() -> Lyrics {
        Lyrics::Synced(vec![
            Line {
                start: Duration::from_secs(1),
                text: "First".to_string(),
            },
            Line {
                start: Duration::from_secs(5),
                text: "Second".to_string(),
            },
            Line {
                start: Duration::from_secs(9),
                text: "Third".to_string(),
            },
        ])
    }

    #[test]
    fn before_the_first_line_there_is_none() {
        let lyrics = synced();
        assert_eq!(lyrics.line_at(Duration::from_secs(0)), None);
        assert_eq!(lyrics.line_at(Duration::from_millis(999)), None);
    }

    #[test]
    fn it_finds_the_line_being_sung() {
        let lyrics = synced();
        assert_eq!(lyrics.line_at(Duration::from_secs(1)), Some(0));
        assert_eq!(lyrics.line_at(Duration::from_secs(4)), Some(0));
        assert_eq!(lyrics.line_at(Duration::from_secs(5)), Some(1));
        assert_eq!(lyrics.line_at(Duration::from_secs(8)), Some(1));
        assert_eq!(lyrics.line_at(Duration::from_secs(9)), Some(2));
    }

    #[test]
    fn after_the_last_line_it_stays_there() {
        let lyrics = synced();
        assert_eq!(lyrics.line_at(Duration::from_secs(600)), Some(2));
    }

    #[test]
    fn plain_and_instrumental_have_no_lines() {
        assert_eq!(
            Lyrics::Plain("words".to_string()).line_at(Duration::from_secs(1)),
            None
        );
        assert_eq!(Lyrics::Instrumental.line_at(Duration::from_secs(1)), None);
        assert_eq!(
            Lyrics::Synced(Vec::new()).line_at(Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn the_parser_is_part_of_the_public_api() {
        let lines = parse_lrc("[00:01.00]Hi\n[00:02.00]There");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].text, "There");
    }
}
