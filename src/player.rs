use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
#[cfg(windows)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::path::PathBuf;
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerStatus {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub ended: bool,
    pub errored: bool,
    pub audio_sample_rate: u32,
    pub volume: f64,
    pub speed: f64,
}

pub struct Player {
    child: Option<Child>,
    writer: Option<Box<dyn Write + Send>>,
    events: Option<Receiver<std::io::Result<Vec<u8>>>>,
    #[cfg(unix)]
    socket_dir: Option<PathBuf>,
    status: PlayerStatus,
    audio_backend: String,
    cookie_file: Option<crate::auth::CookieFile>,
}

impl Player {
    pub fn new() -> Self {
        Self {
            child: None,
            writer: None,
            events: None,
            #[cfg(unix)]
            socket_dir: None,
            status: PlayerStatus::default(),
            audio_backend: String::new(),
            cookie_file: None,
        }
    }

    pub fn load(&mut self, source: &str, volume: f64, speed: f64) -> Result<()> {
        self.stop();
        if source.trim().is_empty() {
            bail!("audio source is empty");
        }
        if !source.starts_with("https://") && !std::path::Path::new(source).is_file() {
            bail!("Audio source must be an existing local file or HTTPS URL");
        }
        let volume = valid_range(volume, 0.0, 100.0, "volume")?;
        let speed = valid_range(speed, 0.1, 10.0, "speed")?;

        #[cfg(unix)]
        let (ipc_path, socket_dir) = {
            use std::{
                os::unix::fs::PermissionsExt,
                sync::atomic::{AtomicU64, Ordering},
            };
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "bitchord-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&dir)?;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            (dir.join("mpv.sock"), Some(dir))
        };
        #[cfg(windows)]
        let ipc_path = {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            format!(
                r"\\.\pipe\bitchord-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )
        };
        #[cfg(unix)]
        let ipc_arg = ipc_path.to_string_lossy().into_owned();
        #[cfg(windows)]
        let ipc_arg = ipc_path.clone();
        #[cfg(unix)]
        {
            self.socket_dir = socket_dir;
        }

        let mpv = std::env::var_os("BITCHORD_MPV").unwrap_or_else(|| "mpv".into());
        let mut cmd = Command::new(mpv);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000);
        }
        cmd.args([
            "--no-config",
            "--no-terminal",
            "--no-video",
            "--really-quiet",
        ])
        .arg("--network-timeout=20")
        .arg(format!("--input-ipc-server={ipc_arg}"))
        .arg(format!("--volume={volume}"))
        .arg(format!("--speed={speed}"));
        if let Some(ytdlp) = std::env::var_os("BITCHORD_YTDLP") {
            cmd.arg(format!(
                "--script-opt=ytdl_hook-ytdl_path={}",
                ytdlp.to_string_lossy()
            ));
        }
        let cookies = crate::auth::cookie_file()?;
        if let Some(ref file) = cookies {
            cmd.arg(format!(
                "--ytdl-raw-options-append=cookies={}",
                file.path().to_string_lossy()
            ));
        }
        if std::env::var("BITCHORD_AUDIO_OUTPUT").as_deref() == Ok("null") {
            cmd.arg("--ao=null");
        }
        let child = cmd
            .arg("--")
            .arg(source)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context(if cfg!(windows) {
                "Starting mpv: extract the COMPLETE BitChord ZIP, keeping tools/mpv.exe beside BitChord.exe, or set BITCHORD_MPV"
            } else { "starting mpv (install mpv or set BITCHORD_MPV)" })?;
        self.child = Some(child);
        self.cookie_file = cookies;
        // Fresh Windows runtime binaries may be scanned by antivirus before startup.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            #[cfg(unix)]
            let connected = std::os::unix::net::UnixStream::connect(&ipc_path).map(|s| {
                let reader = s.try_clone().expect("clone Unix IPC stream");
                (Box::new(s) as Box<dyn Write + Send>, reader)
            });
            #[cfg(windows)]
            let connected = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&ipc_path)
                .and_then(|reader| {
                    // A blocking read on a duplicated Windows handle can block writes too.
                    // Separate IPC clients keep controls responsive while events are read.
                    loop {
                        match OpenOptions::new().read(true).write(true).open(&ipc_path) {
                            Ok(writer) => {
                                return Ok((Box::new(writer) as Box<dyn Write + Send>, reader))
                            }
                            // Keep the first client connected while mpv creates another pipe instance.
                            Err(_) if Instant::now() < deadline => {
                                thread::sleep(Duration::from_millis(10))
                            }
                            Err(error) => return Err(error),
                        }
                    }
                });
            match connected {
                Ok((writer, mut reader)) => {
                    for (i, property) in [
                        "time-pos",
                        "duration",
                        "pause",
                        "audio-out-params",
                        "current-ao",
                        "volume",
                        "speed",
                    ]
                    .iter()
                    .enumerate()
                    {
                        serde_json::to_writer(
                            &mut reader,
                            &json!({"command":["observe_property",i+1,property]}),
                        )?;
                        reader.write_all(b"\n")?;
                    }
                    let (tx, rx) = mpsc::channel();
                    thread::spawn(move || read_events(reader, tx));
                    self.writer = Some(writer);
                    self.events = Some(rx);
                    return Ok(());
                }
                Err(e) if Instant::now() < deadline => {
                    if let Some(child) = self.child.as_mut() {
                        if child.try_wait()?.is_some() {
                            self.stop();
                            bail!("mpv exited before IPC was available");
                        }
                    }
                    let _ = e;
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    self.stop();
                    bail!("mpv IPC did not become available: {error}");
                }
            }
        }
    }

    pub fn toggle(&mut self) -> Result<()> {
        self.command(json!(["cycle", "pause"]))
    }

    pub fn is_active(&self) -> bool {
        self.child.is_some()
    }
    pub fn audio_backend(&self) -> &str {
        &self.audio_backend
    }
    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        if !seconds.is_finite() || seconds < 0.0 {
            bail!("seek position must be a finite non-negative number");
        }
        self.command(json!(["set_property", "time-pos", seconds]))
    }
    pub fn volume(&mut self, value: f64) -> Result<()> {
        self.command(json!([
            "set_property",
            "volume",
            valid_range(value, 0.0, 100.0, "volume")?
        ]))
    }
    pub fn speed(&mut self, value: f64) -> Result<()> {
        self.command(json!([
            "set_property",
            "speed",
            valid_range(value, 0.1, 10.0, "speed")?
        ]))
    }

    pub fn poll(&mut self) -> Option<PlayerStatus> {
        self.child.as_ref()?;
        loop {
            let event = self.events.as_ref().map(|events| events.try_recv());
            match event {
                Some(Ok(Ok(line))) => {
                    if let Ok(v) = serde_json::from_slice::<Value>(&line) {
                        self.apply_event(&v);
                    }
                }
                Some(Ok(Err(_))) | Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    if !self.status.ended {
                        self.status.errored = true;
                    }
                    self.events = None;
                    break;
                }
                Some(Err(mpsc::TryRecvError::Empty)) | None => break,
            }
        }
        if let Ok(Some(exit)) = self.child.as_mut()?.try_wait() {
            if !self.status.ended && !self.status.errored {
                if exit.success() {
                    self.status.ended = true;
                } else {
                    self.status.errored = true;
                }
            }
            self.child = None;
            self.cookie_file = None;
            self.writer = None;
            self.events = None;
        }
        Some(self.status)
    }

    pub fn stop(&mut self) {
        self.writer = None;
        self.events = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.cookie_file = None;
        #[cfg(unix)]
        if let Some(dir) = self.socket_dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
        self.status = PlayerStatus::default();
        self.audio_backend.clear();
    }

    fn command(&mut self, command: Value) -> Result<()> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| anyhow!("mpv is not playing"))?;
        serde_json::to_writer(&mut **writer, &json!({"command": command}))?;
        writer.write_all(b"\n").context("writing mpv command")
    }

    fn apply_event(&mut self, v: &Value) {
        if v["event"] == "property-change" {
            match v["name"].as_str() {
                Some("time-pos") => {
                    if let Some(n) = v["data"].as_f64() {
                        self.status.position = n;
                    }
                }
                Some("duration") => {
                    if let Some(n) = v["data"].as_f64() {
                        self.status.duration = n;
                    }
                }
                Some("pause") => {
                    if let Some(b) = v["data"].as_bool() {
                        self.status.paused = b;
                    }
                }
                Some("audio-out-params") => {
                    self.status.audio_sample_rate =
                        v["data"]["samplerate"].as_u64().unwrap_or(0) as u32;
                }
                Some("current-ao") => {
                    self.audio_backend = v["data"].as_str().unwrap_or("").to_owned();
                }
                Some("volume") => {
                    if let Some(n) = v["data"].as_f64() {
                        self.status.volume = n;
                    }
                }
                Some("speed") => {
                    if let Some(n) = v["data"].as_f64() {
                        self.status.speed = n;
                    }
                }
                _ => {}
            }
        } else if v["event"] == "end-file" {
            match v["reason"].as_str() {
                Some("eof") => self.status.ended = true,
                Some("error") => self.status.errored = true,
                _ => {}
            }
        }
    }
}

