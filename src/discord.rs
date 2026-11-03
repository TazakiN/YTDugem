//! Local Discord Rich Presence over desktop IPC.
use crate::model::Track;
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
};
// The IPC client below is Windows-only (named pipes); elsewhere presence stays Idle.
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
const APP_ID: &str = "1411019391843172514";
const APP_URL: &str = "https://github.com/kushagrasinghx/BitChord";

#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Waiting,
    Connected,
    Error(String),
}

pub struct Presence {
    tx: SyncSender<Option<Value>>,
    latest: Arc<Mutex<Option<Value>>>,
    status: Arc<Mutex<Status>>,
    shutdown: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Presence {
    pub fn new() -> Self {
        Self::start(
            (0..10)
                .map(|n| format!(r"\\.\pipe\discord-ipc-{n}"))
                .collect(),
        )
    }
    fn start(paths: Vec<String>) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        let latest = Arc::new(Mutex::new(None));
        let status = Arc::new(Mutex::new(Status::Idle));
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_latest = latest.clone();
        let worker_status = status.clone();
        let worker_shutdown = shutdown.clone();
        let handle = thread::Builder::new()
            .name("discord-presence".into())
            .spawn(move || {
                #[cfg(windows)]
                worker(rx, worker_latest, worker_status, worker_shutdown, paths);
                #[cfg(not(windows))]
                {
                    let _ = (rx, worker_latest, worker_status, worker_shutdown, paths);
                }
            })
            .expect("Discord worker");
        Self {
            tx,
            latest,
            status,
            shutdown,
            handle: Some(handle),
        }
    }
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }
    pub fn clear(&self) {
        self.update(None);
    }
    pub fn publish(
        &self,
        track: &Track,
        position: f64,
        speed: f64,
        name: &str,
        buttons: [bool; 2],
    ) {
        self.update(Some(build_activity(track, position, speed, name, buttons)));
    }
    fn update(&self, value: Option<Value>) {
        let mut latest = self.latest.lock().unwrap();
        if equivalent(&latest, &value) {
            return;
        }
        *latest = value.clone();
        drop(latest);
        match self.tx.try_send(value) {
            Ok(()) | Err(TrySendError::Full(_)) => {}
            Err(TrySendError::Disconnected(_)) => {
                *self.status.lock().unwrap() = Status::Error("worker stopped".into())
            }
        }
    }
}
impl Drop for Presence {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.tx.try_send(None);
        // Never join IPC on the GUI thread. Nonblocking I/O notices shutdown within 10 ms.
        self.handle.take();
    }
}

fn build_activity(
    track: &Track,
    position: f64,
    speed: f64,
    name: &str,
    buttons: [bool; 2],
) -> Value {
    let title = nonempty(&track.title, "Unknown title");
    let artist = nonempty(&track.artist, "Unknown artist");
    let speed = if speed.is_finite() {
        speed.clamp(0.5, 2.0)
    } else {
        1.0
    };
    let position = if position.is_finite() {
        position.clamp(0.0, 31_536_000.0)
    } else {
        0.0
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let mut timestamps = serde_json::Map::new();
    timestamps.insert("start".into(), json!(now - (position / speed) as i64));
    if track.duration.is_finite() && track.duration > 0.0 {
        timestamps.insert(
            "end".into(),
            json!(now + ((track.duration - position).clamp(0.0, 31_536_000.0) / speed) as i64),
        );
    }
    let mut assets = serde_json::Map::new();
    if let Some(url) = safe_artwork(&track.thumbnail) {
        assets.insert("large_image".into(), json!(url));
    }
    if !track.album.trim().is_empty() {
        assets.insert(
            "large_text".into(),
            json!(track.album.chars().take(128).collect::<String>()),
        );
    }
    let mut actions = Vec::new();
    if buttons[0] && valid_id(&track.id) && track.source.starts_with("https://music.youtube.com/") {
        actions.push(json!({"label":"Listen on YouTube Music","url":format!("https://music.youtube.com/watch?v={}", track.id)}));
    }
    if buttons[1] {
        actions.push(json!({"label":"Visit BitChord","url":APP_URL}));
    }
    let mut out =
        json!({"type":2,"details":title,"state":artist,"timestamps":timestamps,"assets":assets});
    if !name.trim().is_empty() {
        out["name"] = json!(name.trim().chars().take(128).collect::<String>());
    }
    if !actions.is_empty() {
        out["buttons"] = json!(actions);
    }
    out
}
fn nonempty(s: &str, fallback: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        fallback.into()
    } else {
        s.chars().take(128).collect()
    }
}
fn valid_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn safe_artwork(url: &str) -> Option<&str> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    // Artwork is sent to Discord, so accept public HTTPS hostnames only, never LAN/IP URLs.
    (url.len() <= 2048
        && parsed.scheme() == "https"
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && host.contains('.')
        && !host.ends_with('.')
        && !host.ends_with(".localhost")
        && !host.ends_with(".local")
        && host.parse::<std::net::IpAddr>().is_err()
        && !host.contains(':')
        && !host.starts_with('[')
        && !url.contains(['\\', '\r', '\n', '\0']))
    .then_some(url)
}

