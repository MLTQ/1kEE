//! Narrow UI-owned playback interface; AVFoundation owns asynchronous media I/O.
use super::stream_url;
#[cfg(target_os = "macos")]
#[path = "player_macos.rs"]
mod backend;
#[cfg(not(target_os = "macos"))]
#[path = "player_ffplay.rs"]
mod backend;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PlaybackStatus {
    Stopped,
    Connecting,
    Playing,
    Paused,
    Failed(String),
}

impl PlaybackStatus {
    pub fn label(&self) -> &str {
        match self {
            Self::Stopped => "Stopped",
            Self::Connecting => "Connecting…",
            Self::Playing => "Playing",
            Self::Paused => "Paused",
            Self::Failed(_) => "Unavailable",
        }
    }
}

#[derive(Default)]
pub(crate) struct RadioPlayer {
    backend: backend::Player,
    pub status: Option<PlaybackStatus>,
}

impl RadioPlayer {
    pub fn play(&mut self, url: &str, volume: f32) {
        self.stop();
        self.status = Some(
            match stream_url(url).and_then(|url| self.backend.play(url.as_str(), volume)) {
                Ok(()) => PlaybackStatus::Connecting,
                Err(e) => PlaybackStatus::Failed(e),
            },
        );
    }
    pub fn tick(&mut self) {
        if self
            .status
            .as_ref()
            .is_some_and(|s| !matches!(s, PlaybackStatus::Failed(_) | PlaybackStatus::Stopped))
        {
            self.status = Some(self.backend.status());
            if matches!(self.status, Some(PlaybackStatus::Failed(_))) {
                self.backend.stop();
            }
        }
    }
    pub fn pause(&mut self) {
        self.backend.pause();
        self.status = Some(PlaybackStatus::Paused);
    }
    pub fn resume(&mut self) {
        self.backend.resume();
        self.status = Some(PlaybackStatus::Connecting);
    }
    pub fn volume(&mut self, value: f32) {
        self.backend.volume(value.clamp(0.0, 1.0));
    }
    pub fn stop(&mut self) {
        self.backend.stop();
        self.status = None;
    }
}

impl Drop for RadioPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}
