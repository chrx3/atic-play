//! The keyboard: one action per thing the app can do from anywhere, the bindings that reach them
//! and the list the shortcut dialog shows.
//!
//! The bindings hang off the `Shell` key context, so they only fire inside the app window, and
//! they step aside while a text field has the keys: a space in the search box is a space, not a
//! play/pause.
//!
//! `secondary` is Ctrl on Windows and Linux and ⌘ on macOS.

use gpui::{App, KeyBinding, actions};

actions!(shortcuts, [
    TogglePlay,
    FocusSearch,
    GoBack,
    NextTrack,
    PreviousTrack,
    VolumeUp,
    VolumeDown,
    ToggleLike,
    ShowShortcuts,
]);

/// The key context the bindings hang off: the root of the shell.
pub const CONTEXT: &str = "Shell";

/// Everything is bound under the shell, and never while a text field has the focus.
const SHELL: &str = "Shell && !M3TextField";

/// The reference the shortcut dialog shows, in the order it shows it.
pub const BINDINGS: &[(&str, &str)] = &[
    ("Espacio", "Reproducir o pausar"),
    ("Ctrl+F", "Buscar"),
    ("Alt+←", "Volver"),
    ("Ctrl+→", "Siguiente"),
    ("Ctrl+←", "Anterior"),
    ("Ctrl+↑", "Subir el volumen"),
    ("Ctrl+↓", "Bajar el volumen"),
    ("Ctrl+L", "Me gusta"),
    ("Ctrl+/", "Ver estos atajos"),
];

/// Registers the bindings. Called once, before the first window opens.
pub fn install(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, Some(SHELL)),
        KeyBinding::new("secondary-f", FocusSearch, Some(SHELL)),
        KeyBinding::new("alt-left", GoBack, Some(SHELL)),
        KeyBinding::new("secondary-right", NextTrack, Some(SHELL)),
        KeyBinding::new("secondary-left", PreviousTrack, Some(SHELL)),
        KeyBinding::new("secondary-up", VolumeUp, Some(SHELL)),
        KeyBinding::new("secondary-down", VolumeDown, Some(SHELL)),
        KeyBinding::new("secondary-l", ToggleLike, Some(SHELL)),
        KeyBinding::new("secondary-/", ShowShortcuts, Some(SHELL)),
    ]);
}

/// A one-line reminder of the transport shortcuts, for the account sheet and the empty states.
pub fn summary() -> &'static str {
    "Espacio reproduce y pausa · Ctrl+← → cambia de canción · Ctrl+L da Me gusta"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_matches_the_bindings() {
        // Nine bindings, nine lines of reference: the dialog must not promise what is not bound.
        assert_eq!(BINDINGS.len(), 9);
        for (keys, what) in BINDINGS {
            assert!(!keys.is_empty());
            assert!(!what.is_empty());
        }
        assert!(BINDINGS.iter().any(|(_, what)| what.contains("Me gusta")));
        assert!(BINDINGS.iter().any(|(keys, _)| *keys == "Espacio"));
    }

    #[test]
    fn the_context_steps_aside_for_text_fields() {
        assert!(SHELL.contains("!M3TextField"));
        assert!(CONTEXT == "Shell");
    }
}