// Playback polling jitters slightly; only a real seek/timing change should republish.
fn equivalent(a: &Option<Value>, b: &Option<Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            let mut metadata_a = a.clone();
            let mut metadata_b = b.clone();
            metadata_a.as_object_mut().unwrap().remove("timestamps");
            metadata_b.as_object_mut().unwrap().remove("timestamps");
            metadata_a == metadata_b
                && ["start", "end"].iter().all(|key| {
                    match (a["timestamps"][key].as_i64(), b["timestamps"][key].as_i64()) {
                        (Some(a), Some(b)) => a.abs_diff(b) <= 2,
                        (None, None) => true,
                        _ => false,
                    }
                })
        }
        _ => false,
    }
}

#[cfg(windows)]
fn worker(
    rx: mpsc::Receiver<Option<Value>>,
    latest: Arc<Mutex<Option<Value>>>,
    status: Arc<Mutex<Status>>,
    shutdown: Arc<AtomicBool>,
    paths: Vec<String>,
) {
    use std::time::Instant;
    let mut connection: Option<ipc::Pipe> = None;
    let mut sent = None;
    let mut retry = Instant::now();
    let mut last_send = Instant::now() - Duration::from_secs(20);
    let mut nonce = 0u64;
    while !shutdown.load(Ordering::Relaxed) {
        let desired = latest.lock().unwrap().clone();
        if desired.is_none() && connection.is_none() {
            *status.lock().unwrap() = Status::Idle;
        } else if connection.is_none() && Instant::now() >= retry {
            *status.lock().unwrap() = Status::Waiting;
            connection = paths.iter().find_map(|p| ipc::Pipe::connect(p).ok());
            if let Some(pipe) = connection.as_mut() {
                if let Err(error) = pipe.handshake(&shutdown) {
                    *status.lock().unwrap() = Status::Error(error.to_string());
                    connection = None;
                }
            }
            retry = Instant::now() + Duration::from_secs(2);
            sent = None;
            last_send = Instant::now() - Duration::from_secs(20);
        }
        if let Some(pipe) = connection.as_mut() {
            if let Err(error) = pipe.events(&shutdown) {
                *status.lock().unwrap() = Status::Error(error.to_string());
                connection = None;
                retry = Instant::now() + Duration::from_secs(2);
            }
        }
        if let Some(pipe) = connection.as_mut() {
            // Clear/first publication is immediate; edits are coalesced to at most once/2s.
            if (desired != sent || last_send.elapsed() >= Duration::from_secs(15))
                && (desired.is_none()
                    || sent.is_none()
                    || last_send.elapsed() >= Duration::from_secs(2))
            {
                nonce += 1;
                match pipe.activity(&desired, nonce, &shutdown) {
                    Ok(()) => {
                        sent = desired.clone();
                        last_send = Instant::now();
                        *status.lock().unwrap() = if desired.is_some() {
                            Status::Connected
                        } else {
                            Status::Idle
                        };
                        if desired.is_none() {
                            connection = None;
                        }
                    }
                    Err(error) => {
                        *status.lock().unwrap() = Status::Error(error.to_string());
                        connection = None;
                        retry = Instant::now() + Duration::from_secs(2);
                    }
                }
            }
        }
        if let Err(mpsc::RecvTimeoutError::Disconnected) =
            rx.recv_timeout(Duration::from_millis(50))
        {
            break;
        }
    }
    if let Some(mut pipe) = connection {
        // Best-effort bounded clear on exit; don't leave an activity until Discord times out.
        pipe.timeout = Duration::from_millis(200);
        let _ = pipe.activity(&None, nonce + 1, &AtomicBool::new(false));
    }
}

