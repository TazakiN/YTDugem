use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
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
const WEB_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
const WEB_VERSION: &str = "1.20250101.01.00";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub email: String,
    pub channel_id: String,
    pub thumbnail: String,
    #[serde(default)]
    pub scope_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedAccount {
    pub id: String,
    pub account: Account,
    pub active: bool,
}

#[derive(Clone)]
pub struct Session {
    cookie: String,
    auth_user: String,
    page_id: Option<String>,
    data_sync_id: Option<String>,
    account: Account,
    visitor_data: Option<String>,
    client_version: Option<String>,
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
        let request = request
            .header("Cookie", &self.cookie)
            .header(
                "Authorization",
                sapisid_hash(&self.cookie, origin).unwrap_or_default(),
            )
            .header("X-Goog-AuthUser", &self.auth_user)
            .header("X-Origin", origin)
            .header("Origin", origin)
            .header("Referer", format!("{origin}/"))
            .header("User-Agent", WEB_AGENT)
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("X-YouTube-Client-Name", "67")
            .header(
                "X-YouTube-Client-Version",
                self.client_version.as_deref().unwrap_or(WEB_VERSION),
            );
        let request = match &self.page_id {
            Some(id) if !id.is_empty() => request.header("X-Goog-PageId", id),
            _ => request,
        };
        match &self.visitor_data {
            Some(id) => request.header("X-Goog-Visitor-Id", id),
            None => request,
        }
    }
    pub fn context_user(&self) -> Option<Value> {
        self.data_sync_id
            .as_ref()
            .map(|id| json!({"onBehalfOfUser": id}))
    }
    pub fn context(&self) -> Value {
        let mut value = json!({"client":{"clientName":"WEB_REMIX","clientVersion":self.client_version.as_deref().unwrap_or(WEB_VERSION),"hl":"en","gl":"US"},"user":{"lockedSafetyMode":false},"request":{"useSsl":true}});
        if let Some(user) = self.context_user() {
            value["user"]["onBehalfOfUser"] = user["onBehalfOfUser"].clone();
        }
        if let Some(visitor) = &self.visitor_data {
            value["client"]["visitorData"] = json!(visitor);
        }
        value
    }
}

pub fn snapshot() -> Option<Session> {
    active().lock().ok()?.clone()
}
pub fn current_account() -> Option<Account> {
    active().lock().ok()?.as_ref().map(|s| s.account.clone())
}
pub fn restore() -> Result<Option<Account>> {
    let generation = AUTH_GENERATION.load(Ordering::SeqCst);
    let Some(data) = read_secret()? else {
        return Ok(None);
    };
    let mut accounts = decode_container(&data)?;
    let Some(id) = accounts.active_id.clone() else {
        return Ok(None);
    };
    let Some(stored) = accounts.accounts.iter().find(|a| a.id == id) else {
        bail!("Stored active YouTube account is missing");
    };
    let s = stored.session()?;
    let mut state = active()
        .lock()
        .map_err(|_| anyhow!("auth state unavailable"))?;
    if AUTH_GENERATION.load(Ordering::SeqCst) != generation {
        bail!("Sign-in changed while restoring saved account");
    }
    if accounts.legacy {
        accounts.legacy = false;
        write_container(&accounts)?;
    }
    *state = Some(s.clone());
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
    let mut accounts = match read_secret()? {
        Some(bytes) => decode_container(&bytes)?,
        None => CredentialContainer::default(),
    };
    sign_out_saved(&mut accounts);
    accounts.legacy = false;
    write_container(&accounts)?;
    *state = None;
    AUTH_GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}
