use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($constant:ident => $name:literal),* $(,)?) => {
        $(pub const $constant: &str = concat!("icons/", $name, ".svg");)*

        /// Media and navigation icons gpui-m3 does not ship. Paths end in `.svg`, so gpui-m3's
        /// `Icon` uses them as they are.
        const ICONS: &[(&str, &[u8])] = &[
            $(($constant, include_bytes!(concat!("../assets/icons/", $name, ".svg")))),*
        ];
    };
}

icons! {
    PLAY => "play",
    PAUSE => "pause",
    SKIP_NEXT => "skip-next",
    SKIP_PREVIOUS => "skip-previous",
    SHUFFLE => "shuffle",
    REPEAT => "repeat",
    REPEAT_ONE => "repeat-one",
    VOLUME => "volume",
    VOLUME_OFF => "volume-off",
    QUEUE => "queue",
    HOME => "home",
    EXPLORE => "explore",
    LIBRARY => "library",
    HEART => "heart",
    HEART_FILL => "heart-fill",
    PERSON => "person",
    ALBUM => "album",
    DOWNLOAD => "download",
    DOWNLOAD_DONE => "download-done",
    OFFLINE => "offline",
}

pub const LOGO: &str = "brand/atic-play.png";

/// The app's icons and logo plus gpui-m3's.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == LOGO {
            return Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/brand/atic-play.png"
            ))));
        }
        if let Some((_, bytes)) = ICONS.iter().find(|(name, _)| *name == path) {
            return Ok(Some(Cow::Borrowed(*bytes)));
        }
        gpui_m3::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if path.starts_with("icons") {
            return Ok(ICONS.iter().map(|(name, _)| (*name).into()).collect());
        }
        gpui_m3::Assets.list(path)
    }
}