#[cfg(windows)]
mod ipc {
    use super::*;
    use std::io::{Read, Write};
    use std::{
        fs::{File, OpenOptions},
        io,
        os::windows::io::AsRawHandle,
        time::Instant,
    };
    #[link(name = "kernel32")]
    extern "system" {
        fn SetNamedPipeHandleState(
            handle: *mut std::ffi::c_void,
            mode: *const u32,
            count: *const u32,
            timeout: *const u32,
        ) -> i32;
        fn PeekNamedPipe(
            handle: *mut std::ffi::c_void,
            buffer: *mut std::ffi::c_void,
            size: u32,
            read: *mut u32,
            available: *mut u32,
            left: *mut u32,
        ) -> i32;
    }
    pub(super) struct Pipe {
        file: File,
        pub timeout: Duration,
    }
    impl Pipe {
        pub fn connect(path: &str) -> io::Result<Self> {
            let file = OpenOptions::new().read(true).write(true).open(path)?;
            // PIPE_NOWAIT: both reads and writes return immediately, including when Discord hangs.
            if unsafe {
                SetNamedPipeHandleState(
                    file.as_raw_handle(),
                    &1,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                file,
                timeout: Duration::from_secs(2),
            })
        }
        fn transfer(
            &mut self,
            bytes: &mut [u8],
            write: bool,
            deadline: Instant,
            cancelled: &AtomicBool,
        ) -> io::Result<()> {
            let mut offset = 0;
            while offset < bytes.len() {
                if cancelled.load(Ordering::Relaxed) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Discord worker stopping",
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Discord IPC timed out",
                    ));
                }
                let result = if write {
                    self.file.write(&bytes[offset..])
                } else {
                    self.file.read(&mut bytes[offset..])
                };
                match result {
                    Ok(n) if n > 0 => {
                        offset += n;
                        continue;
                    }
                    Ok(_) => {}
                    Err(e)
                        if e.raw_os_error() == Some(232)
                            || e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
                thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        }
        fn send(
            &mut self,
            op: u32,
            value: &Value,
            deadline: Instant,
            cancelled: &AtomicBool,
        ) -> io::Result<()> {
            let body = serde_json::to_vec(value)?;
            if body.len() > 65536 {
                return Err(io::Error::other("oversized Discord IPC frame"));
            }
            let mut bytes = [
                op.to_le_bytes().as_slice(),
                (body.len() as u32).to_le_bytes().as_slice(),
                &body,
            ]
            .concat();
            self.transfer(&mut bytes, true, deadline, cancelled)
        }
        fn read_frame(
            &mut self,
            deadline: Instant,
            cancelled: &AtomicBool,
        ) -> io::Result<(u32, Value)> {
            let mut header = [0; 8];
            self.transfer(&mut header, false, deadline, cancelled)?;
            let op = u32::from_le_bytes(header[..4].try_into().unwrap());
            let len = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
            if len > 65536 {
                return Err(io::Error::other("oversized Discord IPC frame"));
            }
            let mut body = vec![0; len];
            self.transfer(&mut body, false, deadline, cancelled)?;
            if op == 2 {
                return Err(io::Error::other("Discord closed IPC"));
            }
            let value = serde_json::from_slice(&body).map_err(io::Error::other)?;
            Ok((op, value))
        }
        pub fn events(&mut self, cancelled: &AtomicBool) -> io::Result<()> {
            let mut available = 0;
            if unsafe {
                PeekNamedPipe(
                    self.file.as_raw_handle(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            if available > 0 {
                let deadline = Instant::now() + self.timeout;
                let (op, value) = self.read_frame(deadline, cancelled)?;
                if op == 3 {
                    self.send(4, &value, deadline, cancelled)?;
                } else if op != 1 && op != 4 {
                    return Err(io::Error::other("invalid Discord IPC opcode"));
                }
            }
            Ok(())
        }
        fn receive(
            &mut self,
            deadline: Instant,
            cancelled: &AtomicBool,
        ) -> io::Result<(u32, Value)> {
            loop {
                let (op, value) = self.read_frame(deadline, cancelled)?;
                if op == 3 {
                    self.send(4, &value, deadline, cancelled)?;
                    continue;
                }
                if op == 4 {
                    continue;
                }
                if op != 1 {
                    return Err(io::Error::other("invalid Discord IPC opcode"));
                }
                return Ok((op, value));
            }
        }
        pub fn handshake(&mut self, cancelled: &AtomicBool) -> io::Result<()> {
            let deadline = Instant::now() + self.timeout;
            self.send(0, &json!({"v":1,"client_id":APP_ID}), deadline, cancelled)?;
            let (_, value) = self.receive(deadline, cancelled)?;
            if value["cmd"] != "DISPATCH" || value["evt"] != "READY" {
                return Err(io::Error::other("Discord handshake rejected"));
            }
            Ok(())
        }
        pub fn activity(
            &mut self,
            activity: &Option<Value>,
            nonce: u64,
            cancelled: &AtomicBool,
        ) -> io::Result<()> {
            let deadline = Instant::now() + self.timeout;
            let nonce = nonce.to_string();
            self.send(1, &json!({"cmd":"SET_ACTIVITY","args":{"pid":std::process::id(),"activity":activity},"nonce":nonce}), deadline, cancelled)?;
            loop {
                let (_, value) = self.receive(deadline, cancelled)?;
                if value["nonce"] != nonce {
                    continue;
                }
                if value["evt"] == "ERROR" {
                    return Err(io::Error::other("Discord rejected activity"));
                }
                if value["cmd"] != "SET_ACTIVITY" {
                    return Err(io::Error::other("invalid Discord acknowledgement"));
                }
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn payload_is_safe_and_complete() {
        let t = Track {
            id: "5NV6Rdv1a3I".into(),
            title: " Song ".into(),
            artist: "Artist".into(),
            album: String::new(),
            duration: 120.0,
            thumbnail: "https://cdn.example/a.jpg".into(),
            source: "https://music.youtube.com/watch?v=5NV6Rdv1a3I".into(),
        };
        let p = build_activity(&t, 12.0, 1.5, "", [true, false]);
        assert_eq!(p["details"], "Song");
        assert_eq!(p["state"], "Artist");
        assert!(
            p["timestamps"]["end"].as_i64().unwrap() > p["timestamps"]["start"].as_i64().unwrap()
        );
        assert_eq!(
            p["buttons"][0]["url"],
            "https://music.youtube.com/watch?v=5NV6Rdv1a3I"
        );
        assert!(safe_artwork("file:///private").is_none());
        assert!(safe_artwork("https://127.0.0.1/a").is_none());
        let mut bad = t.clone();
        bad.duration = f64::NAN;
        bad.title.clear();
        bad.source = r"C:\private\song.flac".into();
        let p = build_activity(&bad, f64::INFINITY, f64::NAN, "", [true, false]);
        assert_eq!(p["details"], "Unknown title");
        assert!(p["timestamps"]["end"].is_null());
        assert!(p["buttons"].is_null());
        let timed = build_activity(&t, 12.0, 1.5, "", [false, false]);
        assert_eq!(
            timed["timestamps"]["end"].as_i64().unwrap()
                - timed["timestamps"]["start"].as_i64().unwrap(),
            80
        );
        for url in [
            "https://user:secret@cdn.example/a",
            "https://[::1]/a",
            "https://172.16.0.1/a",
            "https://host.local/a",
        ] {
            assert!(safe_artwork(url).is_none(), "{url}");
        }
    }

    #[test]
    fn playback_jitter_is_deduplicated_but_seeks_and_clear_are_not() {
        let activity = Some(json!({"details":"Song","timestamps":{"start":100,"end":200}}));
        assert!(equivalent(
            &activity,
            &Some(json!({"details":"Song","timestamps":{"start":102,"end":202}}))
        ));
        assert!(!equivalent(
            &activity,
            &Some(json!({"details":"Song","timestamps":{"start":110,"end":210}}))
        ));
        assert!(!equivalent(&activity, &None));
    }

    #[cfg(windows)]
    mod windows_pipe {
        use super::*;
        use std::{
            ffi::c_void,
            fs::File,
            io::{Read, Write},
            os::windows::io::FromRawHandle,
            time::Instant,
        };

        #[link(name = "kernel32")]
        extern "system" {
            fn CreateNamedPipeW(
                name: *const u16,
                access: u32,
                mode: u32,
                instances: u32,
                out_size: u32,
                in_size: u32,
                timeout: u32,
                security: *const c_void,
            ) -> *mut c_void;
            fn ConnectNamedPipe(handle: *mut c_void, overlapped: *mut c_void) -> i32;
        }

        struct Server {
            file: File,
        }
        impl Server {
            fn new(path: &str) -> Self {
                let name = path.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
                // Unique test-only pipe, byte stream, nonblocking. Never a real Discord endpoint.
                let raw = unsafe {
                    CreateNamedPipeW(name.as_ptr(), 3, 1, 1, 65536, 65536, 0, std::ptr::null())
                };
                assert_ne!(raw as isize, -1, "{}", std::io::Error::last_os_error());
                unsafe {
                    ConnectNamedPipe(raw, std::ptr::null_mut());
                }
                Self {
                    file: unsafe { File::from_raw_handle(raw) },
                }
            }
            fn read_exact(&mut self, bytes: &mut [u8]) {
                let until = Instant::now() + Duration::from_secs(8);
                let mut offset = 0;
                while offset < bytes.len() {
                    match self.file.read(&mut bytes[offset..]) {
                        Ok(n) if n > 0 => {
                            offset += n;
                            continue;
                        }
                        Ok(_) => {}
                        Err(e) if matches!(e.raw_os_error(), Some(232 | 536)) => {}
                        Err(e) => panic!("test server read: {e}"),
                    }
                    assert!(Instant::now() < until, "test server timed out");
                    thread::sleep(Duration::from_millis(10));
                }
            }
            fn read(&mut self) -> (u32, Value) {
                let mut header = [0; 8];
                self.read_exact(&mut header);
                let size = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
                assert!(size <= 65536);
                let mut body = vec![0; size];
                self.read_exact(&mut body);
                (
                    u32::from_le_bytes(header[..4].try_into().unwrap()),
                    serde_json::from_slice(&body).unwrap(),
                )
            }
            fn send(&mut self, op: u32, body: Value) {
                let body = serde_json::to_vec(&body).unwrap();
                self.file
                    .write_all(
                        &[
                            op.to_le_bytes().as_slice(),
                            (body.len() as u32).to_le_bytes().as_slice(),
                            &body,
                        ]
                        .concat(),
                    )
                    .unwrap();
            }
            fn handshake(&mut self) {
                let (op, body) = self.read();
                assert_eq!(op, 0);
                assert_eq!(body, json!({"v":1,"client_id":APP_ID}));
                self.send(1, json!({"cmd":"DISPATCH","evt":"READY"}));
            }
            fn acknowledge(&mut self, request: &Value) {
                assert_eq!(request["cmd"], "SET_ACTIVITY");
                self.send(
                    1,
                    json!({"cmd":"SET_ACTIVITY","evt":null,"nonce":request["nonce"]}),
                );
            }
        }
        fn path() -> String {
            format!(
                r"\\.\pipe\ytdugem-discord-test-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            )
        }
        fn activity() -> Value {
            json!({"type":2,"details":"Test song","state":"Test artist"})
        }
        fn wait(p: &Presence, predicate: impl Fn(&Status) -> bool) {
            let until = Instant::now() + Duration::from_secs(8);
            while !predicate(&p.status()) {
                assert!(Instant::now() < until, "status: {:?}", p.status());
                thread::sleep(Duration::from_millis(10));
            }
        }
        fn shutdown(mut p: Presence) {
            let handle = p.handle.take().unwrap();
            drop(p);
            let until = Instant::now() + Duration::from_secs(1);
            while !handle.is_finished() {
                assert!(Instant::now() < until, "worker did not exit");
                thread::sleep(Duration::from_millis(10));
            }
            handle.join().unwrap();
        }

        #[test]
        fn named_pipe_handshake_activity_ping_clear_and_shutdown() {
            let path = path();
            let mut server = Server::new(&path);
            let p = Presence::start(vec![path]);
            p.update(Some(activity()));
            server.handshake();
            let (op, request) = server.read();
            assert_eq!(op, 1);
            assert_eq!(request["args"]["activity"], activity());
            assert_eq!(request["args"]["pid"], std::process::id());
            server.send(3, json!({"ping":"test"}));
            assert_eq!(server.read(), (4, json!({"ping":"test"})));
            // An unrelated event must not count as an acknowledgement.
            server.send(1, json!({"cmd":"DISPATCH","evt":"OTHER"}));
            server.acknowledge(&request);
            wait(&p, |s| *s == Status::Connected);
            server.send(3, json!({"idle_ping":true}));
            assert_eq!(server.read(), (4, json!({"idle_ping":true})));
            for _ in 0..10_000 {
                p.update(Some(activity()));
            }
            assert_eq!(p.latest.lock().unwrap().as_ref(), Some(&activity()));
            p.clear();
            let (_, request) = server.read();
            assert!(request["args"]["activity"].is_null());
            server.acknowledge(&request);
            wait(&p, |s| *s == Status::Idle);
            shutdown(p);
            println!("PASS isolated named pipe: READY, activity, ping/PONG, nonce ack, coalescing, clear, worker shutdown");
        }

        #[test]
        fn named_pipe_reconnects_after_discord_restart_and_clears_on_drop() {
            let path = path();
            let mut server = Server::new(&path);
            let p = Presence::start(vec![path.clone()]);
            p.update(Some(activity()));
            server.handshake();
            let (_, request) = server.read();
            server.acknowledge(&request);
            wait(&p, |s| *s == Status::Connected);
            server.send(2, json!({"code":1000}));
            drop(server);
            let mut server = Server::new(&path);
            p.update(Some(json!({"type":2,"details":"Next song"})));
            wait(&p, |s| matches!(s, Status::Error(_)));
            server.handshake();
            let (_, request) = server.read();
            assert_eq!(request["args"]["activity"]["details"], "Next song");
            server.acknowledge(&request);
            wait(&p, |s| *s == Status::Connected);
            let handle = p.handle.as_ref().unwrap().thread().id();
            drop(p);
            let (_, request) = server.read();
            assert!(request["args"]["activity"].is_null());
            server.acknowledge(&request);
            println!("PASS isolated named pipe: reconnect and shutdown clear ({handle:?})");
        }

        #[test]
        fn named_pipe_times_out_rejects_errors_and_cancels_hung_reads() {
            let path = path();
            let mut server = Server::new(&path);
            let p = Presence::start(vec![path]);
            p.update(Some(activity()));
            server.handshake();
            server.read();
            // Wrong nonce is ignored, then lack of matching ack is a timeout, not success.
            server.send(1, json!({"cmd":"SET_ACTIVITY","nonce":"wrong"}));
            wait(
                &p,
                |s| matches!(s, Status::Error(e) if e.contains("timed out")),
            );
            shutdown(p);
            drop(server);

            let path = self::path();
            let mut server = Server::new(&path);
            let p = Presence::start(vec![path]);
            p.update(Some(activity()));
            server.handshake();
            let (_, request) = server.read();
            server.send(1, json!({"cmd":"SET_ACTIVITY","evt":"ERROR","nonce":request["nonce"],"data":{"message":"denied"}}));
            wait(
                &p,
                |s| matches!(s, Status::Error(e) if e.contains("rejected")),
            );
            shutdown(p);
            drop(server);

            let path = self::path();
            let mut server = Server::new(&path);
            let p = Presence::start(vec![path]);
            p.update(Some(activity()));
            server.read(); // Withhold READY entirely; Drop still must finish within 1 second.
            shutdown(p);
            println!("PASS isolated named pipe: nonce timeout, ERROR rejection, hung handshake cancellation");
        }

        #[test]
        fn named_pipe_rejects_invalid_handshake_and_oversized_frames() {
            for oversized in [false, true] {
                let path = path();
                let mut server = Server::new(&path);
                let p = Presence::start(vec![path]);
                p.update(Some(activity()));
                server.read();
                if oversized {
                    server
                        .file
                        .write_all(&[1u32.to_le_bytes(), 65537u32.to_le_bytes()].concat())
                        .unwrap();
                } else {
                    server.send(1, json!({"cmd":"DISPATCH","evt":"NOT_READY"}));
                }
                wait(&p, |s| matches!(s, Status::Error(_)));
                shutdown(p);
            }
        }
    }
}
