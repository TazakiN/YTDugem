use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
#[cfg(windows)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::path::PathBuf;
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};

const EQ_HZ: [u32; 7] = [60, 150, 400, 1000, 2500, 6000, 14000];
const MAX_IPC_FRAME: usize = 1024 * 1024;

/// A directly playable YouTube stream and the HTTP headers yt-dlp says it needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    url: String,
    headers: Vec<(String, String)>,
}

/// How long a resolved stream is reused (Meiro's `urlTTL`). YouTube's media URLs stay valid for
/// hours; half an hour keeps replays, Previous and re-picks instant without leaning on that.
const STREAM_TTL: Duration = Duration::from_secs(30 * 60);

/// Resolved streams by video ID, so playing a track again needs no request at all.
static STREAMS: std::sync::Mutex<Vec<(String, Stream, Instant)>> =
    std::sync::Mutex::new(Vec::new());

/// The fast-path stream for a YouTube source; `None` for anything else or when it cannot be
/// resolved (playback then falls back to mpv's signed-in yt-dlp hook). Blocks; call off the GUI.
///
/// In order, as Meiro resolves audio: a stream resolved in the last half hour; one InnerTube
/// `player` request as the visionOS app (~0.2–0.4 s); yt-dlp with that same client (~3 s, it
/// knows more ways to ask).
pub fn resolve_stream(source: &str) -> Option<Stream> {
    let id = video_id(source)?;
    if let Some(stream) = cached_stream(&id) {
        return Some(stream);
    }
    let stream = match crate::services::audio_stream(&id) {
        Ok((url, _)) => Some(Stream {
            url,
            headers: vec![(
                "User-Agent".into(),
                crate::services::BROWSER_USER_AGENT.into(),
            )],
        }),
        Err(_) => resolve_youtube(source),
    }?;
    if let Ok(mut cache) = STREAMS.lock() {
        cache.retain(|(key, _, at)| *key != id && at.elapsed() < STREAM_TTL);
        cache.push((id, stream.clone(), Instant::now()));
    }
    Some(stream)
}

/// Drop a cached stream that turned out not to play, so the next try resolves it afresh.
pub fn forget_stream(source: &str) {
    if let (Some(id), Ok(mut cache)) = (video_id(source), STREAMS.lock()) {
        cache.retain(|(key, _, _)| *key != id);
    }
}

fn cached_stream(id: &str) -> Option<Stream> {
    let cache = STREAMS.lock().ok()?;
    cache
        .iter()
        .find(|(key, _, at)| key == id && at.elapsed() < STREAM_TTL)
        .map(|(_, stream, _)| stream.clone())
}

/// The video ID of a YouTube Music or YouTube watch URL.
fn video_id(source: &str) -> Option<String> {
    let rest = source
        .strip_prefix("https://music.youtube.com/watch?")
        .or_else(|| source.strip_prefix("https://www.youtube.com/watch?"))?;
    rest.split('&')
        .find_map(|pair| pair.strip_prefix("v="))
        .filter(|id| id.len() == 11)
        .map(str::to_owned)
}

/// Fallback: yt-dlp's cookie-less `visionos` extraction. Letting mpv's yt-dlp hook extract with
/// the account cookies takes ~11 s to first audio: yt-dlp drops its quickest client (`visionos`
/// accepts no cookies), the remaining clients are slower, often only offer a 360p video
/// (format 18), and YouTube then refuses the stream until `available_at`. Anything this cannot
/// play (age-restricted, members-only, private) returns `None`, and the caller falls back to the
/// signed-in hook.
fn resolve_youtube(source: &str) -> Option<Stream> {
    let ytdlp = std::env::var_os("BITCHORD_YTDLP").unwrap_or_else(|| "yt-dlp".into());
    let mut cmd = Command::new(ytdlp);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd
        .args([
            "-J",
            "--no-playlist",
            "--no-warnings",
            "-f",
            "bestaudio/best",
            "--extractor-args",
            "youtube:player_client=visionos",
            "--",
            source,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut body = Vec::new();
        stdout
            .by_ref()
            .take(16 * 1024 * 1024)
            .read_to_end(&mut body)
            .map(|_| body)
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            _ => return None,
        }
    }
    let info: Value = serde_json::from_slice(&reader.join().ok()?.ok()?).ok()?;
    let (stream, available_at) = parse_stream(&info)?;
    // YouTube answers 403 until `available_at`; this client rarely needs it, but honour it.
    if let Some(at) = available_at {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        if at > now {
            thread::sleep(Duration::from_secs((at - now).min(10)));
        }
    }
    Some(stream)
}

