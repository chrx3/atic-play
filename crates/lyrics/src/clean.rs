//! Cleaning of the titles and artist names we get from YouTube Music.
//!
//! Video titles are noisy: `(feat. Someone)`, `(Remastered 2011)`, `[Official Video]`,
//! `- Topic`, `VEVO`… LRCLIB stores plain names, so we take that noise away before asking it.

/// Words that mark a bracketed group as noise: `(feat. X)`, `[Official Video]`, `(Remastered 2011)`…
const NOISE_WORDS: &[&str] = &[
    "feat",
    "ft",
    "featuring",
    "remaster",
    "remastered",
    "official",
    "lyric",
    "lyrics",
    "audio",
    "hd",
    "hq",
    "4k",
    "mv",
    "visualizer",
    "vevo",
    "topic",
];

/// Noise that only shows up as more than one word.
const NOISE_PHRASES: &[&str] = &[
    "official video",
    "official music video",
    "official audio",
    "official lyric video",
    "lyric video",
    "music video",
];

/// Separators that YouTube uses between the name and the noise: `Song - Official Video`.
const DASH_SEPARATORS: &[&str] = &[" - ", " – ", " — "];

/// Cleans a song title: drops noise groups, noise after a dash and squeezes the spaces.
pub(crate) fn title(raw: &str) -> String {
    let without_groups = strip_noise_groups(raw);
    let without_suffix = drop_noise_suffix(&without_groups);
    squeeze(&without_suffix)
}

/// Cleans an artist name: same as [`title`], plus the `- Topic` and `VEVO` leftovers YouTube adds.
pub(crate) fn artist(raw: &str) -> String {
    let without_groups = strip_noise_groups(raw);
    let without_suffix = drop_noise_suffix(&without_groups);
    let without_vevo = strip_vevo(&without_suffix);
    squeeze(&without_vevo)
}

/// Cleans an album name: noise groups and the `- Topic` suffix, nothing else.
pub(crate) fn album(raw: &str) -> String {
    let without_groups = strip_noise_groups(raw);
    let without_suffix = drop_noise_suffix(&without_groups);
    squeeze(&without_suffix)
}

/// The first artist of a list: `A, B`, `A & B` and `A feat. B` are all `A`.
pub(crate) fn first_artist(raw: &str) -> &str {
    let head = match cut_at_feature(raw) {
        Some((head, _)) => head,
        None => raw,
    };
    head.split([',', '&']).next().unwrap_or(head).trim()
}

/// Removes bracketed groups that only hold noise: `(feat. X)`, `[Official Video]`…
fn strip_noise_groups(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;

    while index < chars.len() {
        let current = chars[index];
        if let Some(closing) = closing_bracket(current)
            && let Some(end) = find_closing(&chars, index + 1, closing)
        {
            let inner: String = chars[index + 1..end].iter().collect();
            if is_noise(&inner) {
                while out.ends_with(' ') {
                    out.pop();
                }
                index = end + 1;
                continue;
            }
        }
        out.push(current);
        index += 1;
    }

    out
}

/// Drops a trailing noise segment: `Song - Official Video` becomes `Song`.
fn drop_noise_suffix(text: &str) -> String {
    let mut end = text.len();
    while let Some((index, separator)) = last_separator(&text[..end]) {
        let tail = &text[index + separator.len()..end];
        if !is_noise(tail) {
            break;
        }
        end = index;
    }
    text[..end].trim_end().to_string()
}

/// The last dash separator of the text, with its length.
fn last_separator(text: &str) -> Option<(usize, &'static str)> {
    DASH_SEPARATORS
        .iter()
        .filter_map(|separator| text.rfind(separator).map(|index| (index, *separator)))
        .max_by_key(|(index, _)| *index)
}

/// Removes the `VEVO` mark YouTube puts in some channel names: `ArtistVEVO` is `Artist`.
fn strip_vevo(text: &str) -> String {
    let trimmed = text.trim_end();
    let mark: String = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    if !mark.eq_ignore_ascii_case("vevo") {
        return trimmed.to_string();
    }
    let head: String = trimmed
        .chars()
        .take(trimmed.chars().count() - mark.chars().count())
        .collect();
    head.trim_end().to_string()
}

/// Cuts `Artist feat. Someone` right before the feature marker.
fn cut_at_feature(text: &str) -> Option<(&str, &str)> {
    const MARKERS: &[&str] = &[" feat. ", " feat ", " ft. ", " ft ", " featuring "];
    MARKERS
        .iter()
        .filter_map(|marker| find_ci(text, marker).map(|index| (index, *marker)))
        .min_by_key(|(index, _)| *index)
        .map(|(index, marker)| (&text[..index], &text[index + marker.len()..]))
}

