//! The hand-off between the decoder and the audio callback.
//!
//! The decoder pushes blocks of interleaved samples; the callback takes them out. The queue
//! never hands more than a couple of seconds of audio to the decoder, so a slow network cannot
//! fill the memory.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

/// A chunk of audio, interleaved, at the device's rate and channel count.
#[derive(Debug)]
pub struct Block {
    /// Interleaved samples.
    pub samples: Vec<f32>,
    /// How many frames (samples / channels) this block holds.
    pub frames: usize,
    /// How many channels each frame has.
    pub channels: usize,
}

/// A FIFO of decoded audio between the decoder thread and the device callback.
#[derive(Debug)]
pub struct Queue {
    inner: Mutex<Inner>,
    /// Signalled when the callback frees room.
    space: Condvar,
}

#[derive(Debug)]
struct Inner {
    blocks: VecDeque<Block>,
    /// Frames waiting in the queue.
    frames: usize,
    /// The most frames we keep around.
    capacity: usize,
    /// Bumped on every reset; blocks from an older epoch are dropped.
    epoch: u64,
    /// Whether the decoder is done producing.
    finished: bool,
}

impl Queue {
    /// A queue that holds at most `capacity_frames` frames.
    pub fn new(capacity_frames: usize) -> Self {
        Queue {
            inner: Mutex::new(Inner {
                blocks: VecDeque::new(),
                frames: 0,
                capacity: capacity_frames.max(1),
                epoch: 0,
                finished: false,
            }),
            space: Condvar::new(),
        }
    }

    /// Throws away whatever is queued and starts a new epoch. Returns the new epoch.
    pub fn reset(&self) -> u64 {
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        inner.epoch = inner.epoch.wrapping_add(1);
        inner.blocks.clear();
        inner.frames = 0;
        inner.finished = false;
        self.space.notify_all();
        inner.epoch
    }

    /// Pushes a block, waiting for room if the queue is full.
    ///
    /// Returns `false` when the block belongs to an older epoch and was thrown away.
    pub fn push(&self, epoch: u64, block: Block) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        while inner.epoch == epoch
            && inner.frames + block.frames > inner.capacity
            && !inner.finished
        {
            inner = self.space.wait(inner).unwrap_or_else(|err| err.into_inner());
        }
        if inner.epoch != epoch {
            return false;
        }
        inner.frames += block.frames;
        inner.blocks.push_back(block);
        true
    }

    /// Takes up to `max_frames` frames out of the queue.
    ///
    /// A block that does not fit is split, so no audio is ever thrown away.
    /// Returns the interleaved samples and how many frames they are.
    pub fn take(&self, max_frames: usize) -> (Vec<f32>, usize) {
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        let mut samples = Vec::new();
        let mut frames = 0;
        while frames < max_frames {
            let room = max_frames - frames;
            let Some(block) = inner.blocks.front_mut() else { break };
            if block.frames > room {
                // The block does not fit: take what fits and keep the rest.
                let wanted = room * block.channels;
                samples.extend_from_slice(&block.samples[..wanted]);
                block.samples.drain(..wanted);
                block.frames -= room;
                frames += room;
                break;
            }
            let block = inner.blocks.pop_front().expect("the block was there");
            frames += block.frames;
            samples.extend_from_slice(&block.samples);
        }
        inner.frames = inner.frames.saturating_sub(frames);
        if frames > 0 {
            self.space.notify_all();
        }
        (samples, frames)
    }

    /// Whether the decoder is done and everything it produced has been played.
    pub fn is_drained(&self) -> bool {
        let inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        inner.finished && inner.blocks.is_empty()
    }

    /// Says the decoder of `epoch` will not produce anything else. A decoder from an older epoch
    /// (a track that was replaced) is ignored, so it cannot end the new track.
    pub fn finish(&self, epoch: u64) {
        let mut inner = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        if inner.epoch != epoch {
            return;
        }
        inner.finished = true;
        self.space.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(frames: usize) -> Block {
        Block { samples: vec![0.5; frames * 2], frames, channels: 2 }
    }

    #[test]
    fn blocks_come_back_in_order() {
        let queue = Queue::new(1000);
        let epoch = queue.reset();
        queue.push(epoch, block(10));
        queue.push(epoch, block(20));
        let (samples, frames) = queue.take(100);
        assert_eq!(frames, 30);
        assert_eq!(samples.len(), 60);
    }

    #[test]
    fn take_respects_the_limit_and_keeps_the_rest() {
        let queue = Queue::new(1000);
        let epoch = queue.reset();
        queue.push(epoch, block(10));
        queue.push(epoch, block(20));
        let (samples, frames) = queue.take(15);
        assert_eq!(frames, 15);
        assert_eq!(samples.len(), 30);
        // The rest is still there.
        let (_, frames) = queue.take(100);
        assert_eq!(frames, 15);
    }

    #[test]
    fn an_old_epoch_is_dropped() {
        let queue = Queue::new(1000);
        let old = queue.reset();
        let new = queue.reset();
        assert!(!queue.push(old, block(10)));
        assert!(queue.push(new, block(10)));
        assert_eq!(queue.take(100).1, 10);
    }

    #[test]
    fn reset_throws_the_queue_away() {
        let queue = Queue::new(1000);
        let epoch = queue.reset();
        queue.push(epoch, block(10));
        queue.reset();
        assert_eq!(queue.take(100).1, 0);
    }

    #[test]
    fn drained_needs_both_the_end_and_an_empty_queue() {
        let queue = Queue::new(1000);
        let epoch = queue.reset();
        queue.push(epoch, block(10));
        queue.finish(epoch);
        assert!(!queue.is_drained());
        queue.take(100);
        assert!(queue.is_drained());
    }

    #[test]
    fn an_old_decoder_cannot_end_the_new_track() {
        let queue = Queue::new(100);
        let old = queue.reset();
        let new = queue.reset();
        queue.finish(old);
        assert!(!queue.is_drained());
        queue.finish(new);
        assert!(queue.is_drained());
    }

    #[test]
    fn the_queue_does_not_grow_without_bound() {
        let queue = std::sync::Arc::new(Queue::new(100));
        let epoch = queue.reset();
        // Pushing more than the capacity would block forever, so use a thread.
        let producer = {
            let queue = std::sync::Arc::clone(&queue);
            std::thread::spawn(move || {
                for _ in 0..10 {
                    queue.push(epoch, block(40));
                }
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!producer.is_finished(), "the producer should be waiting for room");
        while !producer.is_finished() {
            queue.take(1000);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        producer.join().unwrap();
    }
}