fn parse_stream(info: &Value) -> Option<(Stream, Option<u64>)> {
    let url = info["url"].as_str().filter(|u| u.starts_with("https://"))?;
    let headers = info["http_headers"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, value)| {
            let value = value.as_str()?;
            // One header per mpv list item; a newline would smuggle in another header.
            (!name.contains(['\r', '\n', ':']) && !value.contains(['\r', '\n']))
                .then(|| (name.clone(), value.to_owned()))
        })
        .collect();
    Some((
        Stream {
            url: url.to_owned(),
            headers,
        },
        info["available_at"].as_u64(),
    ))
}

pub fn audio_filter(settings: &crate::store::AudioSettings) -> Option<String> {
    let mut settings = settings.clone();
    settings.sanitize();
    let mut filters = Vec::new();
    if settings.equalizer_enabled {
        for (hz, gain) in EQ_HZ.into_iter().zip(settings.equalizer_bands) {
            if gain == 0.0 {
                continue;
            }
            // Match upstream: 60/14k shelves, five intermediate bell filters.
            let filter = match hz {
                60 => format!("bass=f={hz}:g={gain}:w=1"),
                14000 => format!("treble=f={hz}:g={gain}:w=1"),
                _ => format!("equalizer=f={hz}:t=q:w=1:g={gain}"),
            };
            filters.push(filter);
        }
    }
    if settings.stereo_balance != 0.0 {
        filters.push(format!(
            "stereotools=balance_out={}",
            settings.stereo_balance
        ));
    }
    (!filters.is_empty()).then(|| format!("lavfi=[{}]", filters.join(",")))
}

/// Return only devices mpv itself advertises; no guessed Windows endpoint labels.
pub fn audio_devices() -> Result<Vec<(String, String)>> {
    let executable = std::env::var_os("BITCHORD_MPV").unwrap_or_else(|| "mpv".into());
    let mut command = Command::new(executable);
    command.args(["--no-config", "--audio-device=help"]);
    let output = crate::services::run(&mut command, Duration::from_secs(10))
        .context("Enumerating audio devices with mpv")?;
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    let devices: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let start = line.find('\'')? + 1;
            let end = line[start..].find('\'')? + start;
            let id = line[start..end].trim();
            if id.is_empty() || id == "auto" {
                return None;
            }
            let label = line[end + 1..]
                .trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .trim();
            Some((
                id.to_owned(),
                if label.is_empty() {
                    id.to_owned()
                } else {
                    label.to_owned()
                },
            ))
        })
        .collect();
    if !output.status.success() && devices.is_empty() {
        bail!("mpv did not provide audio devices: {}", text.trim());
    }
    Ok(devices)
}

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
    pub audio_filter_count: usize,
    pub pending_commands: usize,
}

pub struct Player {
    child: Option<Child>,
    commands: Option<SyncSender<(u64, Value)>>,
    events: Option<Receiver<std::io::Result<Vec<u8>>>>,
    #[cfg(unix)]
    socket_dir: Option<PathBuf>,
    status: PlayerStatus,
    audio_backend: String,
    cookie_file: Option<crate::auth::CookieFile>,
    next_command_id: u64,
    last_error: Option<String>,
}

