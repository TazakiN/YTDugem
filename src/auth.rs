//! YouTube Music sign-in, after Meiro (https://github.com/elianiva/meiro, GPL-3.0): `signin.go`,
//! `account.go`, `internal/cookies`, `internal/secretfile` and `youtube/accounts.go`.
//!
//! YouTube Music lets no other app sign in to it: the account's pages are served only to a
//! request that carries a signed-in browser's cookies. So the session is taken from a browser —
//! a throwaway window opened for the purpose, the user's own browser, or a pasted Cookie header
//! — and the account and channel to act as are chosen from those cookies.
//!
//! One session is kept: the cookie, where it came from and whom it signs in as, sealed in a file
//! whose key the system credential store holds. When YouTube retires the cookie, a session
//! imported from a browser is read from that browser again; any other asks the user to sign in.

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
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tungstenite::{client::IntoClientRequest, Message};

const MAX_INPUT: usize = 1024 * 1024;
const MUSIC: &str = "https://music.youtube.com";
const SIGNING: &[&str] = &["SAPISID", "__Secure-3PAPISID", "__Secure-1PAPISID"];
const WEB_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
const WEB_VERSION: &str = "1.20260601.01.00";

/// The Google sign-in page that returns to YouTube Music, in the form of youtube.com's own Sign in
/// link: the `youtube.com/signin` handler in `continue` is what sets the YouTube cookies. Opening
/// it puts the sign-in form in front of the user at once (Meiro `signInURL`).
const SIGN_IN_URL: &str = "https://accounts.google.com/ServiceLogin?service=youtube&uilel=3&continue=https%3A%2F%2Fwww.youtube.com%2Fsignin%3Faction_handle_signin%3Dtrue%26app%3Ddesktop%26next%3Dhttps%253A%252F%252Fmusic.youtube.com%252F";
/// How long the sign-in window may stay open: time to type a password and a second factor.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// How long the window stays open after the session shows, so the page can set the rest of its
/// cookies.
const SETTLE: Duration = Duration::from_secs(3);

/// Whom a session signs in as, from YouTube's account menu.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub email: String,
    /// The channel handle, such as `@name`.
    pub channel_id: String,
    pub thumbnail: String,
}

/// An account or channel the cookies can act as (Meiro `AccountChannel`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Channel {
    /// The Google `authuser` index; brand channels share their owner's.
    pub account_index: u32,
    pub name: String,
    pub email: String,
    pub byline: String,
    pub handle: String,
    /// The brand channel's page ID, empty for an account's own channel.
    pub channel_id: String,
    pub thumbnail: String,
    pub selected: bool,
    pub disabled: bool,
}

impl Channel {
    pub fn label(&self) -> &str {
        [&self.name, &self.handle]
            .into_iter()
            .find(|s| !s.is_empty())
            .map_or("that channel", String::as_str)
    }
    /// The second line of an account row: the email, else the handle, else the byline.
    pub fn detail(&self) -> &str {
        [&self.email, &self.handle, &self.byline]
            .into_iter()
            .find(|s| !s.is_empty())
            .map_or("", String::as_str)
    }
}

/// What a sign-in remembers to sign in again with (Meiro `sessionSource`): the account and
/// channel it acts as, the email that account must answer with, and the browser it came from —
/// empty for a pasted session or the sign-in window, which nothing can be read from again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Source {
    #[serde(default)]
    pub account_index: u32,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub browser: String,
    #[serde(default)]
    pub email: String,
}

/// A signed-in session: the cookie it signs in with, where it came from, and whom it is.
#[derive(Clone)]
pub struct Session {
    cookie: String,
    source: Source,
    account: Account,
    channels: Vec<Channel>,
}

