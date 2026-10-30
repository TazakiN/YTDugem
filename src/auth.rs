use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tungstenite::{client::IntoClientRequest, Message};

const MAX_INPUT: usize = 1024 * 1024;
const MUSIC: &str = "https://music.youtube.com";
const SIGNING: &[&str] = &["SAPISID", "__Secure-3PAPISID", "__Secure-1PAPISID"];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub email: String,
    pub channel_id: String,
    pub thumbnail: String,
}

#[derive(Clone)]
pub struct Session {
    cookie: String,
    auth_user: String,
    page_id: Option<String>,
    data_sync_id: Option<String>,
    account: Account,
}

static ACTIVE: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
fn active() -> &'static Mutex<Option<Session>> {
    ACTIVE.get_or_init(|| Mutex::new(None))
}

impl Session {
    pub fn apply(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        let origin = MUSIC;
        request
            .header("Cookie", &self.cookie)
            .header(
                "Authorization",
                sapisid_hash(&self.cookie, origin).unwrap_or_default(),
            )
            .header("X-Goog-AuthUser", &self.auth_user)
            .header("X-Origin", origin)
            .header("X-Goog-PageId", self.page_id.as_deref().unwrap_or(""))
    }
    pub fn context_user(&self) -> Option<Value> {
        self.data_sync_id
            .as_ref()
            .map(|id| json!({"onBehalfOfUser": id}))
    }
}

pub fn snapshot() -> Option<Session> {
    active().lock().ok()?.clone()
}
pub fn restore() -> Result<Option<Account>> {
    let Some(data) = read_secret()? else {
        return Ok(None);
    };
    let stored: Stored =
        serde_json::from_slice(&data).context("stored YouTube credentials are invalid")?;
    let s = Session {
        cookie: stored.cookie,
        auth_user: stored.auth_user,
        page_id: stored.page_id,
        data_sync_id: stored.data_sync_id,
        account: stored.account,
    };
    if signing_secret(&s.cookie).is_none()
        || s.cookie.contains(['\r', '\n'])
        || s.auth_user.contains(['\r', '\n'])
        || s.page_id
            .as_deref()
            .is_some_and(|v| v.contains(['\r', '\n']))
        || s.data_sync_id
            .as_deref()
            .is_some_and(|v| v.contains(['\r', '\n']))
    {
        bail!("Stored YouTube credentials are invalid");
    }
    *active()
        .lock()
        .map_err(|_| anyhow!("auth state unavailable"))? = Some(s.clone());
    Ok(Some(s.account))
}
pub fn connect(input: &str, cancel: &AtomicBool) -> Result<Account> {
    let generation = AUTH_GENERATION.load(Ordering::SeqCst);
    commit(parse_cookie_input(input)?, cancel, generation)
}
pub fn logout() -> Result<()> {
    let mut state = active()
        .lock()
        .map_err(|_| anyhow!("auth state unavailable"))?;
    *state = None;
    AUTH_GENERATION.fetch_add(1, Ordering::SeqCst);
    delete_secret()
}
static AUTH_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Serialize, Deserialize)]
struct Stored {
    cookie: String,
    auth_user: String,
    page_id: Option<String>,
    data_sync_id: Option<String>,
    account: Account,
}

