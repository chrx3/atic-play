use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Track;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    #[default]
    Off,
    All,
    One,
}

/// The play queue: what is playing, what comes next, shuffle and repeat.
#[derive(Debug)]
pub struct Queue {
    tracks: Vec<Track>,
    current: Option<usize>,
    repeat: RepeatMode,
    /// The order before shuffling, restored when shuffle is turned off. `Some` means shuffle is on.
    unshuffled: Option<Vec<Track>>,
    rng_state: u64,
}

impl Default for Queue {
    fn default() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Queue {
            tracks: Vec::new(),
            current: None,
            repeat: RepeatMode::Off,
            unshuffled: None,
            // xorshift never leaves zero, so make sure it does not start there.
            rng_state: seed | 1,
        }
    }
}

impl Queue {
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    pub fn current(&self) -> Option<&Track> {
        self.tracks.get(self.current?)
    }

    /// Tracks after the current one.
    pub fn upcoming(&self) -> &[Track] {
        match self.current {
            Some(index) => &self.tracks[index + 1..],
            None => &self.tracks,
        }
    }

    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    pub fn shuffle(&self) -> bool {
        self.unshuffled.is_some()
    }

    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    /// Puts back a queue saved earlier, in the order it was saved, without reshuffling it. With
    /// `shuffle` on, turning it off later keeps that order (the original one is not saved).
    pub fn restore(&mut self, tracks: Vec<Track>, current: Option<usize>, shuffle: bool) {
        self.current = current.filter(|index| *index < tracks.len());
        self.unshuffled = shuffle.then(|| tracks.clone());
        self.tracks = tracks;
    }

    /// Replaces the queue and makes `start` the current track.
    pub fn replace(&mut self, tracks: Vec<Track>, start: usize) {
        self.current = (start < tracks.len()).then_some(start);
        self.tracks = tracks;
        if self.shuffle() {
            self.unshuffled = Some(self.tracks.clone());
            self.shuffle_upcoming();
        }
    }

    /// Adds tracks at the end, skipping any that are already queued.
    pub fn append(&mut self, tracks: impl IntoIterator<Item = Track>) {
        for track in tracks {
            if self.tracks.iter().any(|queued| queued.id == track.id) {
                continue;
            }
            if let Some(unshuffled) = &mut self.unshuffled {
                unshuffled.push(track.clone());
            }
            self.tracks.push(track);
        }
        if self.current.is_none() && !self.tracks.is_empty() {
            self.current = Some(0);
        }
    }

    /// Puts a track right after the current one ("play next"). A track already queued later is
    /// moved there instead of repeated.
    pub fn insert_next(&mut self, track: Track) {
        let Some(current) = self.current else {
            self.append([track]);
            return;
        };
        if self.tracks[current].id == track.id {
            return;
        }
        if let Some(index) = self.tracks.iter().position(|queued| queued.id == track.id) {
            self.tracks.remove(index);
            if index < current {
                self.current = Some(current - 1);
            }
        }
        let at = self.current.map_or(0, |index| index + 1);
        self.tracks.insert(at, track.clone());
        if let Some(unshuffled) = &mut self.unshuffled {
            // Keep the unshuffled order consistent: the track follows the current one there too.
            unshuffled.retain(|queued| queued.id != track.id);
            let current_id = &self.tracks[at - 1].id;
            let after = unshuffled
                .iter()
                .position(|queued| &queued.id == current_id)
                .map_or(unshuffled.len(), |index| index + 1);
            unshuffled.insert(after, track);
        }
    }

    /// Takes a track that has not played yet out of the queue. The current track stays.
    pub fn remove(&mut self, index: usize) {
        if index >= self.tracks.len() || Some(index) == self.current {
            return;
        }
        let removed = self.tracks.remove(index);
        if let Some(current) = self.current
            && index < current
        {
            self.current = Some(current - 1);
        }
        if let Some(unshuffled) = &mut self.unshuffled {
            unshuffled.retain(|queued| queued.id != removed.id);
        }
    }

    /// Takes every track after the current one out of the queue.
    pub fn clear_upcoming(&mut self) {
        let keep = self.current.map_or(0, |index| index + 1);
        let removed: Vec<String> = self.tracks.drain(keep..).map(|track| track.id).collect();
        if let Some(unshuffled) = &mut self.unshuffled {
            unshuffled.retain(|queued| !removed.contains(&queued.id));
        }
    }

