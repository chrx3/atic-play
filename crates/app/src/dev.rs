//! Development hooks: open the window in a given state, theme and size, with no clicks.
//!
//! They exist so a design review can capture the app at its interesting places without driving
//! the mouse. Every variable is optional and the app behaves as usual when it is not set; the
//! parsing lives here so [`crate::model::Model`], [`crate::Page`] and `main` can just ask.
//!
//! - `ATIC_PLAY_THEME=light|dark`: starting colour scheme.
//! - `ATIC_PLAY_SIZE=<width>x<height>`: starting window size, in logical pixels.
//! - `ATIC_PLAY_OPEN=<state>`: see [`Open`].

/// The smallest window [`size`] will ask for: below that the rail and the page stop making sense.
const MIN_SIDE: f32 = 360.;

/// What `ATIC_PLAY_OPEN` asks the app to show. The texts are what the user would have typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Open {
    /// The start page. Also what an unknown or empty value gives.
    Home,
    /// Buscar, with the text already searched and the results showing.
    Search(String),
    /// Buscar, with the text typed and the suggestion panel open.
    Suggest(String),
    /// Buscar, with a search that finds nothing (the empty state).
    EmptySearch(String),
    /// The first album of the start page.
    Album,
    /// The first artist of the start page, or of a search for the first song's artist.
    Artist,
    /// The first playlist of the start page.
    Playlist,
    /// The first song of the start page, paused, with the playing sheet open.
    Now,
    /// Like [`Open::Now`], on the lyrics tab.
    Lyrics,
    /// Like [`Open::Now`], with the queue sheet open instead of the playing sheet.
    Queue,
    /// Biblioteca, on one of its tabs.
    Library(crate::library::LibTab),
}

/// `ATIC_PLAY_OPEN`, parsed and logged. `home` when it is missing or not understood.
pub fn open() -> Open {
    let Ok(value) = std::env::var("ATIC_PLAY_OPEN") else {
        return Open::Home;
    };
    match parse_open(&value) {
        Some(open) => {
            log::info!("open: {open:?}");
            open
        }
        None => {
            log::warn!("ATIC_PLAY_OPEN «{}» is not a known state; opening home", value.trim());
            Open::Home
        }
    }
}

/// `light` and `dark` (any case) become the starting scheme; anything else is not understood.
pub fn parse_theme(value: &str) -> Option<bool> {
    match value.trim() {
        value if value.eq_ignore_ascii_case("light") => Some(false),
        value if value.eq_ignore_ascii_case("dark") => Some(true),
        _ => None,
    }
}

/// `ATIC_PLAY_THEME`, then the saved preference, then the system, with the older `M3_MODE=dark`
/// kept as a last fallback. Light by default.
pub fn dark() -> bool {
    if let Ok(value) = std::env::var("ATIC_PLAY_THEME") {
        match parse_theme(&value) {
            Some(dark) => {
                log::info!("theme: {} (ATIC_PLAY_THEME)", if dark { "dark" } else { "light" });
                return dark;
            }
            None => log::warn!(
                "ATIC_PLAY_THEME «{}» is not light or dark; using the saved preference",
                value.trim()
            ),
        }
    }
    if let Some(dark) = crate::settings::dark() {
        log::info!(
            "theme: {} (saved)",
            if dark { "dark" } else { "light" }
        );
        return dark;
    }
    if let Some(dark) = system_dark() {
        log::info!(
            "theme: {} (system)",
            if dark { "dark" } else { "light" }
        );
        return dark;
    }
    legacy_dark()
}

/// The system appearance, once the window told us what it is.
static SYSTEM_DARK: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(-1);

/// Records what Windows says the app should look like (`window.appearance()`), before the model
/// is built. `-1` (unknown) leaves the question to `M3_MODE`.
pub fn set_system_dark(dark: bool) {
    SYSTEM_DARK.store(dark as i8, std::sync::atomic::Ordering::Relaxed);
}

fn system_dark() -> Option<bool> {
    match SYSTEM_DARK.load(std::sync::atomic::Ordering::Relaxed) {
        -1 => None,
        value => Some(value == 1),
    }
}

/// Whether the theme is the user's choice (from the environment or the file) or the system's:
/// only the second one follows the appearance while the app is open.
pub fn theme_follows_system() -> bool {
    if std::env::var("ATIC_PLAY_THEME")
        .ok()
        .is_some_and(|value| parse_theme(&value).is_some())
    {
        return false;
    }
    crate::settings::dark().is_none()
}

fn legacy_dark() -> bool {
    std::env::var("M3_MODE").is_ok_and(|v| v == "dark")
}