pub fn parse_cookie_input(input: &str) -> Result<Session> {
    if input.len() > MAX_INPUT {
        bail!("Cookie input exceeds 1 MiB")
    }
    if input
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
    {
        bail!("Cookie input contains invalid control characters")
    }
    let trimmed = input.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let v: Value = serde_json::from_str(trimmed).context("invalid browser-cookie JSON")?;
        let arr = v
            .as_array()
            .or_else(|| v.get("cookies").and_then(Value::as_array))
            .ok_or_else(|| anyhow!("JSON must contain a cookie array"))?;
        let mut jar = BTreeMap::new();
        for c in arr {
            let d = c.get("domain").and_then(Value::as_str).unwrap_or("");
            if allowed_domain(d) {
                add_cookie(
                    &mut jar,
                    c.get("name").and_then(Value::as_str).unwrap_or(""),
                    c.get("value").and_then(Value::as_str).unwrap_or(""),
                )?;
            }
        }
        return session_from_jar(jar);
    }
    let mut jar = BTreeMap::new();
    for line in input.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with('#') && !line.starts_with("#HttpOnly_") {
            continue;
        }
        if line.contains('\t') {
            // Netscape cookies.txt
            let cols: Vec<_> = line.trim_start_matches("#HttpOnly_").split('\t').collect();
            if cols.len() >= 7 && allowed_domain(cols[0]) {
                add_cookie(&mut jar, cols[5], cols[6])?;
            }
        } else {
            let header = line.strip_prefix("Cookie:").map(str::trim).unwrap_or(line);
            if !header.contains('=') || line.contains(':') && !line.starts_with("Cookie:") {
                bail!("Unrecognized cookie input line");
            }
            for pair in header.split(';') {
                if let Some((n, v)) = pair.split_once('=') {
                    add_cookie(&mut jar, n.trim(), v.trim())?;
                } else {
                    bail!("Invalid cookie pair");
                }
            }
        }
    }
    session_from_jar(jar)
}
fn session_from_jar(jar: BTreeMap<String, String>) -> Result<Session> {
    let cookie = jar
        .iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("; ");
    if signing_secret(&cookie).is_none() {
        bail!("Cookie input has no SAPISID signing cookie")
    }
    Ok(Session {
        cookie,
        auth_user: "0".into(),
        page_id: None,
        data_sync_id: None,
        account: Account::default(),
    })
}
fn add_cookie(jar: &mut BTreeMap<String, String>, n: &str, v: &str) -> Result<()> {
    if n.is_empty()
        || n.chars()
            .any(|c| !c.is_ascii_alphanumeric() && !"!#$%&'*+-.^_`|~".contains(c))
        || v.is_empty()
        || v.bytes()
            .any(|b| b <= 0x20 || b == 0x7f || b == b';' || b == b',')
    {
        bail!("Invalid cookie name or value")
    }
    if jar.insert(n.to_string(), v.to_string()).is_some() {
        bail!("Duplicate cookie name")
    }
    Ok(())
}
fn allowed_domain(d: &str) -> bool {
    let d = d.trim_start_matches('.').to_ascii_lowercase();
    d == "youtube.com" || d.ends_with(".youtube.com")
}
fn signing_secret(cookie: &str) -> Option<&str> {
    cookie
        .split(';')
        .find_map(|p| {
            let (n, v) = p.trim().split_once('=')?;
            SIGNING.contains(&n).then_some(v)
        })
        .filter(|v| !v.is_empty())
}
fn sapisid_hash(cookie: &str, origin: &str) -> Option<String> {
    let secret = SIGNING.iter().find_map(|name| {
        cookie.split(';').find_map(|p| {
            let (n, v) = p.trim().split_once('=')?;
            (n == *name && !v.is_empty()).then_some(v)
        })
    })?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(format!(
        "SAPISIDHASH {ts}_{}",
        format_digest(&Sha1::digest(format!("{ts} {secret} {origin}").as_bytes()))
    ))
}