    pub fn jump_to(&mut self, index: usize) -> Option<&Track> {
        if index < self.tracks.len() {
            self.current = Some(index);
        }
        self.current()
    }

    /// Moves past the current track. `finished` means it ended on its own, which is when
    /// Repeat One plays it again; skipping by hand always moves on.
    pub fn advance(&mut self, finished: bool) -> Option<&Track> {
        let index = self.current?;
        if finished && self.repeat == RepeatMode::One {
            return self.current();
        }
        if index + 1 < self.tracks.len() {
            self.current = Some(index + 1);
        } else if self.repeat == RepeatMode::Off {
            return None;
        } else {
            self.current = Some(0);
        }
        self.current()
    }

    /// Goes back one track; from the first one, wraps around only with repeat on.
    pub fn previous(&mut self) -> Option<&Track> {
        let index = self.current?;
        if index > 0 {
            self.current = Some(index - 1);
        } else if self.repeat != RepeatMode::Off {
            self.current = Some(self.tracks.len() - 1);
        }
        self.current()
    }

    /// Off → All → One → Off.
    pub fn cycle_repeat(&mut self) -> RepeatMode {
        self.repeat = match self.repeat {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        self.repeat
    }

    /// Shuffling reorders only what comes next, so the current track keeps playing.
    pub fn set_shuffle(&mut self, shuffle: bool) {
        if shuffle == self.shuffle() {
            return;
        }
        if shuffle {
            self.unshuffled = Some(self.tracks.clone());
            self.shuffle_upcoming();
        } else if let Some(original) = self.unshuffled.take() {
            let current_id = self.current().map(|track| track.id.clone());
            self.tracks = original;
            self.current = current_id
                .and_then(|id| self.tracks.iter().position(|track| track.id == id))
                .or(self.current);
        }
    }

    fn shuffle_upcoming(&mut self) {
        let start = self.current.map_or(0, |index| index + 1);
        // Fisher–Yates over the upcoming tracks.
        for i in (start + 1..self.tracks.len()).rev() {
            let j = start + (self.next_random() % (i - start + 1) as u64) as usize;
            self.tracks.swap(i, j);
        }
    }

    fn next_random(&mut self) -> u64 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x;
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str) -> Track {
        Track {
            id: id.to_string(),
            title: id.to_uppercase(),
            artists: String::new(),
            album: None,
            duration_secs: None,
            cover_url: None,
            album_id: None,
            artist_id: None,
        }
    }

    fn queue_of(ids: &[&str], start: usize) -> Queue {
        let mut queue = Queue::default();
        queue.replace(ids.iter().map(|id| track(id)).collect(), start);
        queue
    }

    fn ids(queue: &Queue) -> Vec<&str> {
        queue
            .tracks()
            .iter()
            .map(|track| track.id.as_str())
            .collect()
    }

    fn current_id(queue: &Queue) -> Option<&str> {
        queue.current().map(|track| track.id.as_str())
    }

    #[test]
    fn advance_stops_at_the_end_without_repeat() {
        let mut queue = queue_of(&["a", "b"], 0);
        assert_eq!(queue.advance(true).map(|t| t.id.as_str()), Some("b"));
        assert!(queue.advance(true).is_none());
        assert_eq!(current_id(&queue), Some("b"));
    }

    #[test]
    fn repeat_all_wraps_both_ways() {
        let mut queue = queue_of(&["a", "b", "c"], 2);
        queue.cycle_repeat();
        assert_eq!(queue.advance(true).map(|t| t.id.as_str()), Some("a"));
        assert_eq!(queue.previous().map(|t| t.id.as_str()), Some("c"));
    }

    #[test]
    fn repeat_one_replays_only_when_the_track_finished() {
        let mut queue = queue_of(&["a", "b"], 0);
        queue.cycle_repeat();
        queue.cycle_repeat();
        assert_eq!(queue.repeat(), RepeatMode::One);
        assert_eq!(queue.advance(true).map(|t| t.id.as_str()), Some("a"));
        assert_eq!(queue.advance(false).map(|t| t.id.as_str()), Some("b"));
    }

    #[test]
    fn previous_stays_on_the_first_track_without_repeat() {
        let mut queue = queue_of(&["a", "b"], 0);
        assert_eq!(queue.previous().map(|t| t.id.as_str()), Some("a"));
    }

    #[test]
    fn append_skips_queued_tracks() {
        let mut queue = queue_of(&["a", "b"], 0);
        queue.append([track("b"), track("c")]);
        assert_eq!(ids(&queue), ["a", "b", "c"]);
    }