static AUTH_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn check_sign_in_active(cancel: &AtomicBool, generation: u64) -> Result<()> {
    if cancel.load(Ordering::SeqCst) || AUTH_GENERATION.load(Ordering::SeqCst) != generation {
        bail!("Sign-in cancelled")
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    cookie: String,
    auth_user: String,
    page_id: Option<String>,
    data_sync_id: Option<String>,
    account: Account,
    #[serde(default)]
    visitor_data: Option<String>,
    #[serde(default)]
    client_version: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredAccount {
    id: String,
    #[serde(flatten)]
    credential: Stored,
}

impl StoredAccount {
    fn session(&self) -> Result<Session> {
        let s = Session {
            cookie: self.credential.cookie.clone(),
            auth_user: self.credential.auth_user.clone(),
            page_id: self.credential.page_id.clone(),
            data_sync_id: self.credential.data_sync_id.clone(),
            account: self.credential.account.clone(),
            visitor_data: self.credential.visitor_data.clone(),
            client_version: self.credential.client_version.clone(),
        };
        validate_stored_session(&s)?;
        Ok(s)
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
struct CredentialContainer {
    version: u32,
    active_id: Option<String>,
    accounts: Vec<StoredAccount>,
    #[serde(skip)]
    legacy: bool,
}

fn validate_stored_session(s: &Session) -> Result<()> {
    if signing_secret(&s.cookie).is_none()
        || s.cookie.contains(['\r', '\n'])
        || s.auth_user.contains(['\r', '\n'])
        || s.page_id
            .as_deref()
            .is_some_and(|v| v.contains(['\r', '\n']))
        || s.data_sync_id
            .as_deref()
            .is_some_and(|v| v.contains(['\r', '\n']))
        || [&s.visitor_data, &s.client_version]
            .into_iter()
            .flatten()
            .any(|v| v.len() > 4096 || !v.bytes().all(|b| (0x21..=0x7e).contains(&b)))
    {
        bail!("Stored YouTube credentials are invalid");
    }
    Ok(())
}

fn decode_container(data: &[u8]) -> Result<CredentialContainer> {
    let value: Value =
        serde_json::from_slice(data).context("stored YouTube credentials are invalid")?;
    if value.get("version").is_some() {
        let container: CredentialContainer =
            serde_json::from_value(value).context("stored YouTube credentials are invalid")?;
        if container.version != 1 {
            bail!("Unsupported YouTube credential container version");
        }
        let mut ids = std::collections::BTreeSet::new();
        for entry in &container.accounts {
            if entry.id.is_empty() || !ids.insert(entry.id.as_str()) {
                bail!("Stored YouTube accounts are invalid");
            }
            entry.session()?;
        }
        if container
            .active_id
            .as_ref()
            .is_some_and(|id| !ids.contains(id.as_str()))
        {
            bail!("Stored active YouTube account is missing");
        }
        Ok(container)
    } else {
        let stored: Stored =
            serde_json::from_value(value).context("stored YouTube credentials are invalid")?;
        let id = account_id(&stored.account);
        let entry = StoredAccount {
            id: id.clone(),
            credential: stored,
        };
        entry.session()?;
        Ok(CredentialContainer {
            version: 1,
            active_id: Some(id),
            accounts: vec![entry],
            legacy: true,
        })
    }
}

fn account_id(account: &Account) -> String {
    // Identity is derived only from public account metadata, never the credential.
    let identity = if !account.scope_id.is_empty() {
        &account.scope_id
    } else if !account.email.is_empty() {
        &account.email
    } else if !account.channel_id.is_empty() {
        &account.channel_id
    } else {
        &account.name
    };
    format!(
        "acct-{}",
        format_digest(&Sha1::digest(
            identity.trim().to_ascii_lowercase().as_bytes()
        ))
    )
}

fn stored_session(s: &Session) -> Stored {
    Stored {
        cookie: s.cookie.clone(),
        auth_user: s.auth_user.clone(),
        page_id: s.page_id.clone(),
        data_sync_id: s.data_sync_id.clone(),
        account: s.account.clone(),
        visitor_data: s.visitor_data.clone(),
        client_version: s.client_version.clone(),
    }
}

fn write_container(container: &CredentialContainer) -> Result<()> {
    let mut container = container.clone();
    container.version = 1;
    container.legacy = false;
    write_secret(&serde_json::to_vec(&container)?)
}

fn activate_saved(container: &mut CredentialContainer, id: &str) -> Result<()> {
    if !container.accounts.iter().any(|stored| stored.id == id) {
        bail!("Saved YouTube account was not found");
    }
    container.active_id = Some(id.to_owned());
    Ok(())
}

fn remove_saved(container: &mut CredentialContainer, id: &str) -> bool {
    let before = container.accounts.len();
    container.accounts.retain(|stored| stored.id != id);
    let removed = container.accounts.len() != before;
    if container.active_id.as_deref() == Some(id) {
        container.active_id = None;
    }
    removed
}

fn sign_out_saved(container: &mut CredentialContainer) {
    if let Some(id) = container.active_id.take() {
        container.accounts.retain(|account| account.id != id);
    }
    container.legacy = false;
}

pub fn saved_accounts() -> Result<Vec<SavedAccount>> {
    let Some(data) = read_secret()? else {
        return Ok(Vec::new());
    };
    let container = decode_container(&data)?;
    Ok(container
        .accounts
        .into_iter()
        .map(|stored| SavedAccount {
            active: container.active_id.as_deref() == Some(stored.id.as_str()),
            id: stored.id,
            account: stored.credential.account,
        })
        .collect())
}

pub fn select_account(id: &str, cancel: &AtomicBool) -> Result<Account> {
    let generation = AUTH_GENERATION.load(Ordering::SeqCst);
    let data = read_secret()?.ok_or_else(|| anyhow!("No saved YouTube accounts"))?;
    let container = decode_container(&data)?;
    let mut session = container
        .accounts
        .iter()
        .find(|stored| stored.id == id)
        .ok_or_else(|| anyhow!("Saved YouTube account was not found"))?
        .session()?;
    check_sign_in_active(cancel, generation)?;
    session.account = validate_session(&mut session)?;
    let mut state = active()
        .lock()
        .map_err(|_| anyhow!("auth state unavailable"))?;
    check_sign_in_active(cancel, generation)?;
    let latest = read_secret()?.ok_or_else(|| anyhow!("No saved YouTube accounts"))?;
    let mut latest = decode_container(&latest)?;
    let target = latest
        .accounts
        .iter_mut()
        .find(|stored| stored.id == id)
        .ok_or_else(|| anyhow!("Saved YouTube account was removed"))?;
    if target.credential.cookie != session.cookie {
        bail!("Saved YouTube account changed during validation");
    }
    target.credential = stored_session(&session);
    activate_saved(&mut latest, id)?;
    write_container(&latest)?;
    *state = Some(session.clone());
    AUTH_GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(session.account)
}

pub fn remove_account(id: &str) -> Result<()> {
    let mut state = active()
        .lock()
        .map_err(|_| anyhow!("auth state unavailable"))?;
    let data = read_secret()?.ok_or_else(|| anyhow!("Saved YouTube account was not found"))?;
    let mut container = decode_container(&data)?;
    let was_active = container.active_id.as_deref() == Some(id);
    if !remove_saved(&mut container, id) {
        bail!("Saved YouTube account was not found");
    }
    write_container(&container)?;
    if was_active {
        *state = None;
        AUTH_GENERATION.fetch_add(1, Ordering::SeqCst);
    }
    Ok(())
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
            let header = if line
                .get(..7)
                .is_some_and(|s| s.eq_ignore_ascii_case("Cookie:"))
            {
                line[7..].trim()
            } else {
                line
            };
            // Colons are valid in cookie values (e.g. LOGIN_INFO); add_cookie validates names.
            if !header.contains('=') {
                bail!("Unrecognized cookie input line");
            }
            for pair in header.split(';').filter(|pair| !pair.trim().is_empty()) {
                if let Some((n, v)) = pair.split_once('=') {
                    add_header_cookie(&mut jar, n.trim(), v.trim())?;
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
        visitor_data: None,
        client_version: None,
    })
}
fn add_header_cookie(jar: &mut BTreeMap<String, String>, name: &str, value: &str) -> Result<()> {
    // The upstream paste path forwards an opaque Cookie header. Keep unknown names verbatim,
    // including browser/export artifacts; only exact signing-cookie names authenticate requests.
    if name.is_empty()
        || name
            .bytes()
            .any(|b| !(0x20..=0x7e).contains(&b) || b == b':' || b == b';')
        || value.is_empty()
        || value
            .bytes()
            .any(|b| !(0x21..=0x7e).contains(&b) || b == b';' || b == b',')
    {
        bail!("Invalid Cookie header name or value (paste plain text without extra HTTP headers)");
    }
    if jar.insert(name.to_owned(), value.to_owned()).is_some() {
        bail!("Duplicate cookie name");
    }
    Ok(())
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
    SIGNING.iter().find_map(|name| {
        cookie.split(';').find_map(|p| {
            let (n, v) = p.trim().split_once('=')?;
            (n == *name && !v.is_empty()).then_some(v)
        })
    })
}
fn sapisid_hash(cookie: &str, origin: &str) -> Option<String> {
    let secret = signing_secret(cookie)?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(format!(
        "SAPISIDHASH {ts}_{}",
        format_digest(&Sha1::digest(format!("{ts} {secret} {origin}").as_bytes()))
    ))
}

fn validate_session(s: &mut Session) -> Result<Account> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(WEB_AGENT)
        .build()?;
    validate_session_at(s, &client, MUSIC)
}

fn read_auth_body(response: reqwest::blocking::Response) -> Result<String> {
    if !response.status().is_success() {
        bail!(
            "YouTube sign-in request returned HTTP {}",
            response.status().as_u16()
        );
    }
    let mut body = String::new();
    response
        .take(4 * MAX_INPUT as u64 + 1)
        .read_to_string(&mut body)
        .context("Could not read YouTube sign-in response")?;
    if body.len() > 4 * MAX_INPUT {
        bail!("YouTube sign-in response exceeds 4 MiB");
    }
    Ok(body)
}

fn adopt_shell(s: &mut Session, html: &str) -> Result<()> {
    let mut config = serde_json::Map::new();
    for part in html.split("ytcfg.set").skip(1) {
        let Some(json) = part.trim_start().strip_prefix('(') else {
            continue;
        };
        if let Some(Ok(Value::Object(values))) =
            serde_json::Deserializer::from_str(json.trim_start())
                .into_iter::<Value>()
                .next()
        {
            config.extend(values);
        }
    }
    if config.get("LOGGED_IN").and_then(Value::as_bool) != Some(true) {
        bail!("YouTube Music served a signed-out or unrecognized session page; cookies may be expired, or consent/sign-in is required in the browser");
    }
    let string = |key: &str| {
        config
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let auth_user = config
        .get("SESSION_INDEX")
        .and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .unwrap_or_else(|| "0".into());
    if auth_user.is_empty() || !auth_user.bytes().all(|b| b.is_ascii_digit()) {
        bail!("YouTube returned an invalid session index");
    }
    s.auth_user = auth_user;
    s.page_id = string("DELEGATED_SESSION_ID");
    s.data_sync_id = string("DATASYNC_ID").and_then(|id| {
        let account = id.split("||").next().unwrap_or("");
        (!account.is_empty()).then(|| account.to_owned())
    });
    s.visitor_data = string("VISITOR_DATA");
    s.client_version = string("INNERTUBE_CLIENT_VERSION");
    for value in [
        &s.page_id,
        &s.data_sync_id,
        &s.visitor_data,
        &s.client_version,
    ]
    .into_iter()
    .flatten()
    {
        if value.len() > 4096 || !value.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
            bail!("YouTube returned invalid session metadata");
        }
    }
    Ok(())
}

fn validate_session_at(
    s: &mut Session,
    client: &reqwest::blocking::Client,
    origin: &str,
) -> Result<Account> {
    // Like upstream: the signed-in shell is authoritative. Account-menu details are optional.
    let response = client
        .get(format!("{origin}/"))
        .header("Cookie", &s.cookie)
        .header(
            "Authorization",
            sapisid_hash(&s.cookie, MUSIC).ok_or_else(|| anyhow!("YouTube sign-in expired"))?,
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .context("Could not fetch YouTube Music session page")?;
    adopt_shell(s, &read_auth_body(response)?)?;
    let menu = || -> Result<Option<Account>> {
        let request = client.post(format!(
            "{origin}/youtubei/v1/account/account_menu?prettyPrint=false"
        ));
        let response = s
            .apply(request)
            .json(&json!({"context":s.context()}))
            .send()
            .context("Could not fetch YouTube account details")?;
        let value: Value = serde_json::from_str(&read_auth_body(response)?)
            .context("YouTube returned invalid account details")?;
        Ok(account_from_menu(&value))
    };
    let scope_id = s
        .data_sync_id
        .clone()
        .or_else(|| s.page_id.clone())
        .unwrap_or_default();
    let mut account = match menu().ok().flatten() {
        Some(account) => account,
        None if !scope_id.is_empty() => Account { name: "YouTube Music".into(), ..Default::default() },
        None => bail!("Signed-in page detected, but YouTube did not provide an account identity; retry sign-in"),
    };
    account.scope_id = scope_id;
    Ok(account)
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
        if let Some(text) = v.get("simpleText").and_then(Value::as_str) {
            return text.to_owned();
        }
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
        .as_array()
        .into_iter()
        .flatten()
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
        scope_id: String::new(),
    })
}
fn format_digest(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn commit(mut s: Session, cancel: &AtomicBool, generation: u64) -> Result<Account> {
    s.account = validate_session(&mut s)?;
    commit_validated(s, cancel, generation)
}

fn commit_validated(s: Session, cancel: &AtomicBool, generation: u64) -> Result<Account> {
    let mut state = active()
        .lock()
        .map_err(|_| anyhow!("auth state unavailable"))?;
    if cancel.load(Ordering::SeqCst) || AUTH_GENERATION.load(Ordering::SeqCst) != generation {
        bail!("Sign-in cancelled");
    }
    let mut container = match read_secret()? {
        Some(bytes) => decode_container(&bytes)?,
        None => CredentialContainer::default(),
    };
    let id = account_id(&s.account);
    let entry = StoredAccount {
        id: id.clone(),
        credential: stored_session(&s),
    };
    if let Some(existing) = container.accounts.iter_mut().find(|stored| stored.id == id) {
        *existing = entry;
    } else {
        container.accounts.push(entry);
    }
    container.active_id = Some(id);
    write_container(&container)?;
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
    let profile = AuthProfile::create(auth_dir()?.join(format!(
        "browser-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    )))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(profile.path(), fs::Permissions::from_mode(0o700))?;
    }
    // Google rejects account sign-in when Chrome is launched with remote debugging enabled.
    // Let the user finish an ordinary sign-in first; only then inspect this app-owned profile.
    let mut sign_in = BrowserProcess::spawn(sign_in_command(&exe, profile.path()))
        .context("Could not start sign-in browser")?;
    let wait_result = (|| {
        let deadline = std::time::Instant::now() + Duration::from_secs(600);
        loop {
            check_sign_in_active(&cancel, generation)?;
            if std::time::Instant::now() >= deadline {
                bail!("Sign-in timed out")
            }
            if sign_in.try_wait()?.is_some() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(250));
        }
    })();
    wait_result?;

    let port_file = profile.path().join("DevToolsActivePort");
    let _ = fs::remove_file(&port_file);
    let mut child = BrowserProcess::spawn(capture_command(&exe, profile.path()))
        .context("Could not reopen sign-in profile for secure cookie capture")?;
    let result = (|| {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            check_sign_in_active(&cancel, generation)?;
            if child.try_wait()?.is_some() {
                bail!("Sign-in browser closed before cookie capture")
            }
            if std::time::Instant::now() > deadline {
                bail!("No signed-in YouTube session was found after the sign-in window closed. Try again and complete sign-in before closing it.")
            }
            let endpoint = fs::read_to_string(&port_file)
                .ok()
                .and_then(|content| parse_devtools_active_port(&content));
            if let Some((port, path)) = endpoint {
                let version = client
                    .get(format!("http://127.0.0.1:{port}/json/version"))
                    .send();
                if let Ok(response) = version {
                    if let Ok(value) = response.json::<Value>() {
                        if let Some(ws) = value
                            .get("webSocketDebuggerUrl")
                            .and_then(Value::as_str)
                            .filter(|url| {
                                valid_cdp_url(url, port)
                                    && url.strip_prefix(&format!("ws://127.0.0.1:{port}"))
                                        == Some(path.as_str())
                            })
                        {
                            if let Ok(cookie) = cdp_cookies(ws, &cancel) {
                                if let Ok(mut s) = parse_cookie_input(&cookie) {
                                    if let Ok(account) = validate_session(&mut s) {
                                        s.account = account;
                                        return commit_validated(s, &cancel, generation);
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
    result
}

fn sign_in_command(exe: &Path, profile: &Path) -> Command {
    let mut command = Command::new(exe);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-background-mode")
        .arg("--new-window")
        .arg(MUSIC)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn capture_command(exe: &Path, profile: &Path) -> Command {
    let mut command = Command::new(exe);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--headless=new")
        .arg("--disable-background-mode")
        .arg("--remote-debugging-port=0")
        .arg("--remote-debugging-address=127.0.0.1")
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

struct AuthProfile(PathBuf);
impl AuthProfile {
    fn create(path: PathBuf) -> Result<Self> {
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for AuthProfile {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct BrowserProcess(Child);
impl BrowserProcess {
    fn spawn(mut command: Command) -> std::io::Result<Self> {
        command.spawn().map(Self)
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.0.try_wait()
    }
}
impl Drop for BrowserProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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
        .is_some_and(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
        })
}
fn parse_devtools_active_port(contents: &str) -> Option<(u16, String)> {
    let mut lines = contents.lines();
    let port = lines.next()?.trim().parse::<u16>().ok()?;
    let path = lines.next()?.trim();
    let endpoint = format!("ws://127.0.0.1:{port}{path}");
    (port != 0
        && path
            .strip_prefix("/devtools/browser/")
            .is_some_and(|id| !id.is_empty())
        && valid_cdp_url(&endpoint, port))
    .then(|| (port, path.to_owned()))
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
    f.write_all(netscape_cookies(&s.cookie).as_bytes())?;
    f.sync_all()?;
    Ok(Some(CookieFile { path }))
}
/// yt-dlp rejects a cookies file without the Netscape header ("does not look like a Netscape
/// format cookies file"), which made every signed-in playback fail.
fn netscape_cookies(header: &str) -> String {
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for pair in header.split(';') {
        if let Some((n, v)) = pair.trim().split_once('=') {
            out.push_str(&format!(".youtube.com\tTRUE\t/\tTRUE\t0\t{n}\t{v}\n"));
        }
    }
    out
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
#[cfg(windows)]
pub(crate) fn dpapi(encrypt: bool, input: &[u8]) -> Result<Vec<u8>> {
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
    fn cookie_file_is_netscape_format_for_yt_dlp() {
        assert_eq!(
            netscape_cookies("SID=abc; __Secure-3PSID=x=y;broken; HSID=h"),
            "# Netscape HTTP Cookie File\n\
             .youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc\n\
             .youtube.com\tTRUE\t/\tTRUE\t0\t__Secure-3PSID\tx=y\n\
             .youtube.com\tTRUE\t/\tTRUE\t0\tHSID\th\n"
        );
    }
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
    fn interactive_browser_sign_in_does_not_enable_remote_debugging() {
        let command = sign_in_command(Path::new("chrome.exe"), Path::new("private profile"));
        let args: Vec<_> = command.get_args().collect();
        assert!(args.iter().any(|arg| *arg == MUSIC));
        assert!(!args
            .iter()
            .any(|arg| arg.to_string_lossy().contains("remote-debugging")));

        let capture = capture_command(Path::new("chrome.exe"), Path::new("private profile"));
        let args: Vec<_> = capture.get_args().collect();
        assert!(args.iter().any(|arg| *arg == "--remote-debugging-port=0"));
        assert!(args
            .iter()
            .any(|arg| *arg == "--remote-debugging-address=127.0.0.1"));
        assert!(!args.iter().any(|arg| *arg == MUSIC));
    }
    #[test]
    fn profile_is_removed_when_launch_fails_or_sign_in_is_cancelled() {
        for should_cancel in [false, true] {
            let path = std::env::temp_dir().join(format!(
                "dugem-auth-profile-test-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let result = (|| -> Result<()> {
                let _profile = AuthProfile::create(path.clone())?;
                let cancel = AtomicBool::new(should_cancel);
                if should_cancel {
                    check_sign_in_active(&cancel, AUTH_GENERATION.load(Ordering::SeqCst))?;
                }
                Command::new(path.join("missing-browser.exe"))
                    .spawn()
                    .context("Could not start sign-in browser")?;
                Ok(())
            })();
            assert!(result.is_err());
            assert!(!path.exists());
        }
    }
    #[test]
    fn parses_only_valid_devtools_loopback_endpoint() {
        assert_eq!(
            parse_devtools_active_port("4321\n/devtools/browser/abc\n"),
            Some((4321, "/devtools/browser/abc".into()))
        );
        for bad in [
            "0\n/devtools/browser/abc",
            "65536\n/devtools/browser/abc",
            "4321\n/devtools/browser/",
            "4321\n/devtools/browser/abc?x=1",
            "4321\n/devtools/browser/abc#fragment",
            "4321\n//evil.test/devtools/browser/abc",
            "4321\n/devtools/browser/abc\\evil",
            "4321\n",
        ] {
            assert_eq!(parse_devtools_active_port(bad), None, "{bad:?}");
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
        assert!(!valid_cdp_url(
            "ws://192.168.1.1:4321/devtools/browser/abc",
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
    fn cookie_values_accept_colons_without_accepting_header_injection() {
        for input in [
            "SAPISID=dummy; LOGIN_INFO=dummy:payload",
            "Cookie: SAPISID=dummy; LOGIN_INFO=dummy:payload",
            "cookie: SAPISID=dummy; LOGIN_INFO=dummy:payload;",
        ] {
            let session = parse_cookie_input(input).unwrap();
            assert!(session.cookie.contains("LOGIN_INFO=dummy:payload"));
            assert_eq!(signing_secret(&session.cookie), Some("dummy"));
        }
        for input in [
            "SAPISID=dummy\nInjected: value",
            "SAPISID=dummy\nInjected: name=value",
            "Cookie: SAPISID=dummy\r\nX-Test: name=value",
        ] {
            assert!(parse_cookie_input(input).is_err());
        }
    }
    #[test]
    fn innertube_requests_send_exactly_one_origin() {
        // Google answers HTTP 400 when Origin is repeated; dummy cookies only.
        let session = parse_cookie_input("SAPISID=dummy; SID=dummy").unwrap();
        let body = json!({"browseId":"FEmusic_home"});
        for session in [Some(&session), None] {
            let request = crate::services::innertube_request("browse", &body, session)
                .unwrap()
                .build()
                .unwrap();
            assert_eq!(request.headers().get_all("Origin").iter().count(), 1);
        }
    }
    #[test]
    fn raw_cookie_header_preserves_unknown_names_and_uses_exact_signing_priority() {
        // Dummy browser-header shape, including irrelevant malformed export names; no real secrets.
        let input = "VISITOR_INFO1_LIVE=dummy; **Secure-3PAPISID=artifact; ** Secure-3PSID=artifact; LOGIN_INFO=dummy:payload; __Secure-1PAPISID=first; __Secure-3PAPISID=third";
        let mut session = parse_cookie_input(input).unwrap();
        assert!(session.cookie.contains("** Secure-3PSID=artifact"));
        assert!(session.cookie.contains("LOGIN_INFO=dummy:payload"));
        assert_eq!(signing_secret(&session.cookie), Some("third"));
        let request = session
            .apply(reqwest::blocking::Client::new().post(MUSIC))
            .build()
            .unwrap();
        let authorization = request.headers()["Authorization"].to_str().unwrap();
        let timestamp = authorization
            .strip_prefix("SAPISIDHASH ")
            .unwrap()
            .split('_')
            .next()
            .unwrap();
        assert_eq!(
            authorization,
            format!(
                "SAPISIDHASH {timestamp}_{}",
                format_digest(&Sha1::digest(
                    format!("{timestamp} third {MUSIC}").as_bytes()
                ))
            )
        );
        session.cookie.push_str("; SAPISID=preferred");
        assert_eq!(signing_secret(&session.cookie), Some("preferred"));
        assert!(parse_cookie_input("**Secure-3PAPISID=artifact").is_err());
    }
    #[test]
    fn shell_scope_is_authoritative_and_shared_by_all_signed_requests() {
        let mut session = parse_cookie_input("SAPISID=dummy").unwrap();
        let html = r#"<script>ytcfg.set({"LOGGED_IN":true,"SESSION_INDEX":2,"DATASYNC_ID":"account||rotating-session","DELEGATED_SESSION_ID":"brand","VISITOR_DATA":"visitor"});ytcfg.set({"INNERTUBE_CLIENT_VERSION":"1.20990101.01.00"});</script>"#;
        adopt_shell(&mut session, html).unwrap();
        assert_eq!(session.auth_user, "2");
        assert_eq!(session.data_sync_id.as_deref(), Some("account"));
        let body = session.context();
        assert_eq!(body["user"]["onBehalfOfUser"], "account");
        assert_eq!(body["client"]["visitorData"], "visitor");
        assert_eq!(body["client"]["clientVersion"], "1.20990101.01.00");
        let request = session
            .apply(reqwest::blocking::Client::new().post(MUSIC))
            .build()
            .unwrap();
        assert_eq!(request.headers()["X-Goog-AuthUser"], "2");
        assert_eq!(request.headers()["X-Goog-PageId"], "brand");
        assert_eq!(request.headers()["X-Goog-Visitor-Id"], "visitor");
        assert_eq!(
            request.headers()["X-YouTube-Client-Version"],
            "1.20990101.01.00"
        );
        assert_eq!(request.headers()["User-Agent"], WEB_AGENT);
        adopt_shell(
            &mut session,
            r#"ytcfg.set({"LOGGED_IN":true,"SESSION_INDEX":"1"})"#,
        )
        .unwrap();
        assert!(session.context()["user"].get("onBehalfOfUser").is_none());
        let request = session
            .apply(reqwest::blocking::Client::new().post(MUSIC))
            .build()
            .unwrap();
        assert!(!request.headers().contains_key("X-Goog-PageId"));
        assert!(!request.headers().contains_key("X-Goog-Visitor-Id"));
        for html in [
            r#"ytcfg.set({"LOGGED_IN":false,"DATASYNC_ID":"guest"})"#,
            r#"ytcfg.set({"DATASYNC_ID":"unknown"})"#,
            r#"ytcfg.set({"LOGGED_IN":true,"SESSION_INDEX":"invalid"})"#,
            r#"ytcfg.set({"LOGGED_IN":true,"VISITOR_DATA":"bad\r\nheader"})"#,
        ] {
            assert!(adopt_shell(&mut session, html).is_err());
        }
    }
    #[test]
    fn validation_reads_signed_shell_before_optional_account_menu() {
        use std::net::TcpListener;
        // Only dummy cookies are ever sent, and only to this isolated loopback server.
        for menu_ok in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let server = thread::spawn(move || {
                for step in 0..2 {
                    let until = std::time::Instant::now() + Duration::from_secs(5);
                    let mut stream = loop {
                        if let Ok((s, _)) = listener.accept() {
                            break s;
                        }
                        assert!(
                            std::time::Instant::now() < until,
                            "mock auth request timed out"
                        );
                        thread::sleep(Duration::from_millis(10));
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut header = Vec::new();
                    while !header.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        stream.read_exact(&mut byte).unwrap();
                        header.push(byte[0]);
                        assert!(header.len() < 65536);
                    }
                    let header = String::from_utf8(header).unwrap().to_ascii_lowercase();
                    assert!(header.contains("cookie: "));
                    assert!(header.contains("sapisid=dummy"));
                    assert!(header.contains("authorization: sapisidhash "));
                    let (status, body) = if step == 0 {
                        assert!(header.starts_with("get / http/1.1"));
                        (
                            200,
                            r#"<script>ytcfg.set({"LOGGED_IN":true,"SESSION_INDEX":"3","DATASYNC_ID":"account||session","VISITOR_DATA":"visitor","INNERTUBE_CLIENT_VERSION":"1.20990101.01.00"});</script>"#,
                        )
                    } else {
                        assert!(header.starts_with(
                            "post /youtubei/v1/account/account_menu?prettyprint=false "
                        ));
                        assert!(header.contains("x-goog-authuser: 3\r\n"));
                        assert!(header.contains("x-goog-visitor-id: visitor\r\n"));
                        assert!(header.contains("x-youtube-client-version: 1.20990101.01.00\r\n"));
                        assert!(!header.contains("x-goog-pageid:"));
                        let length: usize = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        let mut body = vec![0; length];
                        stream.read_exact(&mut body).unwrap();
                        let body: Value = serde_json::from_slice(&body).unwrap();
                        assert_eq!(body["context"]["user"]["onBehalfOfUser"], "account");
                        assert_eq!(body["context"]["client"]["visitorData"], "visitor");
                        if menu_ok {
                            (
                                200,
                                r#"{"activeAccountHeaderRenderer":{"accountName":{"simpleText":"Dummy user"},"email":{"simpleText":"dummy@example.invalid"}}}"#,
                            )
                        } else {
                            (503, "{}")
                        }
                    };
                    write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
            });
            let mut session =
                parse_cookie_input("SAPISID=dummy; LOGIN_INFO=dummy:payload").unwrap();
            let client = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap();
            let account = validate_session_at(&mut session, &client, &origin).unwrap();
            server.join().unwrap();
            assert_eq!(account.scope_id, "account");
            assert_eq!(
                account.name,
                if menu_ok {
                    "Dummy user"
                } else {
                    "YouTube Music"
                }
            );
            let stored = StoredAccount {
                id: account_id(&account),
                credential: stored_session(&session),
            };
            let restored = stored.session().unwrap();
            assert_eq!(restored.auth_user, "3");
            assert_eq!(restored.visitor_data.as_deref(), Some("visitor"));
        }
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

    fn fixture(id: &str, email: &str) -> StoredAccount {
        let account = Account {
            name: id.into(),
            email: email.into(),
            channel_id: String::new(),
            thumbnail: String::new(),
            scope_id: String::new(),
        };
        StoredAccount {
            id: account_id(&account),
            credential: Stored {
                cookie: format!("SAPISID={id}-secret"),
                auth_user: "0".into(),
                page_id: Some(format!("page-{id}")),
                data_sync_id: Some(format!("sync-{id}")),
                account,
                visitor_data: None,
                client_version: None,
            },
        }
    }

    #[test]
    fn credential_container_migrates_legacy_and_keeps_multiple_accounts() {
        let first = fixture("first", "one@example.com");
        let mut legacy = serde_json::to_value(&first.credential).unwrap();
        legacy.as_object_mut().unwrap().remove("visitor_data");
        legacy.as_object_mut().unwrap().remove("client_version");
        legacy["account"]
            .as_object_mut()
            .unwrap()
            .remove("scope_id");
        let legacy = serde_json::to_vec(&legacy).unwrap();
        let mut container = decode_container(&legacy).unwrap();
        assert!(container.legacy);
        assert_eq!(container.active_id.as_deref(), Some(first.id.as_str()));
        let second = fixture("second", "two@example.com");
        container.accounts.push(second.clone());
        container.active_id = Some(second.id.clone());
        let bytes = serde_json::to_vec(&container).unwrap();
        let restored = decode_container(&bytes).unwrap();
        assert!(!restored.legacy);
        assert_eq!(restored.accounts.len(), 2);
        assert_eq!(restored.active_id.as_deref(), Some(second.id.as_str()));
        assert_eq!(
            restored.accounts[0].credential.page_id.as_deref(),
            Some("page-first")
        );
        assert_eq!(
            restored.accounts[0].credential.data_sync_id.as_deref(),
            Some("sync-first")
        );
        assert!(!restored.accounts[0].id.contains("secret"));
    }

    #[test]
    fn account_selection_and_removal_helpers_preserve_other_credentials() {
        let first = fixture("first", "one@example.com");
        let second = fixture("second", "two@example.com");
        let mut container = CredentialContainer {
            version: 1,
            active_id: Some(first.id.clone()),
            accounts: vec![first.clone(), second.clone()],
            legacy: false,
        };
        activate_saved(&mut container, &second.id).unwrap();
        assert_eq!(container.active_id.as_deref(), Some(second.id.as_str()));
        assert!(activate_saved(&mut container, "missing").is_err());
        assert_eq!(container.active_id.as_deref(), Some(second.id.as_str()));
        assert!(remove_saved(&mut container, &first.id));
        assert_eq!(container.accounts.len(), 1);
        assert_eq!(container.accounts[0].id, second.id);
        assert_eq!(container.active_id.as_deref(), Some(second.id.as_str()));
        assert!(remove_saved(&mut container, &second.id));
        assert!(container.accounts.is_empty());
        assert_eq!(container.active_id, None);
        assert!(!remove_saved(&mut container, "missing"));
    }

    #[test]
    fn signing_out_removes_only_the_active_credential() {
        let first = fixture("first", "one@example.com");
        let second = fixture("second", "two@example.com");
        let mut container = CredentialContainer {
            version: 1,
            active_id: Some(first.id.clone()),
            accounts: vec![first, second.clone()],
            legacy: false,
        };
        sign_out_saved(&mut container);
        assert_eq!(container.active_id, None);
        assert_eq!(container.accounts.len(), 1);
        assert_eq!(container.accounts[0].id, second.id);
        sign_out_saved(&mut container);
        assert_eq!(container.accounts.len(), 1);
    }

    #[test]
    fn corrupt_or_unsupported_containers_are_rejected() {
        assert!(decode_container(b"not json").is_err());
        assert!(decode_container(br#"{"version":99,"active_id":null,"accounts":[]}"#).is_err());
        let mut duplicate = CredentialContainer {
            version: 1,
            active_id: None,
            accounts: vec![
                fixture("one", "same@example.com"),
                fixture("two", "same@example.com"),
            ],
            legacy: false,
        };
        duplicate.accounts[1].id = duplicate.accounts[0].id.clone();
        assert!(decode_container(&serde_json::to_vec(&duplicate).unwrap()).is_err());
    }
}