static ACTIVE: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
fn active() -> &'static Mutex<Option<Session>> {
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// Bumped by every sign-in and sign-out, so work that started before one cannot undo it.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// A cookie YouTube no longer signs anyone in with (Meiro `errSessionGone`).
#[derive(Debug)]
pub struct SessionGone;
impl std::fmt::Display for SessionGone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Your session expired. Sign in again.")
    }
}
impl std::error::Error for SessionGone {}

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
            .header("X-Goog-AuthUser", self.source.account_index.to_string())
            .header("X-Origin", origin)
            .header("Origin", origin)
            .header("Referer", format!("{origin}/"))
            .header("User-Agent", WEB_AGENT)
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("X-YouTube-Client-Name", "67")
            .header("X-YouTube-Client-Version", WEB_VERSION);
        if self.source.channel.is_empty() {
            request
        } else {
            request.header("X-Goog-PageId", &self.source.channel)
        }
    }
    pub fn context(&self) -> Value {
        let mut value = json!({"client":{"clientName":"WEB_REMIX","clientVersion":WEB_VERSION,"hl":"en","gl":"US"},"user":{"lockedSafetyMode":false},"request":{"useSsl":true}});
        if !self.source.channel.is_empty() {
            value["user"]["onBehalfOfUser"] = json!(self.source.channel);
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
/// The accounts and channels the session's cookies can act as, and which one is in use.
pub fn current_channels() -> (Vec<Channel>, Source) {
    active()
        .lock()
        .ok()
        .and_then(|s| s.as_ref().map(|s| (s.channels.clone(), s.source.clone())))
        .unwrap_or_default()
}
/// A token for the current sign-in, to check before acting on work started under it.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

// ---- Storage (Meiro `secretfile`) -------------------------------------------------------------

/// The saved sign-in. Its account is kept so the window shows who is signed in at once; it is
/// checked against YouTube in the background.
#[derive(Clone, Serialize, Deserialize)]
struct Saved {
    version: u32,
    cookie: String,
    source: Source,
    #[serde(default)]
    account: Account,
}

/// Puts back the saved sign-in without asking YouTube, so the app starts signed in at once;
/// [`renew`] checks it afterwards. A session saved by an older version is moved to the new
/// storage on the way.
pub fn restore() -> Result<Option<Account>> {
    let generation = generation();
    let Some(saved) = secretfile::load()? else {
        return Ok(None);
    };
    if signing_secret(&saved.cookie).is_none() || saved.cookie.contains(['\r', '\n']) {
        bail!("The saved sign-in is invalid");
    }
    let mut state = active()
        .lock()
        .map_err(|_| anyhow!("auth state unavailable"))?;
    if self::generation() != generation {
        bail!("Sign-in changed while restoring the saved session");
    }
    *state = Some(Session {
        cookie: saved.cookie,
        source: saved.source,
        account: saved.account.clone(),
        channels: vec![],
    });
    Ok(Some(saved.account))
}

/// Checks the saved session with YouTube (Meiro `renewSession`). When YouTube no longer
/// recognises it and it came from a browser, the session is read from that browser again —
/// the one place that still holds a session YouTube accepts — and kept instead. Fails with
/// [`SessionGone`] when there is nothing left to sign in with; the session is then forgotten.
pub fn renew() -> Result<Account> {
    let generation = generation();
    let current = snapshot().ok_or_else(|| anyhow!("Not signed in"))?;
    let opened = match open_session(&current.cookie, &current.source) {
        Err(e) if e.is::<SessionGone>() && !current.source.browser.is_empty() => {
            import_browser(&current.source.browser)
                .and_then(|fresh| open_session(&fresh, &current.source))
                .map_err(|_| anyhow::Error::new(SessionGone))
        }
        other => other,
    };
    match opened {
        Ok(session) => commit(session, generation),
        Err(e) => {
            if e.is::<SessionGone>() && self::generation() == generation {
                let _ = logout();
            }
            Err(e)
        }
    }
}

/// Signs in with `cookie` as `source` says: asks YouTube whom it is, keeps it, and uses it.
/// `generation` is [`generation`] when the sign-in began: a sign-in or sign-out since wins.
pub fn sign_in(cookie: &str, source: &Source, generation: u64) -> Result<Account> {
    let session = open_session(cookie, source)?;
    commit(session, generation)
}

/// Acts as another of the session's accounts or channels.
pub fn switch_channel(channel: &Channel) -> Result<Account> {
    let generation = generation();
    let current = snapshot().ok_or_else(|| anyhow!("Not signed in"))?;
    let source = Source {
        account_index: channel.account_index,
        channel: channel.channel_id.clone(),
        browser: current.source.browser.clone(),
        email: channel.email.clone(),
    };
    sign_in(&current.cookie, &source, generation)
}

/// Makes a sign-in still on its way fail instead of saving its session, for a dialog the user
/// dismissed (Meiro `dismissSignIn`). The session in use is kept.
pub fn cancel_sign_in() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Forgets the session and removes the saved copy.
pub fn logout() -> Result<()> {
    let mut state = active()
        .lock()
        .map_err(|_| anyhow!("auth state unavailable"))?;
    secretfile::delete()?;
    *state = None;
    GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

fn commit(session: Session, generation: u64) -> Result<Account> {
    let mut state = active()
        .lock()
        .map_err(|_| anyhow!("auth state unavailable"))?;
    if self::generation() != generation {
        bail!("Sign-in cancelled");
    }
    secretfile::save(&Saved {
        version: 1,
        cookie: session.cookie.clone(),
        source: session.source.clone(),
        account: session.account.clone(),
    })?;
    let account = session.account.clone();
    *state = Some(session);
    GENERATION.fetch_add(1, Ordering::SeqCst);
    Ok(account)
}

mod secretfile {
    //! The session, sealed with AES-256-GCM under a random key that the system credential
    //! store (Windows Credential Manager) holds, as Meiro's `secretfile` does: the store keeps
    //! only small secrets, and a browser session is larger. Without a store (other systems),
    //! the file holds the session in plain text, readable by its user only.
    use super::*;

    #[cfg(windows)]
    const KEY_NAME: &str = "YTDugem/session-key";
    const SEALED: &[u8] = b"YTDS1";

    fn path() -> Result<PathBuf> {
        Ok(auth_dir()?.join("session.json"))
    }

    pub(super) fn load() -> Result<Option<Saved>> {
        let path = path()?;
        let Ok(bytes) = fs::read(&path) else {
            return Ok(None);
        };
        if bytes.len() > 4 * MAX_INPUT {
            bail!("The saved sign-in is too large");
        }
        if let Some(sealed) = bytes.strip_prefix(SEALED) {
            let plain = open(sealed)?;
            return Ok(Some(
                serde_json::from_slice(&plain).context("The saved sign-in is invalid")?,
            ));
        }
        if let Ok(saved) = serde_json::from_slice::<Saved>(&bytes) {
            return Ok(Some(saved));
        }
        // Saved by an older version: protected with DPAPI as a list of accounts.
        #[cfg(windows)]
        let bytes = super::dpapi(false, &bytes)?;
        let legacy: Value =
            serde_json::from_slice(&bytes).context("The saved sign-in is invalid")?;
        let Some(saved) = migrate(&legacy) else {
            return Ok(None);
        };
        save(&saved)?;
        Ok(Some(saved))
    }

    /// The active account of an older version's credential container (or its single-account
    /// predecessor), as a [`Saved`] session.
    pub(super) fn migrate(legacy: &Value) -> Option<Saved> {
        let entry = match legacy["accounts"].as_array() {
            Some(accounts) => {
                let id = legacy["active_id"].as_str()?;
                accounts.iter().find(|a| a["id"].as_str() == Some(id))?
            }
            None => legacy,
        };
        let cookie = entry["cookie"].as_str()?.to_owned();
        Some(Saved {
            version: 1,
            cookie,
            source: Source {
                account_index: entry["auth_user"]
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                channel: entry["page_id"].as_str().unwrap_or("").to_owned(),
                ..Default::default()
            },
            account: serde_json::from_value(entry["account"].clone()).unwrap_or_default(),
        })
    }

    pub(super) fn save(saved: &Saved) -> Result<()> {
        let plain = serde_json::to_vec(saved)?;
        let bytes = match keychain::key()? {
            Some(key) => [SEALED, &seal(&key, &plain)?].concat(),
            None => plain,
        };
        let path = path()?;
        let tmp = path.with_extension("tmp");
        {
            let mut file = fs::File::create(&tmp)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub(super) fn delete() -> Result<()> {
        match fs::remove_file(path()?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub(super) fn seal(key: &[u8; 32], plain: &[u8]) -> Result<Vec<u8>> {
        use aes_gcm::{
            aead::{Aead, Generate, Nonce},
            Aes256Gcm, KeyInit,
        };
        let cipher = Aes256Gcm::new(key.into());
        // A fresh random 96-bit nonce from the OS CSPRNG (aes-gcm's `getrandom` feature).
        let nonce = Nonce::<Aes256Gcm>::try_generate()
            .map_err(|_| anyhow!("Could not encrypt the sign-in"))?;
        let sealed = cipher
            .encrypt(&nonce, plain)
            .map_err(|_| anyhow!("Could not encrypt the sign-in"))?;
        Ok([nonce.as_slice(), &sealed].concat())
    }

    fn open(sealed: &[u8]) -> Result<Vec<u8>> {
        let key = keychain::key()?.ok_or_else(|| {
            anyhow!("The key that opens the saved sign-in is missing; sign in again")
        })?;
        open_with(&key, sealed)
    }

    pub(super) fn open_with(key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>> {
        use aes_gcm::{
            aead::{Aead, Nonce},
            Aes256Gcm, KeyInit,
        };
        if sealed.len() < 12 {
            bail!("The saved sign-in is damaged");
        }
        let (nonce, body) = sealed.split_at(12);
        let nonce = Nonce::<Aes256Gcm>::try_from(nonce)
            .map_err(|_| anyhow!("The saved sign-in is damaged"))?;
        Aes256Gcm::new(key.into())
            .decrypt(&nonce, body)
            .map_err(|_| anyhow!("The saved sign-in could not be opened; sign in again"))
    }

    mod keychain {
        //! The session key in Windows Credential Manager, made on first use.
        #[cfg(windows)]
        pub(in super::super) fn key() -> anyhow::Result<Option<[u8; 32]>> {
            if let Some(bytes) = read(super::KEY_NAME)? {
                if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
                    return Ok(Some(key));
                }
            }
            let key: [u8; 32] = rand::random();
            write(super::KEY_NAME, &key)?;
            Ok(Some(key))
        }
        #[cfg(not(windows))]
        pub(in super::super) fn key() -> anyhow::Result<Option<[u8; 32]>> {
            Ok(None)
        }

        #[cfg(windows)]
        fn wide(s: &str) -> Vec<u16> {
            s.encode_utf16().chain(Some(0)).collect()
        }

        #[cfg(windows)]
        fn read(name: &str) -> anyhow::Result<Option<Vec<u8>>> {
            use windows_sys::Win32::{
                Foundation::{GetLastError, ERROR_NOT_FOUND},
                Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC},
            };
            let target = wide(name);
            let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
            // SAFETY: `target` is NUL-terminated and outlives the call; on success the API
            // hands back a buffer this function frees with `CredFree` after copying it.
            unsafe {
                if CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) == 0 {
                    if GetLastError() == ERROR_NOT_FOUND {
                        return Ok(None);
                    }
                    anyhow::bail!("Windows Credential Manager could not be read");
                }
                let c = &*credential;
                let bytes =
                    std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize)
                        .to_vec();
                CredFree(credential as *const std::ffi::c_void);
                Ok(Some(bytes))
            }
        }

        #[cfg(windows)]
        fn write(name: &str, value: &[u8]) -> anyhow::Result<()> {
            use windows_sys::Win32::Security::Credentials::{
                CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
            };
            let mut target = wide(name);
            let mut user = wide("YTDugem");
            // SAFETY: every pointer in `credential` points into a buffer that outlives the
            // call; the API copies what it keeps.
            unsafe {
                let mut credential: CREDENTIALW = std::mem::zeroed();
                credential.Type = CRED_TYPE_GENERIC;
                credential.TargetName = target.as_mut_ptr();
                credential.UserName = user.as_mut_ptr();
                credential.CredentialBlobSize = value.len() as u32;
                credential.CredentialBlob = value.as_ptr() as *mut u8;
                credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
                if CredWriteW(&credential, 0) == 0 {
                    anyhow::bail!("Windows Credential Manager could not save the key");
                }
            }
            Ok(())
        }

        #[cfg(all(test, windows))]
        mod tests {
            #[test]
            fn credential_manager_keeps_a_key_until_it_is_deleted() {
                use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
                let name = format!("YTDugem/test-key-{}", std::process::id());
                assert_eq!(super::read(&name).unwrap(), None);
                super::write(&name, &[1, 2, 3]).unwrap();
                assert_eq!(super::read(&name).unwrap(), Some(vec![1, 2, 3]));
                let target = super::wide(&name);
                // SAFETY: `target` is NUL-terminated and outlives the call.
                assert_ne!(
                    unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) },
                    0
                );
                assert_eq!(super::read(&name).unwrap(), None);
            }
        }
    }
}

// ---- Asking YouTube ---------------------------------------------------------------------------

fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(WEB_AGENT)
        .build()?)
}

