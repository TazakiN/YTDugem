use std::{sync::mpsc, time::Duration};

use souvlaki::{
    MediaControlEvent as OsEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition,
    PlatformConfig, SeekDirection,
};

use crate::{model::Track, player::PlayerStatus};

#[derive(Clone, Debug, PartialEq)]
pub enum MediaEvent {
    Toggle,
    Play,
    Pause,
    Next,
    Previous,
    Seek(f64),
    SeekBy(f64),
    Volume(f64),
    Stop,
}

pub struct Integrations {
    controls: Option<MediaControls>,
    events: mpsc::Receiver<MediaEvent>,
    last: Option<Snapshot>,
}

#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    title: String,
    artist: String,
    album: String,
    artwork: String,
    duration: f64,
    position: f64,
    paused: bool,
    stopped: bool,
}

impl Integrations {
    pub fn new() -> Self {
        Self::for_window(None)
    }

    pub fn for_window(hwnd: Option<*mut std::ffi::c_void>) -> Self {
        let (tx, events) = mpsc::channel();
        let config = PlatformConfig {
            dbus_name: "ytdugem",
            display_name: "YTDugem",
            hwnd,
        };
        let has_session_bus =
            !cfg!(target_os = "linux") || std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some();
        let controls = has_session_bus
            .then(|| MediaControls::new(config).ok())
            .flatten()
            .and_then(|mut controls| {
                controls
                    .attach(move |event| {
                        if let Some(event) = map_event(event) {
                            let _ = tx.send(event);
                        }
                    })
                    .ok()?;
                Some(controls)
            });
        Self {
            controls,
            events,
            last: None,
        }
    }

    pub fn available(&self) -> bool {
        self.controls.is_some()
    }

    pub fn poll(&mut self) -> Vec<MediaEvent> {
        self.events.try_iter().collect()
    }

    pub fn update(&mut self, track: Option<&Track>, status: &PlayerStatus) {
        let snapshot = Snapshot {
            title: track.map_or_else(String::new, |t| t.title.clone()),
            artist: track.map_or_else(String::new, |t| t.artist.clone()),
            album: track.map_or_else(String::new, |t| t.album.clone()),
            artwork: track.map_or_else(String::new, |t| t.thumbnail.clone()),
            duration: status.duration.max(track.map_or(0.0, |t| t.duration)),
            position: status.position.floor(),
            paused: status.paused,
            stopped: track.is_none() || status.ended || status.errored,
        };
        let Some(controls) = self.controls.as_mut() else {
            return;
        };
        if self.last.as_ref() == Some(&snapshot) {
            return;
        }
        if self
            .last
            .as_ref()
            .map(|s| (&s.title, &s.artist, &s.album, &s.artwork, s.duration))
            != Some((
                &snapshot.title,
                &snapshot.artist,
                &snapshot.album,
                &snapshot.artwork,
                snapshot.duration,
            ))
        {
            let metadata = metadata(&snapshot);
            let _ = controls.set_metadata(metadata);
        }
        // Playback progress is cheap and must stay current for MPRIS position seeks.
        let progress = valid_duration(snapshot.position).map(MediaPosition);
        let playback = if snapshot.stopped {
            MediaPlayback::Stopped
        } else if snapshot.paused {
            MediaPlayback::Paused { progress }
        } else {
            MediaPlayback::Playing { progress }
        };
        let _ = controls.set_playback(playback);
        self.last = Some(snapshot);
    }
}

fn valid_duration(seconds: f64) -> Option<Duration> {
    if seconds.is_finite() && seconds >= 0.0 {
        Duration::try_from_secs_f64(seconds).ok()
    } else {
        None
    }
}

fn metadata(s: &Snapshot) -> MediaMetadata<'_> {
    MediaMetadata {
        title: nonempty(&s.title),
        artist: nonempty(&s.artist),
        album: nonempty(&s.album),
        cover_url: nonempty(&s.artwork),
        duration: valid_duration(s.duration),
    }
}