/// One `open` value, without the environment around it.
pub fn parse_open(value: &str) -> Option<Open> {
    let value = value.trim();
    if value.is_empty() {
        return Some(Open::Home);
    }
    let (name, text) = match value.split_once(':') {
        Some((name, text)) => (name.trim(), text.trim()),
        None => (value, ""),
    };
    let open = match name.to_ascii_lowercase().as_str() {
        "home" => Open::Home,
        "search" => Open::Search(demo_text(text, "amber")),
        "suggest" => Open::Suggest(demo_text(text, "amber")),
        "empty-search" => Open::EmptySearch(demo_text(text, "zzzz")),
        "album" => Open::Album,
        "artist" => Open::Artist,
        "playlist" => Open::Playlist,
        "now" => Open::Now,
        "lyrics" => Open::Lyrics,
        "queue" => Open::Queue,
        "library" => Open::Library(crate::library::LibTab::from_name(text).unwrap_or_default()),
        _ => return None,
    };
    Some(open)
}

/// A state that needs text still has something to show when none was given.
fn demo_text(text: &str, fallback: &str) -> String {
    if text.is_empty() {
        fallback.to_string()
    } else {
        text.to_string()
    }
}

/// `<width>x<height>`, in logical pixels. Round numbers, at least [`MIN_SIDE`] on each side.
pub fn parse_size(value: &str) -> Option<(f32, f32)> {
    let value = value.trim();
    let (width, height) = value.split_once(['x', 'X', '×'])?;
    let width: f32 = width.trim().parse().ok()?;
    let height: f32 = height.trim().parse().ok()?;
    (width >= MIN_SIDE && height >= MIN_SIDE).then(|| (width.round(), height.round()))
}

/// `ATIC_PLAY_SIZE`; `None` keeps the default window size.
pub fn size() -> Option<(f32, f32)> {
    let value = std::env::var("ATIC_PLAY_SIZE").ok()?;
    match parse_size(&value) {
        Some(size) => {
            log::info!("window size: {}x{}", size.0, size.1);
            Some(size)
        }
        None => {
            log::warn!(
                "ATIC_PLAY_SIZE «{}» is not <width>x<height> ({MIN_SIDE} or more); keeping the default",
                value.trim()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_states_parse() {
        assert_eq!(parse_open("home"), Some(Open::Home));
        assert_eq!(parse_open("  "), Some(Open::Home));
        assert_eq!(
            parse_open("search: body to body "),
            Some(Open::Search("body to body".into()))
        );
        assert_eq!(
            parse_open("SUGGEST:amber"),
            Some(Open::Suggest("amber".into()))
        );
        assert_eq!(parse_open("album"), Some(Open::Album));
        assert_eq!(parse_open("artist"), Some(Open::Artist));
        assert_eq!(parse_open("playlist"), Some(Open::Playlist));
        assert_eq!(parse_open("now"), Some(Open::Now));
        assert_eq!(parse_open("lyrics"), Some(Open::Lyrics));
        assert_eq!(parse_open("queue"), Some(Open::Queue));
        assert_eq!(
            parse_open("library"),
            Some(Open::Library(crate::library::LibTab::Liked))
        );
        assert_eq!(
            parse_open("library:listas"),
            Some(Open::Library(crate::library::LibTab::Playlists))
        );
        assert_eq!(
            parse_open("library:HISTORIAL"),
            Some(Open::Library(crate::library::LibTab::History))
        );
        assert_eq!(parse_open("empty-search"), Some(Open::EmptySearch("zzzz".into())));
        assert_eq!(parse_open("shelf"), None);
    }

    #[test]
    fn open_states_without_text_still_have_something_to_show() {
        assert_eq!(parse_open("search:"), Some(Open::Search("amber".into())));
        assert_eq!(parse_open("suggest:"), Some(Open::Suggest("amber".into())));
        assert_eq!(
            parse_open("empty-search:"),
            Some(Open::EmptySearch("zzzz".into()))
        );
    }

    #[test]
    fn sizes_parse_in_both_x_and_reject_nonsense() {
        assert_eq!(parse_size("800x600"), Some((800., 600.)));
        assert_eq!(parse_size(" 1024 X 720 "), Some((1024., 720.)));
        assert_eq!(parse_size("1280×800"), Some((1280., 800.)));
        assert_eq!(parse_size("1024x720.4"), Some((1024., 720.)));
        assert_eq!(parse_size("100x100"), None);
        assert_eq!(parse_size("1280"), None);
        assert_eq!(parse_size("ancho x alto"), None);
    }

    #[test]
    fn theme_is_light_or_dark_in_any_case() {
        assert_eq!(parse_theme("dark"), Some(true));
        assert_eq!(parse_theme(" DARK "), Some(true));
        assert_eq!(parse_theme("light"), Some(false));
        assert_eq!(parse_theme("blue"), None);
    }
}
