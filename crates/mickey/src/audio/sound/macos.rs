//! macOS audio playback and capture (placeholder until CoreAudio is added).

use crate::audio::wire::PCM_BYTES;

pub struct Recorder;

impl Recorder {
    pub fn open(_diverted: bool) -> Result<Recorder, String> {
        Err("shared audio capture on macOS is not implemented yet".into())
    }

    pub fn read(&mut self) -> Result<Option<[u8; PCM_BYTES]>, String> {
        Err("shared audio capture on macOS is not implemented yet".into())
    }
}

pub struct Player;

impl Player {
    pub fn open(_pc: &str) -> Result<Player, String> {
        Err("shared audio playback on macOS is not implemented yet".into())
    }

    pub fn write(&mut self, _pcm: &[u8; PCM_BYTES], _volume: f32) -> Result<(), String> {
        Err("shared audio playback on macOS is not implemented yet".into())
    }
}
