//! The LRC parser: `[mm:ss.xx]line`.
//!
//! LRCLIB hands us plain LRC: one or more time tags per line, some metadata tags at the top and
//! the occasional empty line that means "nobody is singing right now".

use std::time::Duration;

use crate::Line;

/// Parses LRC text into lines sorted by `start`.
pub(crate) fn parse(text: &str) -> Vec<Line> {
    let offset = find_offset(text);
    let mut lines: Vec<Line> = Vec::new();

    for raw in text.lines() {
        let raw = raw.trim_end_matches('\r');
        let mut rest = raw.trim_start();
        let mut stamps: Vec<u64> = Vec::new();

        // A line can carry several time tags: `[00:12.00][01:30.00]Same words`.
        while let Some(after_bracket) = rest.strip_prefix('[') {
            let Some(end) = after_bracket.find(']') else {
                break;
            };
            let Some(ms) = timestamp_ms(&after_bracket[..end]) else {
                break; // a metadata tag such as `[ar:Someone]`: not a time.
            };
            stamps.push(ms);
            rest = after_bracket[end + 1..].trim_start();
        }

        if stamps.is_empty() {
            continue; // metadata or a line without time: nothing to sing.
        }

        let content = strip_word_tags(rest);
        let content = content.trim();
        for ms in stamps {
            lines.push(Line {
                start: Duration::from_millis(shift(ms, offset)),
                text: content.to_string(),
            });
        }
    }

    // `sort_by_key` is stable, so lines with the same time keep the order they came in.
    lines.sort_by_key(|line| line.start);
    lines
}

/// The global shift asked for by `[offset:+500]`, in milliseconds.
fn find_offset(text: &str) -> i64 {
    for raw in text.lines() {
        let line = raw.trim();
        let Some(after_bracket) = line.strip_prefix('[') else {
            continue;
        };
        let Some(end) = after_bracket.find(']') else {
            continue;
        };
        let inner = after_bracket[..end].trim().to_ascii_lowercase();
        let Some(value) = inner.strip_prefix("offset:") else {
            continue;
        };
        if let Ok(offset) = value.trim().parse::<i64>() {
            return offset;
        }
    }
    0
}

/// Applies the offset: a positive one makes the words show up sooner, as the LRC spec asks.
fn shift(ms: u64, offset: i64) -> u64 {
    let shifted = ms as i64 - offset;
    if shifted < 0 { 0 } else { shifted as u64 }
}

/// `[mm:ss.xx]`, `[mm:ss.xxx]` or `[mm:ss]`: how many milliseconds since the start.
fn timestamp_ms(inner: &str) -> Option<u64> {
    let inner = inner.trim();
    let mut parts = inner.split(':');
    let first = parts.next()?;
    let second = parts.next()?;
    let third = parts.next();
    if parts.next().is_some() {
        return None;
    }

    // `[hh:mm:ss.xx]` is rare but legal.
    let (hours, minutes, seconds) = match third {
        Some(seconds) => (
            first.parse::<u64>().ok()?,
            second.parse::<u64>().ok()?,
            seconds,
        ),
        None => (0, first.parse::<u64>().ok()?, second),
    };

    let (whole, fraction) = match seconds.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (seconds, None),
    };
    let whole = whole.trim().parse::<u64>().ok()?;

    // `.5` is half a second, `.50` is also half a second and `.500` too.
    let millis = match fraction {
        None => 0,
        Some(fraction) => {
            let digits: String = fraction.chars().filter(char::is_ascii_digit).collect();
            if digits.len() > 3 {
                return None;
            }
            let padded = format!("{digits:0<3}");
            padded.parse::<u64>().ok()?
        }
    };

    Some(((hours * 60 + minutes) * 60 + whole) * 1000 + millis)
}

/// Removes the word-level tags of the enhanced LRC: `<00:12.00>`.
fn strip_word_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start..].find('>') else {
            break;
        };
        let end = start + end;
        out.push_str(&rest[..start]);
        if timestamp_ms(&rest[start + 1..end]).is_none() {
            out.push_str(&rest[start..=end]);
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn reads_a_simple_file() {
        let lines = parse("[00:12.00]Hello\n[00:15.50]World");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].start, secs(12_000));
        assert_eq!(lines[0].text, "Hello");
        assert_eq!(lines[1].start, secs(15_500));
        assert_eq!(lines[1].text, "World");
    }

    #[test]
    fn reads_the_fixture() {
        let lines = parse(include_str!("../tests/fixtures/sample.lrc"));
        assert_eq!(lines.len(), 7);

        assert_eq!(lines[0].start, secs(1_000));
        assert_eq!(lines[0].text, "First line");
        assert_eq!(lines[1].start, secs(2_500));
        assert_eq!(lines[1].text, "Second line");
        assert_eq!(lines[2].start, secs(4_000));
        assert_eq!(lines[2].text, ""); // a pause
        assert_eq!(lines[3].start, secs(4_000));
        assert_eq!(lines[3].text, "Same words twice");
        assert_eq!(lines[4].start, secs(6_000));
        assert_eq!(lines[4].text, "Repeated words");
        assert_eq!(lines[5].start, secs(6_000));
        assert_eq!(lines[5].text, "Repeated words");
        assert_eq!(lines[6].start, secs(9_000));
        assert_eq!(lines[6].text, "Last line");
    }

    #[test]
    fn ignores_metadata_tags() {
        let lines = parse("[ar:Someone]\n[ti:A Song]\n[00:03.00]Words");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Words");
    }

    #[test]
    fn applies_the_offset() {
        // `[offset:+500]` makes the words show up 500 ms sooner.
        let lines = parse("[offset:+500]\n[00:10.00]Words");
        assert_eq!(lines[0].start, secs(9_500));

        let lines = parse("[offset:-500]\n[00:10.00]Words");
        assert_eq!(lines[0].start, secs(10_500));
    }

    #[test]
    fn reads_three_decimals() {
        let lines = parse("[00:12.345]Words");
        assert_eq!(lines[0].start, secs(12_345));
    }

    #[test]
    fn sorts_by_time() {
        let lines = parse("[00:20.00]Later\n[00:10.00]Sooner");
        assert_eq!(lines[0].text, "Sooner");
        assert_eq!(lines[1].text, "Later");
    }

    #[test]
    fn drops_the_word_tags_of_enhanced_lrc() {
        let lines = parse("[00:12.00]<00:12.00>Hello <00:12.50>world");
        assert_eq!(lines[0].text, "Hello world");
    }

    #[test]
    fn an_empty_file_gives_no_lines() {
        assert!(parse("").is_empty());
        assert!(parse("just some words").is_empty());
    }
}