/// Signs in with `cookie` as `source.channel`, and falls back to the account's own channel when
/// that one is gone (Meiro `openSession`). YouTube answers a retired cookie with signed-out
/// content rather than an error, so an account without a name is none.
fn open_session(cookie: &str, source: &Source) -> Result<Session> {
    let open = |channel: &str| -> Result<Session> {
        let mut session = Session {
            cookie: cookie.to_owned(),
            source: Source {
                channel: channel.to_owned(),
                ..source.clone()
            },
            account: Account::default(),
            channels: vec![],
        };
        session.account = account_details(&session)?;
        // Optional: the switcher only adds the channel menu, and the email when the account
        // menu leaves it out (it does for every account, as of 2026-11).
        session.channels = sign_in_accounts_as(cookie, source.account_index).unwrap_or_default();
        if session.account.email.is_empty() {
            session.account.email =
                switcher_email(&session.channels, source.account_index).unwrap_or_default();
        }
        if !source.email.is_empty() {
            if session.account.email.is_empty() {
                bail!("YouTube returned no email address for the selected Google account; YTDugem could not verify your selection");
            }
            if !session.account.email.eq_ignore_ascii_case(&source.email) {
                bail!("YouTube returned a different Google account than the one you selected; YTDugem did not save the session");
            }
        }
        Ok(session)
    };
    match open(&source.channel) {
        Err(e) if !source.channel.is_empty() && !e.is::<SessionGone>() => open(""),
        other => other,
    }
}