/// Is this bracketed content something we should throw away?
fn is_noise(inner: &str) -> bool {
    let lower = inner.to_lowercase();
    if NOISE_PHRASES.iter().any(|phrase| lower.contains(phrase)) {
        return true;
    }
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    NOISE_WORDS.iter().any(|word| words.contains(word))
}

/// Collapses the spaces and trims the edges.
fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The bracket that closes this one, if any.
fn closing_bracket(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// Index of the bracket that closes the one we are inside of.
fn find_closing(chars: &[char], from: usize, closing: char) -> Option<usize> {
    (from..chars.len()).find(|&index| chars[index] == closing)
}

/// Case-insensitive search that keeps the byte offsets of the original string.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let hay: Vec<char> = haystack.chars().collect();
    let pat: Vec<char> = needle.chars().map(|c| c.to_ascii_lowercase()).collect();
    if pat.is_empty() || hay.len() < pat.len() {
        return None;
    }
    let match_at = (0..=hay.len() - pat.len()).find(|&index| {
        hay[index..index + pat.len()]
            .iter()
            .zip(pat.iter())
            .all(|(left, right)| left.to_ascii_lowercase() == *right)
    })?;
    haystack
        .char_indices()
        .nth(match_at)
        .map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_drops_feature_and_remaster() {
        assert_eq!(title("Song Name (feat. Someone Else)"), "Song Name");
        assert_eq!(title("Song Name (Remastered 2011)"), "Song Name");
        assert_eq!(title("Song Name [Official Video]"), "Song Name");
        assert_eq!(title("Song Name (Official Music Video)"), "Song Name");
        assert_eq!(title("Song Name [Lyrics]"), "Song Name");
    }

    #[test]
    fn title_drops_noise_after_a_dash() {
        assert_eq!(title("Song Name - Official Video"), "Song Name");
        assert_eq!(title("Song Name - Topic"), "Song Name");
        assert_eq!(title("Song Name - Lyrics"), "Song Name");
    }

    #[test]
    fn title_keeps_meaningful_words() {
        assert_eq!(title("Song Name (Acoustic)"), "Song Name (Acoustic)");
        assert_eq!(title("Song Name"), "Song Name");
        assert_eq!(title("  Song   Name  "), "Song Name");
    }

    #[test]
    fn artist_drops_topic_and_vevo() {
        assert_eq!(artist("Queen - Topic"), "Queen");
        assert_eq!(artist("ArtistVEVO"), "Artist");
        assert_eq!(artist("Artist (Official)"), "Artist");
        assert_eq!(artist("Artist - Topic"), "Artist");
    }

    #[test]
    fn first_artist_takes_the_first_name() {
        assert_eq!(first_artist("A, B"), "A");
        assert_eq!(first_artist("A & B"), "A");
        assert_eq!(first_artist("A feat. B"), "A");
        assert_eq!(first_artist("A ft. B"), "A");
        assert_eq!(first_artist("A featuring B"), "A");
        assert_eq!(first_artist("Solo"), "Solo");
    }

    #[test]
    fn album_drops_noise() {
        assert_eq!(album("Album Name - Topic"), "Album Name");
        assert_eq!(album("Album Name (Deluxe)"), "Album Name (Deluxe)");
    }

    #[test]
    fn it_handles_the_titles_youtube_music_gives() {
        // The shapes YouTube Music uses for the same song, over and over.
        assert_eq!(
            title("Bohemian Rhapsody - Remastered 2011"),
            "Bohemian Rhapsody"
        );
        assert_eq!(
            title("Bohemian Rhapsody (Remastered 2011)"),
            "Bohemian Rhapsody"
        );
        assert_eq!(
            title("Bohemian Rhapsody - Official Video (Remastered 2011)"),
            "Bohemian Rhapsody"
        );
        assert_eq!(
            title("Nothing Else Matters (Official Video)"),
            "Nothing Else Matters"
        );
        assert_eq!(artist("Queen - Topic"), "Queen");
        assert_eq!(artist("Beyoncé"), "Beyoncé");
    }

    #[test]
    fn a_title_with_a_dash_that_is_not_noise_is_kept() {
        assert_eq!(
            title("Would You Please Stay - Interlude"),
            "Would You Please Stay - Interlude"
        );
        assert_eq!(title("A - B"), "A - B");
    }
}
