//! Keyboard shortcuts. The app handles them (see `YtmApp`); they are off while a text field has
//! the keyboard, so typing a space or moving the cursor never controls the player.

use gpui::{App, KeyBinding, actions};

/// The key context of the app window.
pub const KEY_CONTEXT: &str = "AticApp";
const OUTSIDE_TEXT_FIELDS: &str = "AticApp && !M3TextField";

actions!(
    atic_play,
    [
        TogglePlay,
        NextTrack,
        PreviousTrack,
        SeekForward,
        SeekBackward,
        VolumeUp,
        VolumeDown,
        ToggleMute,
        ToggleShuffle,
        CycleRepeat,
        GoBack,
        Close,
    ]
);

pub fn bind(cx: &mut App) {
    let ctx = Some(OUTSIDE_TEXT_FIELDS);
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, ctx),
        KeyBinding::new("ctrl-right", NextTrack, ctx),
        KeyBinding::new("ctrl-left", PreviousTrack, ctx),
        KeyBinding::new("right", SeekForward, ctx),
        KeyBinding::new("left", SeekBackward, ctx),
        KeyBinding::new("up", VolumeUp, ctx),
        KeyBinding::new("down", VolumeDown, ctx),
        KeyBinding::new("m", ToggleMute, ctx),
        KeyBinding::new("s", ToggleShuffle, ctx),
        KeyBinding::new("r", CycleRepeat, ctx),
        KeyBinding::new("backspace", GoBack, ctx),
        KeyBinding::new("escape", Close, ctx),
        // Going back works from anywhere, as in a browser.
        KeyBinding::new("alt-left", GoBack, Some(KEY_CONTEXT)),
    ]);
}
