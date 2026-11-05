//! Non-macOS fallback. The worker owns and reaps the only ffplay child.
use super::PlaybackStatus;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

enum Control {
    Start(String, f32),
    Stop,
}
pub(super) struct Player {
    controls: Option<Sender<Control>>,
    updates: Option<Receiver<PlaybackStatus>>,
    current: Option<(String, f32)>,
    status: PlaybackStatus,
}
impl Default for Player {
    fn default() -> Self {
        Self {
            controls: None,
            updates: None,
            current: None,
            status: PlaybackStatus::Stopped,
        }
    }
}
impl Player {
    pub fn play(&mut self, url: &str, volume: f32) -> Result<(), String> {
        if self.controls.is_none() {
            let (tx, rx) = mpsc::channel();
            let (updates, results) = mpsc::channel();
            std::thread::spawn(move || {
                let mut child: Option<Child> = None;
                loop {
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(control) => {
                            if let Some(mut p) = child.take() {
                                let _ = p.kill();
                                let _ = p.wait();
                            }
                            if let Control::Start(url, volume) = control {
                                match Command::new("ffplay")
                                    .args([
                                        "-nodisp",
                                        "-autoexit",
                                        "-loglevel",
                                        "quiet",
                                        "-protocol_whitelist",
                                        "http,https,tcp,tls,crypto",
                                        "-rw_timeout",
                                        "15000000",
                                        "-volume",
                                        &format!("{:.0}", volume * 100.0),
                                        "-i",
                                        &url,
                                    ])
                                    .stdin(Stdio::null())
                                    .stdout(Stdio::null())
                                    .stderr(Stdio::null())
                                    .spawn()
                                {
                                    Ok(p) => {
                                        child = Some(p);
                                        let _ = updates.send(PlaybackStatus::Connecting);
                                    }
                                    Err(_) => {
                                        let _ = updates.send(PlaybackStatus::Failed("Install ffplay (FFmpeg) to play radio on this platform.".into()));
                                    }
                                }
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if let Some(p) = &mut child
                        && let Ok(Some(_)) = p.try_wait()
                    {
                        child = None;
                        let _ = updates.send(PlaybackStatus::Failed(
                            "The radio stream ended or could not be played.".into(),
                        ));
                    }
                }
                if let Some(mut p) = child {
                    let _ = p.kill();
                    let _ = p.wait();
                }
            });
            self.controls = Some(tx);
            self.updates = Some(results);
        }
        self.current = Some((url.into(), volume));
        self.status = PlaybackStatus::Connecting;
        let _ = self
            .controls
            .as_ref()
            .unwrap()
            .send(Control::Start(url.into(), volume));
        Ok(())
    }
    pub fn status(&mut self) -> PlaybackStatus {
        if let Some(rx) = &self.updates {
            for status in rx.try_iter() {
                self.status = status;
            }
        }
        self.status.clone()
    }
    pub fn pause(&mut self) {
        if let Some(tx) = &self.controls {
            let _ = tx.send(Control::Stop);
        }
        self.status = PlaybackStatus::Paused;
    }
    pub fn resume(&mut self) {
        if let Some((url, volume)) = self.current.clone() {
            let _ = self.play(&url, volume);
        }
    }
    pub fn volume(&mut self, volume: f32) {
        if let Some((url, _)) = self.current.clone() {
            if self.status != PlaybackStatus::Paused {
                let _ = self.play(&url, volume);
            } else {
                self.current = Some((url, volume));
            }
        }
    }
    pub fn stop(&mut self) {
        self.controls = None;
        self.updates = None;
        self.current = None;
        self.status = PlaybackStatus::Stopped;
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}