    #[test]
    fn shuffle_keeps_played_tracks_and_restores_the_order() {
        let all = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let mut queue = queue_of(&all, 2);
        queue.set_shuffle(true);

        assert_eq!(&ids(&queue)[..3], ["a", "b", "c"]);
        assert_eq!(current_id(&queue), Some("c"));
        let mut shuffled = ids(&queue);
        shuffled.sort();
        assert_eq!(shuffled, all);

        queue.advance(false);
        let playing = current_id(&queue).unwrap().to_string();
        queue.set_shuffle(false);
        assert_eq!(ids(&queue), all);
        assert_eq!(current_id(&queue), Some(playing.as_str()));
    }

    #[test]
    fn insert_next_goes_right_after_the_current_track() {
        let mut queue = queue_of(&["a", "b", "c"], 0);
        queue.insert_next(track("x"));
        assert_eq!(ids(&queue), ["a", "x", "b", "c"]);
        assert_eq!(current_id(&queue), Some("a"));
    }

    #[test]
    fn insert_next_moves_a_queued_track_instead_of_repeating_it() {
        let mut queue = queue_of(&["a", "b", "c", "d"], 1);
        queue.insert_next(track("d"));
        assert_eq!(ids(&queue), ["a", "b", "d", "c"]);
        // Moving a track from before the current one keeps the current one playing.
        queue.insert_next(track("a"));
        assert_eq!(ids(&queue), ["b", "a", "d", "c"]);
        assert_eq!(current_id(&queue), Some("b"));
        // The current track itself is left alone.
        queue.insert_next(track("b"));
        assert_eq!(ids(&queue), ["b", "a", "d", "c"]);
    }

    #[test]
    fn insert_next_survives_turning_shuffle_off() {
        let mut queue = queue_of(&["a", "b", "c", "d", "e"], 0);
        queue.set_shuffle(true);
        queue.insert_next(track("x"));
        assert_eq!(ids(&queue)[1], "x");
        queue.set_shuffle(false);
        assert_eq!(ids(&queue), ["a", "x", "b", "c", "d", "e"]);
    }

    #[test]
    fn insert_next_into_an_empty_queue_starts_it() {
        let mut queue = Queue::default();
        queue.insert_next(track("x"));
        assert_eq!(current_id(&queue), Some("x"));
    }

    #[test]
    fn remove_keeps_the_current_track_playing() {
        let mut queue = queue_of(&["a", "b", "c", "d"], 2);
        queue.remove(0);
        assert_eq!(ids(&queue), ["b", "c", "d"]);
        assert_eq!(current_id(&queue), Some("c"));
        queue.remove(1);
        assert_eq!(
            ids(&queue),
            ["b", "c", "d"],
            "the current track is not removed"
        );
        queue.remove(2);
        assert_eq!(ids(&queue), ["b", "c"]);
        assert_eq!(current_id(&queue), Some("c"));
    }

    #[test]
    fn clear_upcoming_survives_turning_shuffle_off() {
        let mut queue = queue_of(&["a", "b", "c", "d"], 1);
        queue.set_shuffle(true);
        queue.clear_upcoming();
        assert_eq!(ids(&queue), ["a", "b"]);
        queue.set_shuffle(false);
        assert_eq!(ids(&queue), ["a", "b"]);
        assert_eq!(current_id(&queue), Some("b"));
    }

    #[test]
    fn restore_keeps_the_saved_order_and_position() {
        let mut queue = Queue::default();
        let tracks: Vec<_> = ["c", "a", "b"].iter().map(|id| track(id)).collect();
        queue.restore(tracks, Some(1), true);
        assert_eq!(ids(&queue), ["c", "a", "b"]);
        assert_eq!(current_id(&queue), Some("a"));
        assert!(queue.shuffle());

        queue.restore(vec![track("a")], Some(5), false);
        assert_eq!(current_id(&queue), None);
        assert!(!queue.shuffle());
    }

    #[test]
    fn replace_while_shuffled_keeps_the_chosen_track_current() {
        let mut queue = Queue::default();
        queue.set_shuffle(true);
        queue.replace(["a", "b", "c", "d"].iter().map(|id| track(id)).collect(), 1);
        assert_eq!(current_id(&queue), Some("b"));
        queue.set_shuffle(false);
        assert_eq!(ids(&queue), ["a", "b", "c", "d"]);
    }
}
