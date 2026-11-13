//! The rules that decide which state the engine shows.
//!
//! They live apart from the engine loop so they can be tested without a sound card.

use crate::State;

/// What happened in the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// The app asked to play.
    Play,
    /// The app asked to pause.
    Pause,
    /// The first audio of the track reached the device.
    AudioStarted,
    /// The track played to the end.
    Ended,
    /// Something went wrong.
    Failed,
}

/// The state the engine shows after `change`, knowing the state it was in.
///
/// While the track is still loading the state stays `Loading`: the decoder says when the audio
/// arrives, so the app never shows a playing song that is still silent.
pub fn next(current: State, change: Change) -> State {
    match change {
        Change::Play => match current {
            State::Loading => State::Loading,
            _ => State::Playing,
        },
        Change::Pause => State::Paused,
        Change::AudioStarted => State::Playing,
        Change::Ended => State::Ended,
        Change::Failed => State::Idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_audio_turns_loading_into_playing() {
        assert_eq!(next(State::Loading, Change::AudioStarted), State::Playing);
    }

    #[test]
    fn playing_while_loading_waits_for_the_audio() {
        assert_eq!(next(State::Loading, Change::Play), State::Loading);
    }

    #[test]
    fn playing_again_starts_over() {
        assert_eq!(next(State::Paused, Change::Play), State::Playing);
        assert_eq!(next(State::Ended, Change::Play), State::Playing);
    }

    #[test]
    fn pausing_and_ending_have_their_own_states() {
        assert_eq!(next(State::Playing, Change::Pause), State::Paused);
        assert_eq!(next(State::Playing, Change::Ended), State::Ended);
    }

    #[test]
    fn a_failure_goes_back_to_idle() {
        assert_eq!(next(State::Loading, Change::Failed), State::Idle);
        assert_eq!(next(State::Playing, Change::Failed), State::Idle);
    }
}