impl Player {
    pub fn new() -> Self {
        Self {
            child: None,
            commands: None,
            events: None,
            #[cfg(unix)]
            socket_dir: None,
            status: PlayerStatus::default(),
            audio_backend: String::new(),
            cookie_file: None,
            next_command_id: 1,
            last_error: None,
        }
    }

    #[allow(dead_code)] // Keep the original default-settings entry point for existing callers.
    pub fn load(&mut self, source: &str, volume: f64, speed: f64) -> Result<()> {
        self.load_with_audio(
            source,
            volume,
            speed,
            &crate::store::AudioSettings::default(),
        )
    }

    pub fn load_with_audio(
        &mut self,
        source: &str,
        volume: f64,
        speed: f64,
        audio: &crate::store::AudioSettings,
    ) -> Result<()> {
        self.load_prepared(source, None, volume, speed, audio)
    }

    /// As [`Self::load_with_audio`], with a stream already resolved by [`resolve_stream`] (the
    /// upcoming track is prepared while the current one plays, so Next skips extraction).
    pub fn load_prepared(
        &mut self,
        source: &str,
        prepared: Option<Stream>,
        volume: f64,
        speed: f64,
        audio: &crate::store::AudioSettings,
    ) -> Result<()> {
        self.stop();
        if source.trim().is_empty() {
            bail!("audio source is empty");
        }
        if !source.starts_with("https://") && !std::path::Path::new(source).is_file() {
            bail!("Audio source must be an existing local file or HTTPS URL");
        }
        let volume = valid_range(volume, 0.0, 100.0, "volume")?;
        let speed = valid_range(speed, 0.1, 10.0, "speed")?;
        let mut audio = audio.clone();
        audio.sanitize();
        let force_null_output = std::env::var("BITCHORD_AUDIO_OUTPUT").as_deref() == Ok("null");
        #[cfg(windows)]
        if !force_null_output
            && audio.wasapi_exclusive
            && !audio.output_device.is_empty()
            && !audio
                .output_device
                .to_ascii_lowercase()
                .starts_with("wasapi/")
        {
            bail!(
                "WASAPI exclusive output requires a wasapi/ audio device; selected {}",
                audio.output_device
            );
        }

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

        let stream = prepared.or_else(|| resolve_stream(source));

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
        if !force_null_output && !audio.output_device.is_empty() {
            cmd.arg(format!("--audio-device={}", audio.output_device));
        }
        #[cfg(windows)]
        if !force_null_output && audio.wasapi_exclusive {
            cmd.arg("--audio-exclusive=yes");
        }
        if let Some(filter) = audio_filter(&audio) {
            cmd.arg(format!("--af={filter}"));
        }
        if let Some(ytdlp) = std::env::var_os("BITCHORD_YTDLP") {
            cmd.arg(format!(
                "--script-opt=ytdl_hook-ytdl_path={}",
                ytdlp.to_string_lossy()
            ));
        }
        let cookies = if stream.is_none() {
            crate::auth::cookie_file()?
        } else {
            None
        };
        if let Some(ref file) = cookies {
            cmd.arg(format!(
                "--ytdl-raw-options-append=cookies={}",
                file.path().to_string_lossy()
            ));
        }
        if let Some(ref stream) = stream {
            cmd.arg("--ytdl=no");
            for (name, value) in &stream.headers {
                cmd.arg(format!("--http-header-fields-append={name}: {value}"));
            }
        }
        if force_null_output {
            cmd.arg("--ao=null");
        }
        let child = cmd
            .arg("--")
            .arg(stream.as_ref().map_or(source, |s| s.url.as_str()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| {
                if cfg!(windows) {
                    crate::runtime::missing_tool_message("mpv")
                } else {
                    "starting mpv (install mpv or set BITCHORD_MPV)".into()
                }
            })?;
        self.child = Some(child);
        self.cookie_file = cookies;
        // Fresh Windows runtime binaries may be scanned by antivirus before startup.
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            #[cfg(unix)]
            let connected = std::os::unix::net::UnixStream::connect(&ipc_path).and_then(|events| {
                std::os::unix::net::UnixStream::connect(&ipc_path)
                    .map(|commands| (events, commands))
            });
            #[cfg(windows)]
            let connected = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&ipc_path)
                .and_then(|events| {
                    // A blocking read on a duplicated Windows handle can block writes too.
                    // Separate IPC clients keep controls responsive while events are read.
                    loop {
                        match OpenOptions::new().read(true).write(true).open(&ipc_path) {
                            Ok(commands) => return Ok((events, commands)),
                            // Keep the first client connected while mpv creates another pipe instance.
                            Err(_) if Instant::now() < deadline => {
                                thread::sleep(Duration::from_millis(10))
                            }
                            Err(error) => return Err(error),
                        }
                    }
                });
            match connected {
                Ok((mut reader, commands)) => {
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
                    let event_tx = tx.clone();
                    thread::spawn(move || read_events(reader, event_tx));
                    let (command_tx, command_rx) = mpsc::sync_channel(64);
                    thread::spawn(move || run_commands(commands, command_rx, tx));
                    self.commands = Some(command_tx);
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
    /// Switch the output device of the running player; empty means `auto`. No-op when idle.
    /// The device applies as given: WASAPI exclusive is a load-time setting and is not re-checked.
    #[allow(dead_code)] // used by the player sheet (in progress)
    pub fn set_audio_device(&mut self, device: &str) -> Result<()> {
        if self.commands.is_none() {
            return Ok(());
        }
        let device = device.trim();
        let device = if device.is_empty() { "auto" } else { device };
        self.command(json!(["set_property", "audio-device", device]))
    }
    pub fn take_error(&mut self) -> Option<String> {
        self.last_error.take()
    }

    /// Replace only the filter chain while playback continues. An empty `af set` bypasses it.
    pub fn set_audio_filter(&mut self, settings: &crate::store::AudioSettings) -> Result<()> {
        self.command(json!(["af", "set", ""]))?;
        if let Some(filter) = audio_filter(settings) {
            self.command(json!(["af", "add", filter]))?;
        }
        Ok(())
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
            self.commands = None;
            self.events = None;
        }
        Some(self.status)
    }

    pub fn stop(&mut self) {
        self.commands = None;
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
        self.last_error = None;
    }

    fn command(&mut self, command: Value) -> Result<()> {
        let request_id = self.next_command_id;
        self.next_command_id = self.next_command_id.wrapping_add(1).max(1);
        let commands = self
            .commands
            .as_ref()
            .ok_or_else(|| anyhow!("mpv is not playing"))?;
        commands
            .try_send((request_id, command))
            .map_err(|error| anyhow!("mpv command queue unavailable: {error}"))?;
        self.status.pending_commands = self.status.pending_commands.saturating_add(1);
        Ok(())
    }

    fn apply_event(&mut self, v: &Value) {
        if v["request_id"].is_number() && v["error"].as_str().is_some_and(|e| e != "success") {
            self.status.pending_commands = self.status.pending_commands.saturating_sub(1);
            let command = v["command"]
                .as_array()
                .map_or_else(String::new, |command| format!(" ({command:?})"));
            self.last_error = Some(format!(
                "mpv command rejected: {}{}",
                v["error"].as_str().unwrap_or("unknown error"),
                command
            ));
        }
        if v["request_id"].is_number() && v["error"] == "success" {
            self.status.pending_commands = self.status.pending_commands.saturating_sub(1);
        }
        if let Some(error) = v["transport_error"].as_str() {
            self.status.pending_commands = self.status.pending_commands.saturating_sub(1);
            self.last_error = Some(format!("mpv command transport failed: {error}"));
        }
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
                Some("af") => {
                    self.status.audio_filter_count = v["data"]
                        .as_u64()
                        .map_or_else(|| v["data"].as_array().map_or(0, Vec::len), |n| n as usize);
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
    let mut reader = BufReader::new(reader);
    loop {
        match read_frame(&mut reader) {
            Ok(Some(line)) => {
                if tx.send(Ok(line)).is_err() {
                    return;
                }
            }
            Ok(None) => return,
            Err(error) => {
                let _ = tx.send(Err(error));
                return;
            }
        }
    }
}

fn read_frame(reader: &mut impl BufRead) -> std::io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Ok(Some(frame))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |index| index + 1);
        if frame.len() + count > MAX_IPC_FRAME {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "mpv IPC frame exceeds 1 MiB",
            ));
        }
        frame.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            frame.pop();
            return Ok(Some(frame));
        }
    }
}

