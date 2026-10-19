use std::io::Cursor;
use std::time::Duration;

use anyhow::Result;
use rodio::{Decoder, MixerDeviceSink, Player};

/// Plays one track at a time on the default output device.
///
/// Not `Send` on every platform: keep it on the thread that created it.
pub struct AudioPlayer {
    // Dropping the device sink stops all sound, so it lives as long as the player.
    _sink: MixerDeviceSink,
    player: Player,
}

impl AudioPlayer {
    pub fn new() -> Result<Self> {
        let mut sink = rodio::DeviceSinkBuilder::open_default_sink()?;
        sink.log_on_drop(false);
        let player = Player::connect_new(sink.mixer());
        Ok(AudioPlayer {
            _sink: sink,
            player,
        })
    }

    /// Replaces whatever is playing with an M4A/AAC track held in memory.
    pub fn play(&self, audio: Vec<u8>) -> Result<()> {
        let byte_len = audio.len() as u64;
        let source = Decoder::builder()
            .with_data(Cursor::new(audio))
            .with_byte_len(byte_len)
            .with_seekable(true)
            .with_mime_type("audio/mp4")
            .build()?;
        self.player.clear();
        self.player.append(source);
        self.player.play();
        Ok(())
    }

    /// Silences the current track, e.g. while the next one loads.
    pub fn stop(&self) {
        self.player.clear();
    }

    pub fn toggle_pause(&self) {
        if self.player.is_paused() {
            self.player.play();
        } else {
            self.player.pause();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.player.is_paused()
    }

    pub fn is_idle(&self) -> bool {
        self.player.empty()
    }

    pub fn position(&self) -> Duration {
        self.player.get_pos()
    }

    pub fn seek(&self, position: Duration) {
        if let Err(err) = self.player.try_seek(position) {
            log::warn!("seek failed: {err}");
        }
    }

    pub fn set_volume(&self, volume: f32) {
        self.player.set_volume(volume);
    }
}