fn nonempty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn map_event(event: OsEvent) -> Option<MediaEvent> {
    match event {
        OsEvent::Toggle => Some(MediaEvent::Toggle),
        OsEvent::Play => Some(MediaEvent::Play),
        OsEvent::Pause => Some(MediaEvent::Pause),
        OsEvent::Next => Some(MediaEvent::Next),
        OsEvent::Previous => Some(MediaEvent::Previous),
        OsEvent::Stop => Some(MediaEvent::Stop),
        OsEvent::SetPosition(MediaPosition(position)) => {
            Some(MediaEvent::Seek(position.as_secs_f64()))
        }
        OsEvent::SetVolume(volume) => Some(MediaEvent::Volume(volume)),
        OsEvent::Seek(direction) => Some(MediaEvent::SeekBy(match direction {
            SeekDirection::Forward => 10.0,
            SeekDirection::Backward => -10.0,
        })),
        OsEvent::SeekBy(direction, amount) => Some(MediaEvent::SeekBy(match direction {
            SeekDirection::Forward => amount.as_secs_f64(),
            SeekDirection::Backward => -amount.as_secs_f64(),
        })),
        _ => None,
    }
}

impl Default for Integrations {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_events_and_metadata() {
        assert_eq!(
            map_event(OsEvent::SeekBy(
                SeekDirection::Backward,
                Duration::from_secs(4)
            )),
            Some(MediaEvent::SeekBy(-4.0))
        );
        assert_eq!(
            map_event(OsEvent::SetPosition(MediaPosition(Duration::from_secs(12)))),
            Some(MediaEvent::Seek(12.0))
        );
        let track = Track {
            title: "Song".into(),
            artist: "Artist".into(),
            duration: 42.0,
            ..Track::default()
        };
        let snapshot = Snapshot {
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: String::new(),
            artwork: String::new(),
            duration: track.duration,
            position: 0.0,
            paused: false,
            stopped: false,
        };
        let m = metadata(&snapshot);
        assert_eq!(m.title, Some("Song"));
        assert_eq!(m.artist, Some("Artist"));
        assert_eq!(m.duration, Some(Duration::from_secs(42)));
        assert!(valid_duration(f64::NAN).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "requires a D-Bus session and gdbus; run under dbus-run-session"]
    fn live_mpris() {
        use std::{process::Command, thread, time::Instant};
        let mut integrations = Integrations::new();
        assert!(integrations.available());
        let track = Track {
            title: "YTDugem MPRIS test".into(),
            artist: "Test Artist".into(),
            duration: 120.0,
            ..Default::default()
        };
        integrations.update(
            Some(&track),
            &PlayerStatus {
                position: 12.0,
                ..Default::default()
            },
        );
        let base = [
            "call",
            "--session",
            "--dest",
            "org.mpris.MediaPlayer2.ytdugem",
            "--object-path",
            "/org/mpris/MediaPlayer2",
            "--method",
        ];
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let out = Command::new("gdbus")
                .args(base)
                .args([
                    "org.freedesktop.DBus.Properties.Get",
                    "org.mpris.MediaPlayer2.Player",
                    "Metadata",
                ])
                .output()
                .unwrap();
            if out.status.success()
                && String::from_utf8_lossy(&out.stdout).contains("YTDugem MPRIS test")
            {
                break;
            }
            assert!(
                Instant::now() < until,
                "MPRIS metadata not exposed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            thread::sleep(Duration::from_millis(25));
        }
        let out = Command::new("gdbus")
            .args(base)
            .arg("org.mpris.MediaPlayer2.Player.PlayPause")
            .output()
            .unwrap();
        assert!(out.status.success());
        loop {
            if integrations.poll().contains(&MediaEvent::Toggle) {
                break;
            }
            assert!(Instant::now() < until, "PlayPause did not reach the app");
            thread::sleep(Duration::from_millis(25));
        }
        println!("MPRIS metadata and desktop PlayPause event roundtrip passed");
    }
}