fn run_commands(
    stream: impl Read + Write,
    commands: mpsc::Receiver<(u64, Value)>,
    events: mpsc::Sender<std::io::Result<Vec<u8>>>,
) {
    let mut connection = BufReader::new(stream);
    while let Ok((request_id, command)) = commands.recv() {
        let result = (|| {
            serde_json::to_writer(
                connection.get_mut(),
                &json!({"command": command, "request_id": request_id}),
            )?;
            connection.get_mut().write_all(b"\n")?;
            connection.get_mut().flush()?;
            loop {
                let frame = read_frame(&mut connection)?.ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "mpv closed command IPC before replying",
                    )
                })?;
                let reply: Value = serde_json::from_slice(&frame)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
                if reply["request_id"].is_null() && reply["event"].is_string() {
                    if events.send(Ok(frame)).is_err() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "event receiver closed",
                        ));
                    }
                    continue;
                }
                if reply["request_id"].as_u64() != Some(request_id)
                    || reply["error"].as_str().is_none()
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "mpv returned a missing or mismatched command reply",
                    ));
                }
                break Ok(frame);
            }
        })();
        let (frame, failed) = match result {
            Ok(frame) => {
                let success = serde_json::from_slice::<Value>(&frame)
                    .is_ok_and(|reply| reply["error"] == "success");
                if !success {
                    let rejected = serde_json::from_slice::<Value>(&frame).unwrap_or(Value::Null);
                    let frame = serde_json::to_vec(&json!({
                        "request_id": rejected["request_id"],
                        "error": rejected["error"],
                        "command": command
                    }))
                    .unwrap_or_default();
                    (frame, false)
                } else {
                    let af_count = match (success, command[0].as_str(), command[1].as_str()) {
                        (true, Some("af"), Some("set")) => Some(0),
                        (true, Some("af"), Some("add")) => Some(1),
                        _ => None,
                    };
                    let frame = af_count
                        .map(|count| {
                            serde_json::to_vec(&json!({
                                "event": "property-change",
                                "name": "af",
                                "data": count,
                                "request_id": request_id,
                                "error": "success"
                            }))
                            .unwrap_or(frame.clone())
                        })
                        .unwrap_or(frame);
                    (frame, false)
                }
            }
            Err(error) => (
                serde_json::to_vec(&json!({"transport_error": error.to_string()}))
                    .unwrap_or_default(),
                true,
            ),
        };
        if events.send(Ok(frame)).is_err() || failed {
            return;
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
mod stream_tests {
    use super::{parse_stream, Stream};
    use serde_json::json;

    #[test]
    fn parses_direct_stream_from_yt_dlp_json() {
        let info = json!({
            "url": "https://rr1.googlevideo.com/videoplayback?id=1",
            "available_at": 1793710336u64,
            "http_headers": {"User-Agent": "UA", "Accept": "a/b,c/d", "Bad\nName": "x", "X-Evil": "a\r\nHost: y"}
        });
        let (stream, at) = parse_stream(&info).unwrap();
        assert_eq!(at, Some(1793710336));
        assert_eq!(
            stream,
            Stream {
                url: "https://rr1.googlevideo.com/videoplayback?id=1".into(),
                headers: vec![
                    ("Accept".into(), "a/b,c/d".into()),
                    ("User-Agent".into(), "UA".into())
                ],
            }
        );
        // Merged video+audio selections have no single URL: fall back to the mpv hook.
        assert!(parse_stream(&json!({"requested_formats": []})).is_none());
        assert!(parse_stream(&json!({"url": "file:///etc/passwd"})).is_none());
    }
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
    fn command_worker_receives_real_shaped_reply_on_its_connection() {
        use std::sync::{Arc, Mutex};
        struct MockConnection {
            reply: Cursor<Vec<u8>>,
            written: Arc<Mutex<Vec<u8>>>,
        }
        impl Read for MockConnection {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                self.reply.read(out)
            }
        }
        impl Write for MockConnection {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.written.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let written = Arc::new(Mutex::new(Vec::new()));
        let connection = MockConnection {
            reply: Cursor::new(br#"{"request_id":7,"error":"success"}"#.to_vec()),
            written: written.clone(),
        };
        let (commands_tx, commands_rx) = mpsc::channel();
        let (events_tx, events_rx) = mpsc::channel();
        commands_tx.send((7, json!(["cycle", "pause"]))).unwrap();
        drop(commands_tx);
        run_commands(connection, commands_rx, events_tx);
        let reply = events_rx.recv().unwrap().unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&reply).unwrap(),
            json!({"request_id":7,"error":"success"})
        );
        let sent: Value = serde_json::from_slice(
            written
                .lock()
                .unwrap()
                .split(|byte| *byte == b'\n')
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(sent["request_id"], 7);
        assert_eq!(sent["command"], json!(["cycle", "pause"]));
    }

    #[test]
    fn command_rejection_without_event_is_taken_as_error() {
        let mut player = Player::new();
        player.apply_event(&json!({"request_id": 8, "error": "property unavailable"}));
        assert_eq!(
            player.take_error().as_deref(),
            Some("mpv command rejected: property unavailable")
        );
    }

    #[test]
    fn command_worker_forwards_mpv_rejection_without_event_field() {
        use std::sync::{Arc, Mutex};
        struct MockConnection {
            reply: Cursor<Vec<u8>>,
            written: Arc<Mutex<Vec<u8>>>,
        }
        impl Read for MockConnection {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                self.reply.read(out)
            }
        }
        impl Write for MockConnection {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.written.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let connection = MockConnection {
            reply: Cursor::new(br#"{"request_id":9,"error":"invalid parameter"}"#.to_vec()),
            written: Default::default(),
        };
        let (commands_tx, commands_rx) = mpsc::channel();
        let (events_tx, events_rx) = mpsc::channel();
        commands_tx
            .send((9, json!(["af", "add", "invalid"])))
            .unwrap();
        drop(commands_tx);
        run_commands(connection, commands_rx, events_tx);
        let reply: Value = serde_json::from_slice(&events_rx.recv().unwrap().unwrap()).unwrap();
        assert_eq!(reply["request_id"], 9);
        assert_eq!(reply["error"], "invalid parameter");
        assert!(reply.get("event").is_none());
        let mut player = Player::new();
        player.apply_event(&reply);
        assert!(player.take_error().unwrap().contains("invalid parameter"));
    }

    #[test]
    fn ipc_frames_are_bounded() {
        let mut reader = Cursor::new(vec![b'x'; MAX_IPC_FRAME + 1]);
        assert_eq!(
            read_frame(&mut reader).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn filter_serialization_covers_enabled_bypass_balance_and_reset() {
        let mut settings = crate::store::AudioSettings::default();
        assert_eq!(audio_filter(&settings), None);
        settings.equalizer_enabled = true;
        settings.equalizer_bands[0] = 6.0;
        settings.equalizer_bands[3] = -2.5;
        settings.stereo_balance = 0.5;
        let filter = audio_filter(&settings).unwrap();
        assert!(filter.contains("bass=f=60:g=6"));
        assert!(filter.contains("equalizer=f=1000:t=q:w=1:g=-2.5"));
        assert!(filter.contains("stereotools=balance_out=0.5"));
        settings.equalizer_enabled = false;
        assert_eq!(
            audio_filter(&settings).unwrap(),
            "lavfi=[stereotools=balance_out=0.5]"
        );
        settings.stereo_balance = 0.0;
        assert_eq!(audio_filter(&settings), None);
    }

    #[test]
    #[ignore = "requires mpv; run with BITCHORD_AUDIO_OUTPUT=null"]
    fn plays_local_audio() {
        use std::{fs::File, io::Write};
        let path = std::env::temp_dir().join(format!("bitchord-test-{}.wav", std::process::id()));
        let mut f = File::create(&path).unwrap();
        let samples = 44_100u32 * 12;
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
                if let Some(error) = p.take_error() {
                    panic!("mpv rejected a test command: {error}");
                }
                assert!(!s.errored, "mpv could not decode test audio");
                if condition(s) && s.pending_commands == 0 {
                    return s;
                }
                assert!(
                    Instant::now() < until,
                    "timed out waiting for status: {s:?}"
                );
                thread::sleep(Duration::from_millis(25));
            }
        }
        let s = wait_for(&mut p, |s| s.duration > 11.9 && s.position > 0.05);
        println!(
            "Decoded WAV: duration={:.2}s position={:.2}s",
            s.duration, s.position
        );
        let mut eq = crate::store::AudioSettings::default();
        eq.equalizer_enabled = true;
        eq.equalizer_bands[0] = 4.0;
        eq.stereo_balance = 0.25;
        p.set_audio_filter(&eq).unwrap();
        assert!(wait_for(&mut p, |s| s.audio_filter_count > 0).audio_filter_count > 0);
        p.set_audio_filter(&crate::store::AudioSettings::default())
            .unwrap();
        assert_eq!(
            wait_for(&mut p, |s| s.audio_filter_count == 0).audio_filter_count,
            0
        );
        p.command(json!(["af", "add", "this-filter-does-not-exist"]))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while p.take_error().is_none() {
            p.poll().expect("mpv quit during rejected filter check");
            assert!(
                Instant::now() < until,
                "mpv command rejection was not received"
            );
            thread::sleep(Duration::from_millis(10));
        }
        println!("EQ/balance apply, bypass/reset and real command rejection passed");
        p.toggle().unwrap();
        assert!(wait_for(&mut p, |s| s.paused).paused);
        p.seek(1.5).unwrap();
        assert!(wait_for(&mut p, |s| s.position >= 1.4).paused);
        p.volume(30.0).unwrap();
        p.speed(1.2).unwrap();
        p.toggle().unwrap();
        p.seek(11.5).unwrap();
        assert!(wait_for(&mut p, |s| s.ended).ended);
        println!("Pause, seek, volume, speed, resume and natural EOF passed");
        p.stop();
        assert!(p.poll().is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[ignore = "requires mpv, yt-dlp and internet access to YouTube Music"]
    fn prepared_streams_start_fast_and_overlap_for_crossfade() {
        std::env::set_var("BITCHORD_AUDIO_OUTPUT", "null");
        let audible = |p: &mut Player, until: Instant| loop {
            let s = p.poll().expect("mpv exited");
            assert!(!s.errored, "stream could not be opened");
            if s.position > 0.1 {
                return s;
            }
            assert!(Instant::now() < until, "stream did not start");
            thread::sleep(Duration::from_millis(20));
        };
        let a = "https://music.youtube.com/watch?v=4D7u5KF7SP8";
        let b = "https://music.youtube.com/watch?v=dQw4w9WgXcQ";
        let started = Instant::now();
        let stream = resolve_stream(b).expect("fast-path resolution failed");
        println!("prepare: {:.2}s", started.elapsed().as_secs_f32());

        let mut outgoing = Player::new();
        outgoing.load(a, 50.0, 1.0).unwrap();
        audible(&mut outgoing, Instant::now() + Duration::from_secs(45));

        // What the UI does at a crossfade: start the prepared track silent beside the old one.
        let started = Instant::now();
        let mut incoming = Player::new();
        incoming
            .load_prepared(b, Some(stream), 0.0, 1.0, &Default::default())
            .unwrap();
        audible(&mut incoming, Instant::now() + Duration::from_secs(20));
        let open = started.elapsed().as_secs_f32();
        println!("prepared stream audible after {open:.2}s");
        assert!(
            open < 2.5,
            "a prepared stream should skip extraction: {open:.2}s"
        );

        for t in [0.25_f32, 0.5, 1.0] {
            let (old, new) = crate::ui::fade_gains(t);
            outgoing.volume(50.0 * old).unwrap();
            incoming.volume(50.0 * new).unwrap();
        }
        thread::sleep(Duration::from_millis(400));
        let (o, i) = (outgoing.poll().unwrap(), incoming.poll().unwrap());
        println!(
            "volumes after blend: outgoing={:.1} incoming={:.1}",
            o.volume, i.volume
        );
        assert!(o.volume < 1.0 && (i.volume - 50.0).abs() < 0.5);
        assert!(!o.ended && !i.ended, "both players run during the blend");
        outgoing.stop();
        incoming.stop();
    }

    #[test]
    fn video_ids_come_from_watch_urls_only() {
        use super::video_id;
        assert_eq!(
            video_id("https://music.youtube.com/watch?v=dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            video_id("https://www.youtube.com/watch?list=PL1&v=dQw4w9WgXcQ&t=3").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(video_id("https://music.youtube.com/watch?v=short"), None);
        assert_eq!(video_id(r"C:\Music\song.mp3"), None);
    }

    #[test]
    #[ignore = "requires mpv and internet access to YouTube Music"]
    fn selecting_a_track_to_audible_audio() {
        std::env::set_var("BITCHORD_AUDIO_OUTPUT", "null");
        let source = "https://music.youtube.com/watch?v=4D7u5KF7SP8";
        for label in ["cold", "cached"] {
            let started = Instant::now();
            let stream = resolve_stream(source).expect("resolve failed");
            let resolved = started.elapsed().as_secs_f32();
            let mut p = Player::new();
            p.load_prepared(source, Some(stream), 50.0, 1.0, &Default::default())
                .unwrap();
            let until = Instant::now() + Duration::from_secs(20);
            loop {
                let s = p.poll().expect("mpv exited");
                assert!(!s.errored, "stream could not be opened");
                if s.position > 0.05 {
                    break;
                }
                assert!(Instant::now() < until, "no audio");
                thread::sleep(Duration::from_millis(10));
            }
            println!(
                "{label}: resolved {resolved:.2}s, audible {:.2}s after selection",
                started.elapsed().as_secs_f32()
            );
            p.stop();
        }
    }

    #[test]
    #[ignore = "requires mpv, yt-dlp and internet access to YouTube Music"]
    fn plays_youtube_audio() {
        std::env::set_var("BITCHORD_AUDIO_OUTPUT", "null");
        // Play the exact source string the UI passes for a provider search result.
        let track = crate::services::search("Get Lucky Daft Punk")
            .expect("YouTube Music search failed")
            .into_iter()
            .next()
            .expect("YouTube Music search returned no tracks");
        assert!(track
            .source
            .starts_with("https://music.youtube.com/watch?v="));
        println!(
            "Provider selection: {} by {} -> {}",
            track.title, track.artist, track.source
        );
        let mut p = Player::new();
        p.load(&track.source, 50.0, 1.0).unwrap();
        let until = Instant::now() + Duration::from_secs(45);
        loop {
            let status = p.poll().expect("mpv exited without streaming audio");
            assert!(!status.errored, "YouTube stream could not be opened");
            if status.position > 0.1 && status.duration > 30.0 {
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