fn read_events(reader: impl Read, tx: mpsc::Sender<std::io::Result<Vec<u8>>>) {
    for line in BufReader::new(reader).split(b'\n') {
        match line {
            Ok(line) => {
                if tx.send(Ok(line)).is_err() {
                    return;
                }
            }
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

fn valid_range(n: f64, min: f64, max: f64, name: &str) -> Result<f64> {
    if !n.is_finite() || !(min..=max).contains(&n) {
        bail!("{name} must be between {min} and {max}");
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn protocol_eof_and_validation() {
        let (tx, rx) = mpsc::channel();
        read_events(
            Cursor::new(b"{\"event\":\"end-file\",\"reason\":\"eof\"}\n"),
            tx,
        );
        assert_eq!(
            rx.recv().unwrap().unwrap(),
            br#"{"event":"end-file","reason":"eof"}"#
        );
        assert!(rx.recv().is_err());
        let mut p = Player::new();
        p.apply_event(&json!({"event":"property-change","name":"current-ao","data":"wasapi"}));
        p.apply_event(&json!({"event":"property-change","name":"audio-out-params","data":{"samplerate":48000}}));
        p.apply_event(&json!({"event":"property-change","name":"volume","data":25.0}));
        p.apply_event(&json!({"event":"property-change","name":"speed","data":1.25}));
        assert_eq!(p.audio_backend(), "wasapi");
        assert_eq!(p.status.audio_sample_rate, 48000);
        assert_eq!(p.status.volume, 25.0);
        assert_eq!(p.status.speed, 1.25);
        p.apply_event(&json!({"event":"end-file","reason":"eof"}));
        assert!(p.status.ended);
        p.apply_event(&json!({"event":"end-file","reason":"error"}));
        assert!(p.status.errored);
        assert!(valid_range(1.0, 0.1, 10.0, "speed").is_ok());
        assert!(valid_range(f64::NAN, 0.1, 10.0, "speed").is_err());
        assert!(Player::new().seek(-1.0).is_err());
    }

    #[test]
    #[ignore = "requires mpv; run with BITCHORD_AUDIO_OUTPUT=null"]
    fn plays_local_audio() {
        use std::{fs::File, io::Write};
        let path = std::env::temp_dir().join(format!("bitchord-test-{}.wav", std::process::id()));
        let mut f = File::create(&path).unwrap();
        let samples = 44_100u32 * 4;
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36 + samples * 2).to_le_bytes()).unwrap();
        f.write_all(b"WAVEfmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&44_100u32.to_le_bytes()).unwrap();
        f.write_all(&88_200u32.to_le_bytes()).unwrap();
        f.write_all(&2u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&(samples * 2).to_le_bytes()).unwrap();
        for _ in 0..samples {
            f.write_all(&0i16.to_le_bytes()).unwrap();
        }
        drop(f);
        std::env::set_var("BITCHORD_AUDIO_OUTPUT", "null");
        let mut p = Player::new();
        p.load(path.to_str().unwrap(), 50.0, 1.0).unwrap();
        fn wait_for(p: &mut Player, condition: impl Fn(PlayerStatus) -> bool) -> PlayerStatus {
            let until = Instant::now() + Duration::from_secs(4);
            loop {
                let s = p.poll().expect("mpv quit before expected status");
                assert!(!s.errored, "mpv could not decode test audio");
                if condition(s) {
                    return s;
                }
                assert!(
                    Instant::now() < until,
                    "timed out waiting for status: {s:?}"
                );
                thread::sleep(Duration::from_millis(25));
            }
        }
        let s = wait_for(&mut p, |s| s.duration > 3.9 && s.position > 0.05);
        println!(
            "Decoded WAV: duration={:.2}s position={:.2}s",
            s.duration, s.position
        );
        p.toggle().unwrap();
        assert!(wait_for(&mut p, |s| s.paused).paused);
        p.seek(1.5).unwrap();
        assert!(wait_for(&mut p, |s| s.position >= 1.4).paused);
        p.volume(30.0).unwrap();
        p.speed(1.2).unwrap();
        p.toggle().unwrap();
        p.seek(3.8).unwrap();
        assert!(wait_for(&mut p, |s| s.ended).ended);
        println!("Pause, seek, volume, speed, resume and natural EOF passed");
        p.stop();
        assert!(p.poll().is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[ignore = "requires mpv, yt-dlp and internet access to YouTube"]
    fn plays_youtube_audio() {
        std::env::set_var("BITCHORD_AUDIO_OUTPUT", "null");
        let mut p = Player::new();
        p.load("https://www.youtube.com/watch?v=5NV6Rdv1a3I", 50.0, 1.0)
            .unwrap();
        let until = Instant::now() + Duration::from_secs(45);
        loop {
            let status = p.poll().expect("mpv exited without streaming audio");
            assert!(!status.errored, "YouTube stream could not be opened");
            if status.position > 0.1 && status.duration > 60.0 {
                println!(
                    "YouTube audio decoded: position={:.2}s duration={:.2}s",
                    status.position, status.duration
                );
                break;
            }
            assert!(
                Instant::now() < until,
                "YouTube stream did not start: {status:?}"
            );
            thread::sleep(Duration::from_millis(100));
        }
        p.stop();
    }
}