/// The email of the account the switcher, asked as `account_index`, marks as the one in use.
fn switcher_email(channels: &[Channel], account_index: u32) -> Option<String> {
    channels
        .iter()
        .find(|c| c.selected && c.account_index == account_index && !c.email.is_empty())
        .map(|c| c.email.clone())
}

fn account_details(session: &Session) -> Result<Account> {
    let response = session
        .apply(http()?.post(format!(
            "{MUSIC}/youtubei/v1/account/account_menu?prettyPrint=false"
        )))
        .json(&json!({"context": session.context()}))
        .send()
        .context("Could not reach YouTube Music")?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(SessionGone.into());
    }
    let value: Value = serde_json::from_str(&read_auth_body(response)?)
        .context("YouTube returned invalid account details")?;
    account_from_menu(&value).ok_or_else(|| SessionGone.into())
}

/// The accounts and channels these cookies can act as — other Google accounts signed in to the
/// same browser included — from YouTube's web account switcher (Meiro `GetSignInAccounts`).
pub fn sign_in_accounts(cookie: &str) -> Result<Vec<Channel>> {
    let channels = sign_in_accounts_as(cookie, 0)?;
    if channels.is_empty() {
        bail!("YouTube did not return any accounts; sign in at music.youtube.com in that browser, then try again");
    }
    Ok(channels)
}

fn sign_in_accounts_as(cookie: &str, account_index: u32) -> Result<Vec<Channel>> {
    const ORIGIN: &str = "https://www.youtube.com";
    let response = http()?
        .get(format!("{ORIGIN}/getAccountSwitcherEndpoint"))
        .header("Cookie", cookie)
        .header(
            "Authorization",
            sapisid_hash(cookie, ORIGIN).ok_or_else(|| anyhow!("The cookie has no SAPISID"))?,
        )
        .header("X-Goog-AuthUser", account_index.to_string())
        .header("X-Origin", ORIGIN)
        .header("Accept", "application/json")
        .send()
        .context("Could not reach YouTube")?;
    let body = read_auth_body(response)?;
    // The endpoint prefixes its JSON with an XSSI guard.
    let body = body.trim().trim_start_matches(")]}'").trim();
    let value: Value =
        serde_json::from_str(body).context("YouTube's account switcher returned invalid JSON")?;
    Ok(extract_channels(&value, account_index))
}

fn read_auth_body(response: reqwest::blocking::Response) -> Result<String> {
    if !response.status().is_success() {
        bail!("YouTube answered HTTP {}", response.status().as_u16());
    }
    let mut body = String::new();
    response
        .take(4 * MAX_INPUT as u64 + 1)
        .read_to_string(&mut body)
        .context("Could not read YouTube's answer")?;
    if body.len() > 4 * MAX_INPUT {
        bail!("YouTube's answer exceeds 4 MiB");
    }
    Ok(body)
}

/// Every account and channel row in an account-switcher response, once each (Meiro
/// `extractAccountChannels`). The email of a Google account's header applies to the rows under it.
fn extract_channels(root: &Value, default_index: u32) -> Vec<Channel> {
    fn has_fields(v: &Value) -> bool {
        [
            "channelId",
            "accountName",
            "account_name",
            "channelHandle",
            "channel_handle",
        ]
        .iter()
        .any(|k| v.get(k).is_some())
    }
    fn walk(v: &Value, email: &str, default_index: u32, out: &mut Vec<Channel>) {
        match v {
            Value::Array(items) => items
                .iter()
                .for_each(|x| walk(x, email, default_index, out)),
            Value::Object(map) => {
                let header = &v["header"]["googleAccountHeaderRenderer"]["email"];
                let own = renderer_text(header);
                let email = if own.is_empty() { email } else { own.as_str() };
                if has_fields(v) {
                    push(out, parse_channel(v, default_index), email);
                    return;
                }
                for (key, child) in map {
                    if child.is_object()
                        && (key == "accountItemRenderer"
                            || key == "accountItem"
                            || has_fields(child))
                    {
                        push(out, parse_channel(child, default_index), email);
                    } else {
                        walk(child, email, default_index, out);
                    }
                }
            }
            _ => {}
        }
    }
    fn push(out: &mut Vec<Channel>, mut channel: Channel, email: &str) {
        channel.email = email.to_owned();
        if channel.channel_id.is_empty() && channel.name.is_empty() && channel.handle.is_empty() {
            return;
        }
        let same = |c: &Channel| {
            c.account_index == channel.account_index
                && c.channel_id == channel.channel_id
                && c.name == channel.name
                && c.handle == channel.handle
        };
        if !out.iter().any(same) {
            out.push(channel);
        }
    }
    let mut out = vec![];
    walk(root, "", default_index, &mut out);
    out
}

