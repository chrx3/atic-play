//! The icons gpui-m3 does not ship (transport, library, queue, volume...) and the app's asset
//! source: our own set plus everything the library bundles.
//!
//! Adapted from gpui-m3's `showcase_music` example (MIT OR Apache-2.0).

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! stroke {
    ($body:expr) => {
        concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">"##,
            $body,
            "</svg>"
        )
    };
}

macro_rules! solid {
    ($body:expr) => {
        concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="#000" stroke="#000" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">"##,
            $body,
            "</svg>"
        )
    };
}

const ICONS: &[(&str, &str)] = &[
    (
        "play",
        solid!(
            r#"<path d="M7 4.6v14.8a1 1 0 0 0 1.5.86l12-7.4a1 1 0 0 0 0-1.72l-12-7.4A1 1 0 0 0 7 4.6z" />"#
        ),
    ),
    (
        "pause",
        solid!(
            r#"<rect x="6" y="4.5" width="4" height="15" rx="1.2" /><rect x="14" y="4.5" width="4" height="15" rx="1.2" />"#
        ),
    ),
    (
        "next",
        solid!(
            r#"<path d="M5 5.6v12.8a1 1 0 0 0 1.5.86l9.2-6.4a1 1 0 0 0 0-1.72L6.5 4.74A1 1 0 0 0 5 5.6z" /><path d="M19 5v14" fill="none" />"#
        ),
    ),
    (
        "previous",
        solid!(
            r#"<path d="M19 5.6v12.8a1 1 0 0 1-1.5.86l-9.2-6.4a1 1 0 0 1 0-1.72l9.2-6.4A1 1 0 0 1 19 5.6z" /><path d="M5 5v14" fill="none" />"#
        ),
    ),
    (
        "shuffle",
        stroke!(
            r#"<path d="m18 14 4 4-4 4" /><path d="m18 2 4 4-4 4" /><path d="M2 18h1.97a4 4 0 0 0 3.3-1.7l5.45-8.6a4 4 0 0 1 3.3-1.7H22" /><path d="M2 6h1.97a4 4 0 0 1 3.6 2.2" /><path d="M22 18h-6.04a4 4 0 0 1-3.3-1.8l-.36-.45" />"#
        ),
    ),
    (
        "repeat",
        stroke!(
            r#"<path d="m17 2 4 4-4 4" /><path d="M3 11v-1a4 4 0 0 1 4-4h14" /><path d="m7 22-4-4 4-4" /><path d="M21 13v1a4 4 0 0 1-4 4H3" />"#
        ),
    ),
    (
        "repeat-one",
        stroke!(
            r#"<path d="m17 2 4 4-4 4" /><path d="M3 11v-1a4 4 0 0 1 4-4h14" /><path d="m7 22-4-4 4-4" /><path d="M21 13v1a4 4 0 0 1-4 4H3" /><path d="M11 10h1v5" />"#
        ),
    ),
    (
        "volume",
        stroke!(
            r#"<path d="M11 4.7a.7.7 0 0 0-1.2-.5L6.4 7.6A1.4 1.4 0 0 1 5.4 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.4a1.4 1.4 0 0 1 1 .4l3.4 3.4a.7.7 0 0 0 1.2-.5z" /><path d="M16 9a5 5 0 0 1 0 6" /><path d="M19.4 18.4a9 9 0 0 0 0-12.8" />"#
        ),
    ),
    (
        "volume-low",
        stroke!(
            r#"<path d="M11 4.7a.7.7 0 0 0-1.2-.5L6.4 7.6A1.4 1.4 0 0 1 5.4 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.4a1.4 1.4 0 0 1 1 .4l3.4 3.4a.7.7 0 0 0 1.2-.5z" /><path d="M16 9a5 5 0 0 1 0 6" />"#
        ),
    ),
    (
        "volume-off",
        stroke!(
            r#"<path d="M11 4.7a.7.7 0 0 0-1.2-.5L6.4 7.6A1.4 1.4 0 0 1 5.4 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.4a1.4 1.4 0 0 1 1 .4l3.4 3.4a.7.7 0 0 0 1.2-.5z" /><path d="m16 9 6 6" /><path d="m22 9-6 6" />"#
        ),
    ),
    (
        "home",
        stroke!(
            r#"<path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8" /><path d="M3 10a2 2 0 0 1 .7-1.5l7-6a2 2 0 0 1 2.6 0l7 6A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />"#
        ),
    ),
    (
        "music",
        stroke!(
            r#"<path d="M9 18V5l12-2v13" /><circle cx="6" cy="18" r="3" /><circle cx="18" cy="16" r="3" />"#
        ),
    ),
    (
        "queue",
        stroke!(
            r#"<path d="M21 15V6" /><path d="M18.5 18a2.5 2.5 0 1 0 0-5 2.5 2.5 0 0 0 0 5z" /><path d="M12 12H3" /><path d="M16 6H3" /><path d="M12 18H3" />"#
        ),
    ),
    (
        "waves",
        stroke!(
            r#"<path d="M2 10v3" /><path d="M6 6v11" /><path d="M10 3v18" /><path d="M14 8v7" /><path d="M18 5v13" /><path d="M22 10v3" />"#
        ),
    ),
    (
        "lyrics",
        stroke!(
            r#"<path d="M4 5h16" /><path d="M4 10h10" /><path d="M4 15h13" /><path d="M4 20h7" /><path d="M19 9v10a2 2 0 1 0 2-2" />"#
        ),
    ),
    (
        "disc",
        stroke!(
            r#"<circle cx="12" cy="12" r="9" /><circle cx="12" cy="12" r="2.5" />"#
        ),
    ),
    (
        "grip",
        solid!(
            r#"<circle cx="9" cy="6" r="1.2" /><circle cx="15" cy="6" r="1.2" /><circle cx="9" cy="12" r="1.2" /><circle cx="15" cy="12" r="1.2" /><circle cx="9" cy="18" r="1.2" /><circle cx="15" cy="18" r="1.2" />"#
        ),
    ),
    (
        "account",
        stroke!(
            r#"<circle cx="12" cy="8.5" r="3.8" /><path d="M4.5 20.2a7.5 7.5 0 0 1 15 0" />"#
        ),
    ),
    (
        "heart",
        stroke!(
            r#"<path d="M12 20.2s-7.6-4.6-7.6-9.7A4.3 4.3 0 0 1 12 7.6a4.3 4.3 0 0 1 7.6 2.9c0 5.1-7.6 9.7-7.6 9.7z" />"#
        ),
    ),
    (
        "heart-filled",
        solid!(
            r#"<path d="M12 20.4s-7.8-4.7-7.8-9.9A4.4 4.4 0 0 1 12 7.4a4.4 4.4 0 0 1 7.8 3.1c0 5.2-7.8 9.9-7.8 9.9z" />"#
        ),
    ),
    (
        "library",
        stroke!(
            r#"<path d="M4 4.5v15" /><path d="M8.5 4.5v15" /><path d="M13 4.5v15" /><path d="M17.4 5.2l3.1-.8 3.4 14.6-3.1.8z" />"#
        ),
    ),
    (
        "settings",
        stroke!(
            r#"<circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.9.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.9 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1A1.7 1.7 0 0 0 4.6 9a1.7 1.7 0 0 0-.3-1.9l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1A1.7 1.7 0 0 0 9 4.6h.1A1.7 1.7 0 0 0 10 3.1V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.9-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.9v.1a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />"#
        ),
    ),
];

/// The library's assets plus this app's icons.
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path
            .strip_prefix("music/")
            .and_then(|n| n.strip_suffix(".svg"))
        {
            return Ok(ICONS
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, svg)| Cow::Borrowed(svg.as_bytes())));
        }
        gpui_m3::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_m3::Assets.list(path)
    }
}

/// Path of one of this app's icons, for `Icon::new` and every component that takes an icon.
pub fn icon(name: &str) -> SharedString {
    format!("music/{name}.svg").into()
}
