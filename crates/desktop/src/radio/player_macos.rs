use super::PlaybackStatus;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_av_foundation::{
    AVPlayer, AVPlayerItemStatus, AVPlayerStatus, AVPlayerTimeControlStatus,
};
use objc2_foundation::{NSString, NSURL};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Player {
    inner: Option<Retained<AVPlayer>>,
    started: Option<Instant>,
    paused: bool,
}

impl Player {
    pub fn play(&mut self, url: &str, volume: f32) -> Result<(), String> {
        self.stop();
        let mtm =
            MainThreadMarker::new().ok_or("Radio playback must start on the app's main thread.")?;
        let url =
            NSURL::URLWithString(&NSString::from_str(url)).ok_or("Invalid radio stream URL.")?;
        // SAFETY: AVPlayer is created and accessed only on the eframe main thread;
        // the URL is retained by the player and its asynchronous item loader.
        let player = unsafe { AVPlayer::playerWithURL(&url, mtm) };
        unsafe {
            player.setVolume(volume.clamp(0.0, 1.0));
            player.play();
        }
        self.inner = Some(player);
        self.started = Some(Instant::now());
        self.paused = false;
        Ok(())
    }
    pub fn status(&mut self) -> PlaybackStatus {
        let Some(player) = &self.inner else {
            return PlaybackStatus::Stopped;
        };
        // SAFETY: All methods are invoked on the same main thread as creation.
        unsafe {
            if player.status() == AVPlayerStatus::Failed {
                return PlaybackStatus::Failed(
                    player
                        .error()
                        .map(|e| e.localizedDescription().to_string())
                        .unwrap_or_else(|| "The radio stream could not be played.".into()),
                );
            }
            if let Some(item) = player.currentItem()
                && item.status() == AVPlayerItemStatus::Failed
            {
                return PlaybackStatus::Failed(
                    item.error()
                        .map(|e| e.localizedDescription().to_string())
                        .unwrap_or_else(|| {
                            "The radio feed is offline or uses an unsupported format.".into()
                        }),
                );
            }
            if self.paused {
                return PlaybackStatus::Paused;
            }
            if player.timeControlStatus() == AVPlayerTimeControlStatus::Playing {
                self.started = None;
                return PlaybackStatus::Playing;
            }
        }
        let started = self.started.get_or_insert_with(Instant::now);
        if started.elapsed() > Duration::from_secs(30) {
            self.stop();
            return PlaybackStatus::Failed(
                "The stream could not buffer within 30 seconds. Try another feed.".into(),
            );
        }
        PlaybackStatus::Connecting
    }
    pub fn pause(&mut self) {
        if let Some(p) = &self.inner {
            unsafe {
                p.pause();
            }
            self.paused = true;
        }
    }
    pub fn resume(&mut self) {
        if let Some(p) = &self.inner {
            unsafe {
                p.play();
            }
            self.paused = false;
            self.started = Some(Instant::now());
        }
    }
    pub fn volume(&mut self, value: f32) {
        if let Some(p) = &self.inner {
            unsafe {
                p.setVolume(value);
            }
        }
    }
    pub fn stop(&mut self) {
        if let Some(p) = self.inner.take() {
            unsafe {
                p.pause();
                p.replaceCurrentItemWithPlayerItem(None);
            }
        }
        self.started = None;
        self.paused = false;
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}