fn parse_channel(v: &Value, default_index: u32) -> Channel {
    let text = |keys: &[&str]| {
        keys.iter()
            .map(|k| renderer_text(&v[*k]))
            .find(|s| !s.is_empty())
            .unwrap_or_default()
    };
    let photo = |key: &str| {
        v[key]["thumbnails"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .find_map(|t| t["url"].as_str())
            .unwrap_or("")
            .to_owned()
    };
    let flag = |key: &str| v[key].as_bool().unwrap_or(false);
    let endpoint = &v["serviceEndpoint"];
    let channel_id = [
        v["endpoint"]["browseEndpoint"]["browseId"].as_str(),
        v["channelId"].as_str(),
        page_id(endpoint),
    ]
    .into_iter()
    .flatten()
    .find(|s| !s.is_empty())
    .unwrap_or("")
    .to_owned();
    let index = signin_index(endpoint, default_index);
    let thumbnail = [photo("accountPhoto"), photo("account_photo")]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default();
    Channel {
        account_index: index.unwrap_or(0),
        name: text(&["accountName", "account_name", "name"]),
        email: String::new(),
        byline: text(&["accountByline", "account_byline"]),
        handle: text(&["channelHandle", "handle", "channel_handle"]),
        channel_id,
        thumbnail,
        selected: flag("isSelected"),
        disabled: flag("isDisabled") || index.is_none(),
    }
}

/// The `authuser` of a row's sign-in token. Brand channels share their account's index, so the
/// position in the list means nothing. A row without a token is the current account; one with
/// an unreadable token is `None`, and must not select account zero.
fn signin_index(endpoint: &Value, default_index: u32) -> Option<u32> {
    let tokens = endpoint["selectActiveIdentityEndpoint"]["supportedTokens"].as_array();
    let Some(url) = tokens
        .into_iter()
        .flatten()
        .find_map(|t| t.get("accountSigninToken"))
    else {
        return Some(default_index);
    };
    let url = url["signinUrl"].as_str()?;
    let query = url.split_once('?')?.1;
    let mut values = query.split('&').filter_map(|p| p.strip_prefix("authuser="));
    let value = values.next()?;
    if values.next().is_some() || value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// A brand channel's page ID, from its identity token; the account's own channel has none.
fn page_id(endpoint: &Value) -> Option<&str> {
    endpoint["selectActiveIdentityEndpoint"]["supportedTokens"]
        .as_array()?
        .iter()
        .find_map(|t| t["pageIdToken"]["pageId"].as_str())
}

fn renderer_text(v: &Value) -> String {
    if let Some(text) = v.as_str() {
        return text.to_owned();
    }
    if let Some(text) = v.get("simpleText").and_then(Value::as_str) {
        return text.to_owned();
    }
    v.get("runs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| r.get("text").and_then(Value::as_str))
        .collect()
}

// ---- Where a session comes from ---------------------------------------------------------------

/// The value of a Cookie header out of what the user pasted: the value alone, the header with
/// its name, or a whole cURL command (Meiro `cookies.Header`). Exported cookie files and JSON
/// still go through [`parse_cookie_input`].
pub fn pasted_cookie(input: &str) -> Result<String> {
    let text = input.trim();
    let lower = text.to_ascii_lowercase();
    let header = match lower.find("cookie:") {
        Some(at) => {
            let rest = text[at + 7..].trim_start();
            let end = rest.find(['\'', '"', '\n']).unwrap_or(rest.len());
            rest[..end].trim()
        }
        None => text,
    };
    let cookie = parse_cookie_input(header)
        .or_else(|_| parse_cookie_input(text))
        .map_err(|_| {
            anyhow!("That is not the cookie of a signed-in session: it has no SAPISID. Copy the whole value of the Cookie header.")
        })?
        .cookie;
    Ok(cookie)
}

/// A browser the session can be imported from, through yt-dlp's `--cookies-from-browser`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Browser {
    pub label: &'static str,
    spec: &'static str,
    /// Where its profiles live, under `%LOCALAPPDATA%` (Chromium) or `%APPDATA%` (Firefox).
    profiles: &'static str,
    firefox: bool,
    /// yt-dlp knows the browser by `spec` alone. A fork it does not know (Zen) is read through its
    /// engine's `spec` plus a profile path only: the bare `spec` would name another browser.
    bare: bool,
}

const BROWSERS: [Browser; 5] = [
    Browser {
        label: "Chrome",
        spec: "chrome",
        profiles: "Google/Chrome/User Data",
        firefox: false,
        bare: true,
    },
    Browser {
        label: "Edge",
        spec: "edge",
        profiles: "Microsoft/Edge/User Data",
        firefox: false,
        bare: true,
    },
    Browser {
        label: "Brave",
        spec: "brave",
        profiles: "BraveSoftware/Brave-Browser/User Data",
        firefox: false,
        bare: true,
    },
    Browser {
        label: "Firefox",
        spec: "firefox",
        profiles: "Mozilla/Firefox/Profiles",
        firefox: true,
        bare: true,
    },
    Browser {
        label: "Zen",
        spec: "firefox",
        profiles: "zen/Profiles",
        firefox: true,
        bare: false,
    },
];

impl Browser {
    fn root(&self) -> Option<PathBuf> {
        let base = if self.firefox {
            "APPDATA"
        } else {
            "LOCALAPPDATA"
        };
        std::env::var_os(base).map(|b| PathBuf::from(b).join(self.profiles))
    }
    /// The yt-dlp selector of each profile, the default first.
    fn selectors(&self) -> Vec<String> {
        self.selectors_in(self.root())
    }
    fn selectors_in(&self, root: Option<PathBuf>) -> Vec<String> {
        let mut out: Vec<String> = self
            .bare
            .then(|| self.spec.to_owned())
            .into_iter()
            .collect();
        let Some(root) = root else { return out };
        let Ok(entries) = fs::read_dir(root) else {
            return out;
        };
        let mut profiles: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if self.firefox {
                    p.join("cookies.sqlite").is_file()
                } else {
                    (name == "Default" || name.starts_with("Profile "))
                        && (p.join("Network/Cookies").is_file() || p.join("Cookies").is_file())
                }
            })
            .collect();
        profiles.sort();
        out.extend(
            profiles
                .iter()
                .map(|p| format!("{}:{}", self.spec, p.display())),
        );
        out
    }
}

/// The browsers on this machine to offer for an import.
pub fn browsers() -> Vec<Browser> {
    BROWSERS
        .into_iter()
        .filter(|b| {
            if cfg!(windows) {
                b.root().is_some_and(|r| r.is_dir())
            } else {
                // Elsewhere only browsers yt-dlp finds by itself; the profile roots are Windows ones.
                b.bare
            }
        })
        .collect()
}