fn validate_session(s: &Session) -> Result<Account> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let auth = format!(
        "SAPISIDHASH {ts}_{}",
        format_digest(&Sha1::digest(
            format!(
                "{ts} {} {MUSIC}",
                signing_secret(&s.cookie).ok_or_else(|| anyhow!("YouTube sign-in expired"))?
            )
            .as_bytes()
        ))
    );
    let res=client.post("https://music.youtube.com/youtubei/v1/account/account_menu?prettyPrint=false").header("Cookie",&s.cookie).header("Authorization",auth).header("X-Origin",MUSIC).header("X-Goog-AuthUser",&s.auth_user).header("X-Goog-PageId",s.page_id.as_deref().unwrap_or("")).header("Content-Type","application/json").json(&json!({"context":{"client":{"clientName":"WEB_REMIX","clientVersion":"1.20250101.01.00"},"user":{"onBehalfOfUser":s.data_sync_id}}})).send().context("Could not validate YouTube sign-in")?;
    if !res.status().is_success() {
        bail!(
            "YouTube rejected the sign-in ({}); check or replace the cookies",
            res.status()
        )
    }
    let v: Value = res
        .json()
        .context("YouTube returned an invalid account response")?;
    account_from_menu(&v).ok_or_else(|| {
        anyhow!("YouTube did not confirm a signed-in account; cookies may be expired")
    })
}
fn account_from_menu(v: &Value) -> Option<Account> {
    fn walk(v: &Value) -> Option<&Value> {
        if let Some(x) = v
            .get("activeAccountHeaderRenderer")
            .or_else(|| v.get("accountHeaderRenderer"))
        {
            return Some(x);
        }
        match v {
            Value::Object(m) => m.values().find_map(walk),
            Value::Array(a) => a.iter().find_map(walk),
            _ => None,
        }
    }
    fn text(v: &Value) -> String {
        v.get("runs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|r| r.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("")
    }
    let h = walk(v)?;
    let name = text(&h["accountName"]);
    let email = text(&h["email"]);
    let channel_id = text(&h["channelHandle"]);
    let thumbnail = h["accountPhoto"]["thumbnails"]
        .as_array()?
        .iter()
        .rev()
        .find_map(|t| t["url"].as_str())
        .unwrap_or("")
        .to_string();
    if name.is_empty() && email.is_empty() && channel_id.is_empty() {
        return None;
    }
    Some(Account {
        name,
        email,
        channel_id,
        thumbnail,
    })
}
fn format_digest(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn commit(mut s: Session, cancel: &AtomicBool, generation: u64) -> Result<Account> {
    s.account = validate_session(&s)?;
    let mut state = active()
        .lock()
        .map_err(|_| anyhow!("auth state unavailable"))?;
    if cancel.load(Ordering::SeqCst) || AUTH_GENERATION.load(Ordering::SeqCst) != generation {
        bail!("Sign-in cancelled");
    }
    let bytes = serde_json::to_vec(&Stored {
        cookie: s.cookie.clone(),
        auth_user: s.auth_user.clone(),
        page_id: s.page_id.clone(),
        data_sync_id: s.data_sync_id.clone(),
        account: s.account.clone(),
    })?;
    write_secret(&bytes)?;
    *state = Some(s.clone());
    AUTH_GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(s.account)
}

fn default_browser_command() -> Command {
    #[cfg(windows)]
    let mut command = {
        let system = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let mut command = Command::new(system.join("System32/rundll32.exe"));
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(windows))]
    let mut command = Command::new("xdg-open");
    command.arg(MUSIC);
    command
}

/// Opens a page only; account connection still requires validated manual cookies.
pub fn open_default_login() -> Result<()> {
    let status = default_browser_command()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("Could not open your default browser")?;
    if !status.success() {
        bail!("Could not open your default browser ({status}). Open https://music.youtube.com manually and connect with cookies.");
    }
    Ok(())
}

pub fn browser_login(cancel: Arc<AtomicBool>) -> Result<Account> {
    let exe = find_chromium_browser().ok_or_else(|| anyhow!("Automatic sign-in requires Chrome, Edge, or Chromium. To use your default browser, open YouTube Music and connect with cookies."))?;
    let generation = AUTH_GENERATION.load(Ordering::SeqCst);
    let profile = auth_dir()?.join(format!(
        "browser-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir_all(&profile)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&profile, fs::Permissions::from_mode(0o700))?;
    }
    let port = TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
    let mut child = Command::new(exe)
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--remote-debugging-port={port}"))
        .arg("--remote-debugging-address=127.0.0.1")
        .arg("--no-first-run")
        .arg(MUSIC)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not start sign-in browser")?;
    let result = (|| {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(600);
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("Sign-in cancelled")
            }
            if std::time::Instant::now() > deadline {
                bail!("Sign-in timed out")
            }
            if let Ok(r) = client
                .get(format!("http://127.0.0.1:{port}/json/version"))
                .send()
            {
                if let Ok(j) = r.json::<Value>() {
                    if let Some(ws) = j.get("webSocketDebuggerUrl").and_then(Value::as_str) {
                        if valid_cdp_url(ws, port) {
                            if let Ok(cookie) = cdp_cookies(ws, &cancel) {
                                if let Ok(mut s) = parse_cookie_input(&cookie) {
                                    if cancel.load(Ordering::Relaxed) {
                                        bail!("Sign-in cancelled")
                                    }
                                    if let Ok(a) = validate_session(&s) {
                                        s.account = a;
                                        let mut state = active()
                                            .lock()
                                            .map_err(|_| anyhow!("auth state unavailable"))?;
                                        if cancel.load(Ordering::SeqCst)
                                            || AUTH_GENERATION.load(Ordering::SeqCst) != generation
                                        {
                                            bail!("Sign-in cancelled")
                                        }
                                        let bytes = serde_json::to_vec(&Stored {
                                            cookie: s.cookie.clone(),
                                            auth_user: s.auth_user.clone(),
                                            page_id: s.page_id.clone(),
                                            data_sync_id: s.data_sync_id.clone(),
                                            account: s.account.clone(),
                                        })?;
                                        write_secret(&bytes)?;
                                        *state = Some(s.clone());
                                        AUTH_GENERATION.fetch_add(1, Ordering::SeqCst);
                                        return Ok(s.account);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(500));
        }
    })();
    if let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/json/version"))
            .send()
        {
            if let Ok(value) = response.json::<Value>() {
                if let Some(url) = value["webSocketDebuggerUrl"]
                    .as_str()
                    .filter(|url| valid_cdp_url(url, port))
                {
                    if let Ok(mut ws) = cdp_socket(url) {
                        let _ =
                            ws.send(Message::Text(r#"{"id":2,"method":"Browser.close"}"#.into()));
                    }
                }
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_dir_all(profile);
    result
}
fn cdp_cookies(url: &str, cancel: &AtomicBool) -> Result<String> {
    let mut ws = cdp_socket(url)?;
    ws.send(Message::Text(
        r#"{"id":1,"method":"Storage.getCookies"}"#.into(),
    ))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if std::time::Instant::now() >= deadline {
            bail!("Browser cookie request timed out");
        }
        if cancel.load(Ordering::Relaxed) {
            bail!("Sign-in cancelled")
        }
        let m = match ws.read() {
            Ok(m) => m,
            Err(tungstenite::Error::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue
            }
            Err(e) => return Err(e.into()),
        };
        if let Message::Text(t) = m {
            let v: Value = serde_json::from_str(&t)?;
            if v["id"] == 1 {
                let mut jar = BTreeMap::new();
                for c in v["result"]["cookies"].as_array().into_iter().flatten() {
                    let d = c["domain"].as_str().unwrap_or("");
                    if allowed_domain(d) {
                        if let (Some(n), Some(x)) = (c["name"].as_str(), c["value"].as_str()) {
                            let _ = add_cookie(&mut jar, n, x);
                        }
                    }
                }
                return Ok(jar
                    .iter()
                    .map(|(n, v)| format!("{n}={v}"))
                    .collect::<Vec<_>>()
                    .join("; "));
            }
        }
    }
}
fn cdp_socket(url: &str) -> Result<tungstenite::WebSocket<std::net::TcpStream>> {
    let req = url.into_client_request()?;
    let port = req
        .uri()
        .port_u16()
        .ok_or_else(|| anyhow!("Invalid browser port"))?;
    if !valid_cdp_url(url, port) {
        bail!("Invalid browser endpoint");
    }
    let stream = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(2),
    )?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_INPUT))
        .max_frame_size(Some(MAX_INPUT));
    let (ws, _) = tungstenite::client::client_with_config(req, stream, Some(config))?;
    Ok(ws)
}
fn valid_cdp_url(url: &str, port: u16) -> bool {
    url.strip_prefix(&format!("ws://127.0.0.1:{port}/"))
        .is_some_and(|p| !p.is_empty() && !p.contains(['#', '?']))
}
fn find_chromium_browser() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        for p in [
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|p| p.join("Google/Chrome/Application/chrome.exe")),
            std::env::var_os("PROGRAMFILES")
                .map(PathBuf::from)
                .map(|p| p.join("Google/Chrome/Application/chrome.exe")),
            std::env::var_os("PROGRAMFILES")
                .map(PathBuf::from)
                .map(|p| p.join("Microsoft/Edge/Application/msedge.exe")),
            std::env::var_os("PROGRAMFILES(X86)")
                .map(PathBuf::from)
                .map(|p| p.join("Google/Chrome/Application/chrome.exe")),
            std::env::var_os("PROGRAMFILES(X86)")
                .map(PathBuf::from)
                .map(|p| p.join("Microsoft/Edge/Application/msedge.exe")),
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .map(|p| p.join("Microsoft/Edge/Application/msedge.exe")),
        ]
        .into_iter()
        .flatten()
        {
            if p.is_file() {
                return Some(p);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        for n in [
            "google-chrome",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "google-chrome-stable",
        ] {
            if let Ok(p) = Command::new("which").arg(n).output() {
                if p.status.success() {
                    return Some(PathBuf::from(String::from_utf8_lossy(&p.stdout).trim()));
                }
            }
        }
    }
    None
}

fn auth_dir() -> Result<PathBuf> {
    let p = crate::store::data_path()
        .parent()
        .ok_or_else(|| anyhow!("Could not locate application data directory"))?
        .join("auth");
    fs::create_dir_all(&p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700))?;
    }
    Ok(p)
}

pub struct CookieFile {
    path: PathBuf,
}
impl CookieFile {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for CookieFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
pub fn cookie_file() -> Result<Option<CookieFile>> {
    let Some(s) = snapshot() else { return Ok(None) };
    let path = auth_dir()?.join(format!("cookies-{}.txt", std::process::id()));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    for pair in s.cookie.split(';') {
        if let Some((n, v)) = pair.trim().split_once('=') {
            writeln!(f, ".youtube.com\tTRUE\t/\tTRUE\t0\t{n}\t{v}")?;
        }
    }
    f.sync_all()?;
    Ok(Some(CookieFile { path }))
}
fn secret_path() -> Result<PathBuf> {
    Ok(auth_dir()?.join("session.json"))
}
fn read_secret() -> Result<Option<Vec<u8>>> {
    let p = secret_path()?;
    if !p.exists() {
        return Ok(None);
    };
    let b = fs::read(p)?;
    if b.len() > MAX_INPUT {
        bail!("Stored YouTube credentials exceed 1 MiB")
    }
    #[cfg(windows)]
    {
        dpapi(false, &b).map(Some)
    }
    #[cfg(not(windows))]
    {
        Ok(Some(b))
    }
}
fn write_secret(data: &[u8]) -> Result<()> {
    if data.len() > MAX_INPUT {
        bail!("Stored YouTube credentials exceed 1 MiB")
    }
    #[cfg(windows)]
    let data = dpapi(true, data)?;
    let p = secret_path()?;
    let tmp = p.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(data.as_ref())?;
        f.sync_all()?;
    }
    fs::rename(tmp, p)?;
    Ok(())
}
fn delete_secret() -> Result<()> {
    let p = secret_path()?;
    if p.exists() {
        fs::remove_file(p)?;
    }
    Ok(())
}
#[cfg(windows)]
fn dpapi(encrypt: bool, input: &[u8]) -> Result<Vec<u8>> {
    use std::{ffi::c_void, ptr};
    #[repr(C)]
    struct Blob {
        len: u32,
        data: *mut u8,
    }
    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            i: *const Blob,
            d: *const u16,
            e: *mut Blob,
            p: *mut c_void,
            r: *mut c_void,
            f: u32,
            o: *mut Blob,
        ) -> i32;
        fn CryptUnprotectData(
            i: *const Blob,
            d: *mut *mut u16,
            e: *mut Blob,
            p: *mut c_void,
            r: *mut c_void,
            f: u32,
            o: *mut Blob,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(p: *mut c_void) -> *mut c_void;
    }
    let src = Blob {
        len: input.len() as u32,
        data: input.as_ptr() as *mut u8,
    };
    let mut out = Blob {
        len: 0,
        data: ptr::null_mut(),
    };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &src,
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                1,
                &mut out,
            )
        } else {
            CryptUnprotectData(
                &src,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                1,
                &mut out,
            )
        }
    };
    if ok == 0 {
        bail!("Windows credential protection failed")
    }
    let v = unsafe { std::slice::from_raw_parts(out.data, out.len as usize).to_vec() };
    unsafe {
        LocalFree(out.data as *mut c_void);
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_login_uses_os_handler_with_fixed_url() {
        let command = default_browser_command();
        let args: Vec<_> = command.get_args().collect();
        #[cfg(windows)]
        {
            assert!(Path::new(command.get_program()).ends_with("System32/rundll32.exe"));
            assert_eq!(args, ["url.dll,FileProtocolHandler", MUSIC]);
        }
        #[cfg(not(windows))]
        {
            assert_eq!(command.get_program(), "xdg-open");
            assert_eq!(args, [MUSIC]);
        }
    }
    #[test]
    #[ignore = "opens the system default browser; requires a desktop session"]
    fn opens_default_browser_without_connecting_account() {
        assert!(snapshot().is_none());
        open_default_login().unwrap();
        assert!(
            snapshot().is_none(),
            "Opening a browser must not authenticate BitChord"
        );
    }
    #[test]
    fn parses_cookie_and_netscape_and_filters_domains() {
        let s = parse_cookie_input("SAPISID=secret; SID=x").unwrap();
        assert_eq!(signing_secret(&s.cookie), Some("secret"));
        let s = parse_cookie_input(
            ".youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tsecret\n.evil.com\tTRUE\t/\tTRUE\t0\tSID\tx",
        )
        .unwrap();
        assert_eq!(s.cookie, "SAPISID=secret");
    }
    #[test]
    fn account_header_only_confirms_real_signed_in_account() {
        let logged_in = json!({"contents":{"activeAccountHeaderRenderer":{"accountName":{"runs":[{"text":"Ada"}]},"email":{"runs":[{"text":"ada@example.com"}]},"channelHandle":{"runs":[{"text":"@ada"}]},"accountPhoto":{"thumbnails":[{"url":"https://img.test/a"}]}}}});
        let a = account_from_menu(&logged_in).unwrap();
        assert_eq!(
            (a.name.as_str(), a.email.as_str(), a.channel_id.as_str()),
            ("Ada", "ada@example.com", "@ada")
        );
        assert_eq!(a.thumbnail, "https://img.test/a");
        let logged_out = json!({"contents":{"accountItemSectionRenderer":{"contents":[{"accountItem":{"serviceEndpoint":{"browseEndpoint":{"browseId":"FEaccounts"}}}}]}}});
        assert!(account_from_menu(&logged_out).is_none());
    }
    #[test]
    fn cdp_url_is_pinned_to_loopback_owned_port() {
        assert!(valid_cdp_url(
            "ws://127.0.0.1:4321/devtools/browser/abc",
            4321
        ));
        assert!(!valid_cdp_url(
            "ws://localhost:4321/devtools/browser/abc",
            4321
        ));
        assert!(!valid_cdp_url(
            "ws://127.0.0.1:9/devtools/browser/abc",
            4321
        ));
        assert!(!valid_cdp_url(
            "wss://127.0.0.1:4321/devtools/browser/abc",
            4321
        ));
    }
    #[test]
    fn cookie_file_drop_removes_only_its_private_fixture() {
        let dir = std::env::temp_dir().join(format!(
            "dugem-auth-cookie-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("cookies.txt");
        fs::write(&path, "fixture").unwrap();
        drop(CookieFile { path: path.clone() });
        assert!(!path.exists());
        fs::remove_dir(&dir).unwrap();
    }
    #[test]
    fn parses_json_document_and_crlf_netscape() {
        let json = r#"{"cookies":[{"domain":".youtube.com","name":"SAPISID","value":"secret"}]}"#;
        assert_eq!(
            signing_secret(&parse_cookie_input(json).unwrap().cookie),
            Some("secret")
        );
        let netscape = ".youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tsecret\r\n";
        assert_eq!(
            signing_secret(&parse_cookie_input(netscape).unwrap().cookie),
            Some("secret")
        );
        assert!(parse_cookie_input("SAPISID=secret\nInjected: yes").is_err());
    }
    #[test]
    fn rejects_invalid_and_duplicate() {
        assert!(parse_cookie_input("SAPISID=x\nInjected: yes").is_err());
        assert!(parse_cookie_input("SAPISID=x; SAPISID=y").is_err());
        assert!(parse_cookie_input("SID=x").is_err());
    }
    #[test]
    fn hash_deterministic() {
        let h = format_digest(&Sha1::digest(b"123 secret https://music.youtube.com"));
        assert_eq!(h, "70c4da258f21d816b37d5530bd3f6cd07379c8b2");
    }
    #[test]
    fn persistence_roundtrip() {
        let v = b"auth persistence test";
        #[cfg(windows)]
        let protected = dpapi(true, v).unwrap();
        #[cfg(not(windows))]
        let protected = v.to_vec();
        #[cfg(windows)]
        assert_eq!(dpapi(false, &protected).unwrap(), v);
        #[cfg(not(windows))]
        assert_eq!(protected, v);
    }
}