/// Reads the signed-in YouTube session of a browser by its label, trying each of its profiles in
/// turn (Meiro `firstSession`). Chrome on Windows encrypts its cookies with a key only it can
/// use; then the error says so and the dialog points at pasting the Cookie header instead.
pub fn import_browser(label: &str) -> Result<String> {
    let browser = BROWSERS
        .into_iter()
        .find(|b| b.label == label)
        .ok_or_else(|| anyhow!("{label} is not a browser YTDugem can import from"))?;
    let mut last = None;
    for selector in browser.selectors() {
        match yt_dlp_cookie(&selector) {
            Ok(cookie) if signing_secret(&cookie).is_some() => return Ok(cookie),
            Ok(_) => {}
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        anyhow!(
            "{label} has no signed-in YouTube session. Sign in at music.youtube.com there first."
        )
    }))
}

/// Has yt-dlp write one browser profile's cookies to a private file, then reads the YouTube ones
/// that have not expired. yt-dlp needs a URL to get as far as loading them, and saves them when
/// it ends, whether or not it found anything to download (Meiro `ytdlp.BrowserCookie`).
fn yt_dlp_cookie(selector: &str) -> Result<String> {
    let dir = AuthProfile::create(auth_dir()?.join(format!(
        "import-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    )))?;
    let file = dir.path().join("cookies.txt");
    let program = std::env::var_os("BITCHORD_YTDLP").unwrap_or_else(|| "yt-dlp".into());
    let mut command = Command::new(program);
    command
        .arg("--cookies-from-browser")
        .arg(selector)
        .arg("--cookies")
        .arg(&file)
        .args([
            "--skip-download",
            "--no-warnings",
            "--no-playlist",
            "--playlist-items",
            "0",
            "https://music.youtube.com/",
        ]);
    let output = crate::services::run(&mut command, Duration::from_secs(90))?;
    if let Ok(text) = fs::read_to_string(&file) {
        let cookie = from_netscape(&text, SystemTime::now());
        if !cookie.is_empty() {
            return Ok(cookie);
        }
    }
    let log = String::from_utf8_lossy(&output.stderr);
    let reason = log
        .lines()
        .filter_map(|l| l.trim().strip_prefix("ERROR:"))
        .next_back()
        .map(str::trim);
    match reason {
        Some(r) if r.contains("DPAPI") || r.contains("app-bound") || r.contains("decrypt") => {
            bail!("{selector} keeps its cookies encrypted so that only it can read them ({r})")
        }
        Some(r) => bail!("{r}"),
        None => Ok(String::new()),
    }
}

/// The Cookie header a request to music.youtube.com carries, from a cookies.txt file: the
/// cookies of youtube.com and its subdomains that have not expired. A later one of a name wins.
fn from_netscape(text: &str, now: SystemTime) -> String {
    let now = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut jar: Vec<(String, String)> = vec![];
    for line in text.lines() {
        let line = line.trim().trim_start_matches("#HttpOnly_");
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 7 || !allowed_domain(fields[0]) {
            continue;
        }
        let expires: u64 = fields[4].parse().unwrap_or(0);
        if expires > 0 && expires < now {
            continue;
        }
        let (name, value) = (fields[5].to_owned(), fields[6].to_owned());
        match jar.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value,
            None => jar.push((name, value)),
        }
    }
    jar.iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Has the user sign in to YouTube Music in a browser window of its own, and returns the Cookie
/// header of that session (Meiro `SignInWindow`). The window is a new, empty profile, so it holds
/// nothing of the user's own browsing, and it is closed and deleted as soon as the session is
/// read: YouTube replaces the cookies of a session as long as a browser keeps it open, so one
/// read from a window that is then closed does not go stale the way an everyday browser's does.
pub fn window_sign_in(cancel: &AtomicBool) -> Result<String> {
    let exe = find_chromium_browser().ok_or_else(|| {
        anyhow!("No Chrome, Edge or Brave browser was found for the sign-in window")
    })?;
    let profile = AuthProfile::create(auth_dir()?.join(format!(
        "signin-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    )))?;
    let mut browser = BrowserProcess::spawn(window_command(&exe, profile.path()))
        .context("Could not start the sign-in window")?;
    let deadline = Instant::now() + SIGN_IN_TIMEOUT;
    let port_file = profile.path().join("DevToolsActivePort");
    let mut endpoint = None;
    let mut signed_at: Option<Instant> = None;
    loop {
        if cancel.load(Ordering::SeqCst) {
            bail!("Sign-in cancelled");
        }
        if Instant::now() >= deadline {
            bail!("The sign-in window stayed open too long; try again");
        }
        if browser.try_wait()?.is_some() {
            bail!("The sign-in window was closed before you finished signing in");
        }
        if endpoint.is_none() {
            endpoint = fs::read_to_string(&port_file)
                .ok()
                .and_then(|c| parse_devtools_active_port(&c))
                .map(|(port, path)| format!("ws://127.0.0.1:{port}{path}"));
        }
        if let Some(url) = &endpoint {
            if let Ok(header) = cdp_cookies(url, cancel) {
                if signing_secret(&header).is_some() {
                    let at = *signed_at.get_or_insert_with(Instant::now);
                    if at.elapsed() >= SETTLE {
                        let _ = cdp_close(url);
                        return Ok(header);
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
}

/// The browser's flags (Meiro `chromiumWindowArgs`). A browser with a debugging port reports
/// `navigator.webdriver`, which Google answers with "This browser or app may not be secure";
/// a person signs in here, and the port only reads the cookies, so the page is told what a normal
/// browser would say. The window is not incognito: a private window keeps its cookies where the
/// debugging port does not read them, and this profile is deleted just the same.
fn window_command(exe: &Path, profile: &Path) -> Command {
    let mut command = Command::new(exe);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .args([
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-sync",
            "--disable-blink-features=AutomationControlled",
        ]);
    if cfg!(windows) {
        // Edge otherwise relaunches and exits the process watched here, ending the sign-in.
        command.arg("--edge-skip-compat-layer-relaunch");
    } else {
        command.arg("--password-store=basic");
    }
    command
        .arg(SIGN_IN_URL)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Asks the browser to quit; it may drop the connection before it answers.
fn cdp_close(url: &str) -> Result<()> {
    let mut ws = cdp_socket(url)?;
    ws.send(Message::Text(r#"{"id":2,"method":"Browser.close"}"#.into()))?;
    Ok(())
}

// ---- Cookie text and signing -----------------------------------------------------------------

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
        source: Source::default(),
        account: Account::default(),
        channels: vec![],
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
    })
}
fn format_digest(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---- Browser plumbing ---------------------------------------------------------------------------

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
                    let expires = c["expires"].as_f64().unwrap_or(-1.0);
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0.0, |d| d.as_secs_f64());
                    if expires > 0.0 && expires < now {
                        continue;
                    }
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
    fn switcher_email_is_the_selected_row_of_that_account() {
        let row = |index, selected, email: &str| Channel {
            account_index: index,
            selected,
            email: email.into(),
            name: "x".into(),
            ..Default::default()
        };
        let rows = [row(0, false, "a@x.com"), row(1, true, "b@x.com")];
        assert_eq!(switcher_email(&rows, 1).as_deref(), Some("b@x.com"));
        // The switcher answering as another account must not vouch for this one.
        assert_eq!(switcher_email(&rows, 0), None);
        assert_eq!(switcher_email(&[row(0, true, "")], 0), None);
    }

    #[test]
    fn zen_is_read_as_firefox_by_profile_path_only() {
        let zen = BROWSERS.into_iter().find(|b| b.label == "Zen").unwrap();
        let root = std::env::temp_dir().join(format!("ytdugem-zen-{}", std::process::id()));
        let profile = root.join("abc.Default (alpha)");
        fs::create_dir_all(&profile).unwrap();
        fs::create_dir_all(root.join("no-cookies")).unwrap();
        fs::write(profile.join("cookies.sqlite"), b"").unwrap();
        let selectors = zen.selectors_in(Some(root.clone()));
        let _ = fs::remove_dir_all(&root);
        // Never the bare "firefox", which would read Firefox's own profile instead.
        assert_eq!(selectors, [format!("firefox:{}", profile.display())]);
    }

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
    fn sealed_session_round_trips_and_rejects_another_key() {
        let key = [7u8; 32];
        let sealed = secretfile::seal(&key, b"{\"cookie\":\"SAPISID=x\"}").unwrap();
        assert_ne!(&sealed[12..], b"{\"cookie\":\"SAPISID=x\"}".as_slice());
        assert_eq!(
            secretfile::open_with(&key, &sealed).unwrap(),
            b"{\"cookie\":\"SAPISID=x\"}"
        );
        assert!(secretfile::open_with(&[8u8; 32], &sealed).is_err());
        assert!(secretfile::open_with(&key, &sealed[..8]).is_err());
        // A fresh nonce each time: the same session never seals to the same bytes.
        assert_ne!(
            sealed,
            secretfile::seal(&key, b"{\"cookie\":\"SAPISID=x\"}").unwrap()
        );
    }
    const LEGACY_SEALED: &str = "a0a1a2a3a4a5a6a7a8a9aaab9f6c185822ae6f9f1100f4a06e15aefe04c92a64b2c1270fe86154b2fcb126951110ade05fb35172a35636";
    #[test]
    fn session_sealed_by_the_aes_gcm_0_10_build_still_opens() {
        // Produced with aes-gcm 0.10.3: key 00..1f, nonce a0..ab, then nonce || ciphertext || tag.
        let key: [u8; 32] = core::array::from_fn(|i| i as u8);
        let blob: Vec<u8> = (0..LEGACY_SEALED.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&LEGACY_SEALED[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(blob.len(), 12 + 27 + 16);
        assert_eq!(
            secretfile::open_with(&key, &blob).unwrap(),
            b"ytdugem session test vector"
        );
        let mut tampered = blob.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(secretfile::open_with(&key, &tampered).is_err());
        let mut tampered = blob;
        tampered[20] ^= 1;
        assert!(secretfile::open_with(&key, &tampered).is_err());
    }
    #[test]
    fn older_saved_accounts_migrate_to_the_active_one() {
        let container = json!({"version":1,"active_id":"b","accounts":[
            {"id":"a","cookie":"SAPISID=a","auth_user":"0","account":{"name":"A"}},
            {"id":"b","cookie":"SAPISID=b","auth_user":"2","page_id":"UCbrand","account":{"name":"B","email":"b@example.com","channel_id":"@b","thumbnail":"","scope_id":"x"}}
        ]});
        let saved = secretfile::migrate(&container).unwrap();
        assert_eq!(saved.cookie, "SAPISID=b");
        assert_eq!(saved.source.account_index, 2);
        assert_eq!(saved.source.channel, "UCbrand");
        assert_eq!(saved.account.email, "b@example.com");
        let single = json!({"cookie":"SAPISID=s","auth_user":"1","account":{"name":"S"}});
        assert_eq!(
            secretfile::migrate(&single).unwrap().source.account_index,
            1
        );
        assert!(secretfile::migrate(&json!({"version":1,"accounts":[]})).is_none());
    }
    #[test]
    fn switcher_rows_carry_their_account_index_page_and_email() {
        let signin = |user: &str| {
            json!({"selectActiveIdentityEndpoint":{"supportedTokens":[
            {"accountSigninToken":{"signinUrl":format!("/signin?action=x&authuser={user}")}}]}})
        };
        let response = json!({"data":{"actions":[{"getMultiPageMenuAction":{"menu":{"multiPageMenuRenderer":{"sections":[
            {"accountSectionListRenderer":{
                "header":{"googleAccountHeaderRenderer":{"email":{"simpleText":"ada@example.com"}}},
                "contents":[{"accountItemSectionRenderer":{"contents":[
                    {"accountItem":{"accountName":{"simpleText":"Ada"},"channelHandle":{"simpleText":"@ada"},"isSelected":true,
                        "accountPhoto":{"thumbnails":[{"url":"https://img/a"}]},"serviceEndpoint":signin("0")}},
                    {"accountItem":{"accountName":{"simpleText":"Ada Music"},"serviceEndpoint":{"selectActiveIdentityEndpoint":{"supportedTokens":[
                        {"accountSigninToken":{"signinUrl":"/signin?authuser=0"}},{"pageIdToken":{"pageId":"111"}}]}}}},
                    {"accountItem":{"accountName":{"simpleText":"Broken"},"serviceEndpoint":signin("x")}}
                ]}}]}},
            {"accountSectionListRenderer":{
                "header":{"googleAccountHeaderRenderer":{"email":{"simpleText":"bob@example.com"}}},
                "contents":[{"accountItemSectionRenderer":{"contents":[
                    {"accountItem":{"accountName":{"simpleText":"Bob"},"serviceEndpoint":signin("1")}},
                    {"accountItem":{"accountName":{"simpleText":"Bob"},"serviceEndpoint":signin("1")}}
                ]}}]}}
        ]}}}}]}});
        let rows = extract_channels(&response, 0);
        let summary: Vec<_> = rows
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.account_index,
                    c.channel_id.as_str(),
                    c.email.as_str(),
                    c.disabled,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("Ada", 0, "", "ada@example.com", false),
                ("Ada Music", 0, "111", "ada@example.com", false),
                ("Broken", 0, "", "ada@example.com", true),
                ("Bob", 1, "", "bob@example.com", false),
            ]
        );
        assert!(
            rows[0].selected && rows[0].handle == "@ada" && rows[0].thumbnail == "https://img/a"
        );
        assert_eq!(rows[0].detail(), "ada@example.com");
    }
    #[test]
    fn pasted_cookie_accepts_header_name_and_curl() {
        for input in [
            "SAPISID=dummy; SID=x",
            "Cookie: SAPISID=dummy; SID=x",
            "curl 'https://music.youtube.com/' -H 'accept: */*' -H 'cookie: SAPISID=dummy; SID=x' --compressed",
        ] {
            let cookie = pasted_cookie(input).unwrap();
            assert_eq!(signing_secret(&cookie), Some("dummy"), "{input}");
            assert!(cookie.contains("SID=x"));
        }
        assert!(pasted_cookie("SID=x")
            .unwrap_err()
            .to_string()
            .contains("no SAPISID"));
    }
    #[test]
    fn imported_cookies_are_youtube_ones_that_have_not_expired() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let text = "# Netscape HTTP Cookie File\n\
                    .youtube.com\tTRUE\t/\tTRUE\t2000\tSAPISID\tlive\n\
                    #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tsession\n\
                    .youtube.com\tTRUE\t/\tTRUE\t500\tOLD\tgone\n\
                    .google.com\tTRUE\t/\tTRUE\t2000\tNID\tother\n\
                    music.youtube.com\tFALSE\t/\tTRUE\t2000\tSAPISID\tnewer\n";
        assert_eq!(from_netscape(text, now), "SAPISID=newer; SID=session");
    }
    #[test]
    fn sign_in_window_is_a_private_profile_that_looks_like_a_normal_browser() {
        let command = window_command(Path::new("msedge.exe"), Path::new("profile dir"));
        let args: Vec<_> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for expected in [
            "--user-data-dir=profile dir",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "--disable-blink-features=AutomationControlled",
            SIGN_IN_URL,
        ] {
            assert!(args.iter().any(|a| a == expected), "missing {expected}");
        }
        assert!(!args
            .iter()
            .any(|a| a.contains("incognito") || a.contains("headless")));
    }
    #[test]
    #[ignore = "opens a browser window on the desktop; no account is signed in"]
    fn sign_in_window_opens_reads_and_cleans_up() {
        let data = std::env::temp_dir().join(format!("ytdugem-signin-{}", std::process::id()));
        fs::create_dir_all(&data).unwrap();
        std::env::set_var("BITCHORD_DATA_DIR", &data);
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let shot = std::env::var("YTDUGEM_SHOT").ok();
        let watcher = thread::spawn(move || {
            thread::sleep(Duration::from_secs(8));
            if let Some(path) = shot {
                let script = format!("Add-Type -AssemblyName System.Windows.Forms,System.Drawing; $b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds; $i=New-Object System.Drawing.Bitmap $b.Width,$b.Height; [System.Drawing.Graphics]::FromImage($i).CopyFromScreen($b.Location,[System.Drawing.Point]::Empty,$b.Size); $i.Save('{path}')");
                let _ = Command::new("powershell")
                    .args(["-NoProfile", "-Command", &script])
                    .status();
            }
            let profiles: Vec<_> = fs::read_dir(auth_dir().unwrap())
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            println!("while open: {profiles:?}");
            thread::sleep(Duration::from_secs(4));
            stop.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let result = window_sign_in(&cancel);
        watcher.join().unwrap();
        println!(
            "ended after {:.1}s: {:?}",
            started.elapsed().as_secs_f32(),
            result.as_ref().err()
        );
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        let left: Vec<_> = fs::read_dir(auth_dir().unwrap())
            .unwrap()
            .flatten()
            .collect();
        assert!(
            left.is_empty(),
            "the throwaway profile must be deleted: {left:?}"
        );
        let _ = fs::remove_dir_all(data);
    }
    #[test]
    fn requests_act_as_the_chosen_account_and_channel() {
        let mut session = parse_cookie_input("SAPISID=dummy").unwrap();
        session.source = Source {
            account_index: 2,
            channel: "UCbrand".into(),
            ..Default::default()
        };
        let request = session
            .apply(reqwest::blocking::Client::new().post(MUSIC))
            .build()
            .unwrap();
        assert_eq!(request.headers()["X-Goog-AuthUser"], "2");
        assert_eq!(request.headers()["X-Goog-PageId"], "UCbrand");
        assert_eq!(session.context()["user"]["onBehalfOfUser"], "UCbrand");
        session.source.channel.clear();
        let request = session
            .apply(reqwest::blocking::Client::new().post(MUSIC))
            .build()
            .unwrap();
        assert!(!request.headers().contains_key("X-Goog-PageId"));
        assert!(session.context()["user"].get("onBehalfOfUser").is_none());
    }
}
