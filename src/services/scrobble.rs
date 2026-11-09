//! Last.fm and ListenBrainz requests (upstream `DesktopScrobbling`, `LastFM`,
//! `ListenBrainzManager`): request building, the Last.fm `api_sig`, and response parsing.
//! Session keys and tokens are passed in by the caller and never logged.
use crate::scrobble::Entry;
use md5::{Digest, Md5};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

pub const LASTFM_API: &str = "https://ws.audioscrobbler.com/2.0/";
/// Last.fm's desktop web-auth page: the user approves YTDugem there, signed in to Last.fm.
pub const LASTFM_AUTH: &str = "https://www.last.fm/api/auth/";
const LISTENBRAINZ_SUBMIT: &str = "https://api.listenbrainz.org/1/submit-listens";
const LISTENBRAINZ_VALIDATE: &str = "https://api.listenbrainz.org/1/validate-token";
const USER_AGENT: &str = concat!(
    "YTDugem/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/TazakiN/YTDugem)"
);

/// Why a request failed, which decides what happens to the listen.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// Network trouble or a temporary server problem: keep the listen and retry later.
    Retry(String),
    /// The session, token or API key was refused: stop using the service until reconnected.
    Auth(String),
    /// The service refused this request itself: retrying would not help, so it is dropped.
    Rejected(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Retry(m) | Failure::Auth(m) | Failure::Rejected(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Failure {}

/// This build's Last.fm API account (never committed; see `docs/RELEASING.md`).
#[derive(Clone)]
pub struct LastFmKeys {
    pub api_key: String,
    pub secret: String,
}

fn nonblank(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// The API key and shared secret: baked in at build time, else read from the environment.
pub fn lastfm_keys() -> Option<LastFmKeys> {
    let pick = |built: Option<&str>, var: &str| {
        nonblank(built.map(str::to_owned)).or_else(|| nonblank(std::env::var(var).ok()))
    };
    Some(LastFmKeys {
        api_key: pick(
            option_env!("BITCHORD_LASTFM_API_KEY"),
            "BITCHORD_LASTFM_API_KEY",
        )?,
        secret: pick(
            option_env!("BITCHORD_LASTFM_API_SECRET"),
            "BITCHORD_LASTFM_API_SECRET",
        )?,
    })
}

/// A session key given in the environment (upstream `BITCHORD_LASTFM_SESSION_KEY`).
pub fn env_lastfm_session() -> Option<String> {
    nonblank(std::env::var("BITCHORD_LASTFM_SESSION_KEY").ok())
}

/// A ListenBrainz token given in the environment (upstream `BITCHORD_LISTENBRAINZ_TOKEN`).
pub fn env_listenbrainz_token() -> Option<String> {
    nonblank(std::env::var("BITCHORD_LISTENBRAINZ_TOKEN").ok())
}

/// Last.fm's `api_sig`: every parameter but `format` and `callback`, sorted by name and
/// concatenated as name+value, then the shared secret, MD5 in lowercase hex.
pub fn sign(params: &BTreeMap<String, String>, secret: &str) -> String {
    let mut hasher = Md5::new();
    for (key, value) in params {
        if key != "format" && key != "callback" {
            hasher.update(key.as_bytes());
            hasher.update(value.as_bytes());
        }
    }
    hasher.update(secret.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The signed form for one Last.fm call.
pub fn lastfm_form(
    method: &str,
    mut params: BTreeMap<String, String>,
    keys: &LastFmKeys,
    session: Option<&str>,
) -> Vec<(String, String)> {
    params.insert("method".into(), method.into());
    params.insert("api_key".into(), keys.api_key.clone());
    if let Some(sk) = session {
        params.insert("sk".into(), sk.into());
    }
    let signature = sign(&params, &keys.secret);
    let mut form: Vec<(String, String)> = params.into_iter().collect();
    form.push(("api_sig".into(), signature));
    form.push(("format".into(), "json".into()));
    form
}

/// `track.updateNowPlaying` parameters.
pub fn now_playing_params(entry: &Entry) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    params.insert("artist".into(), entry.artist.clone());
    params.insert("track".into(), entry.track.clone());
    if !entry.album.is_empty() {
        params.insert("album".into(), entry.album.clone());
    }
    if entry.duration > 0 {
        params.insert("duration".into(), entry.duration.to_string());
    }
    params
}

/// `track.scrobble` parameters for a batch (Last.fm takes up to 50), with array indices.
pub fn scrobble_params(entries: &[Entry]) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    for (i, entry) in entries.iter().enumerate() {
        params.insert(format!("artist[{i}]"), entry.artist.clone());
        params.insert(format!("track[{i}]"), entry.track.clone());
        params.insert(format!("timestamp[{i}]"), entry.timestamp.to_string());
        if !entry.album.is_empty() {
            params.insert(format!("album[{i}]"), entry.album.clone());
        }
        if entry.duration > 0 {
            params.insert(format!("duration[{i}]"), entry.duration.to_string());
        }
    }
    params
}

/// The error code and message in a Last.fm reply, JSON (`format=json`) or XML.
pub fn lastfm_error(body: &str) -> Option<(i64, String)> {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        let code = value["error"].as_i64()?;
        let message = value["message"]
            .as_str()
            .unwrap_or("Last.fm error")
            .to_owned();
        return Some((code, message));
    }
    if !body.contains("status=\"failed\"") {
        return None;
    }
    let start = body.find("<error")?;
    let tag = &body[start..];
    let code = tag
        .split_once("code=\"")?
        .1
        .split_once('"')?
        .0
        .parse()
        .ok()?;
    let message = tag
        .split_once('>')
        .and_then(|(_, rest)| rest.split_once("</error>"))
        .map(|(text, _)| text.trim().to_owned())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "Last.fm error".into());
    Some((code, message))
}

/// Sorts a Last.fm error code: refused credentials, temporary trouble, or a bad request.
fn lastfm_failure(code: i64, message: String) -> Failure {
    let text = format!("Last.fm: {message} (error {code})");
    match code {
        // Authentication failed, invalid session key, invalid API key, invalid signature,
        // unauthorized or expired token, suspended API key.
        4 | 9 | 10 | 13 | 14 | 15 | 26 => Failure::Auth(text),
        // Operation failed, service offline, temporarily unavailable, rate limit exceeded.
        8 | 11 | 16 | 29 => Failure::Retry(text),
        _ => Failure::Rejected(text),
    }
}

/// A Last.fm reply as JSON, or why it failed.
pub fn parse_lastfm(status: u16, body: &str) -> Result<Value, Failure> {
    if let Some((code, message)) = lastfm_error(body) {
        return Err(lastfm_failure(code, message));
    }
    match status {
        200..=299 => serde_json::from_str(body)
            .map_err(|_| Failure::Retry("Last.fm sent an unreadable reply".into())),
        429 | 500..=599 => Err(Failure::Retry(format!("Last.fm returned HTTP {status}"))),
        _ => Err(Failure::Rejected(format!("Last.fm returned HTTP {status}"))),
    }
}

fn client() -> Result<reqwest::blocking::Client, Failure> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| Failure::Retry(format!("Could not start a connection: {e}")))
}

/// A transport error, with any URL (and so any query) left out of the message.
fn network(e: reqwest::Error) -> Failure {
    Failure::Retry(format!("Network error: {}", e.without_url()))
}

fn lastfm_call(form: Vec<(String, String)>) -> Result<Value, Failure> {
    let response = client()?
        .post(LASTFM_API)
        .form(&form)
        .send()
        .map_err(network)?;
    let status = response.status().as_u16();
    let body = response.text().map_err(network)?;
    parse_lastfm(status, &body)
}

/// Step 1 of web auth: a request token for the user to approve.
pub fn lastfm_token(keys: &LastFmKeys) -> Result<String, Failure> {
    let reply = lastfm_call(lastfm_form("auth.getToken", BTreeMap::new(), keys, None))?;
    reply["token"]
        .as_str()
        .filter(|t| safe_token(t))
        .map(str::to_owned)
        .ok_or_else(|| Failure::Rejected("Last.fm did not return a token".into()))
}

/// Last.fm tokens and keys are short alphanumeric strings; anything else is not put in a URL.
fn safe_token(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Step 2: the page where the user approves the token.
pub fn lastfm_auth_url(keys: &LastFmKeys, token: &str) -> Option<String> {
    (safe_token(&keys.api_key) && safe_token(token))
        .then(|| format!("{LASTFM_AUTH}?api_key={}&token={token}", keys.api_key))
}

/// The user name and session key in an `auth.getSession` reply.
pub fn parse_session(reply: &Value) -> Option<(String, String)> {
    let session = &reply["session"];
    let key = session["key"].as_str().filter(|k| !k.trim().is_empty())?;
    let name = session["name"].as_str().unwrap_or_default();
    Some((name.to_owned(), key.to_owned()))
}

/// Step 3: the session, or `None` while the token is still waiting for approval (error 14).
pub fn lastfm_session(keys: &LastFmKeys, token: &str) -> Result<Option<(String, String)>, Failure> {
    let mut params = BTreeMap::new();
    params.insert("token".to_owned(), token.to_owned());
    let form = lastfm_form("auth.getSession", params, keys, None);
    let response = client()?
        .post(LASTFM_API)
        .form(&form)
        .send()
        .map_err(network)?;
    let status = response.status().as_u16();
    let body = response.text().map_err(network)?;
    if lastfm_error(&body).is_some_and(|(code, _)| code == 14) {
        return Ok(None);
    }
    let reply = parse_lastfm(status, &body)?;
    parse_session(&reply)
        .map(Some)
        .ok_or_else(|| Failure::Rejected("Last.fm did not return a session".into()))
}

pub fn lastfm_now_playing(keys: &LastFmKeys, session: &str, entry: &Entry) -> Result<(), Failure> {
    let form = lastfm_form(
        "track.updateNowPlaying",
        now_playing_params(entry),
        keys,
        Some(session),
    );
    lastfm_call(form).map(drop)
}

pub fn lastfm_scrobble(keys: &LastFmKeys, session: &str, entries: &[Entry]) -> Result<(), Failure> {
    let form = lastfm_form(
        "track.scrobble",
        scrobble_params(entries),
        keys,
        Some(session),
    );
    lastfm_call(form).map(drop)
}

/// What a ListenBrainz submission announces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListenKind {
    PlayingNow,
    /// Finished listens: `single` for one, `import` for a batch.
    Listens,
}

/// The `submit-listens` body for `entries`.
pub fn listenbrainz_body(kind: ListenKind, entries: &[Entry]) -> Value {
    let listen_type = match kind {
        ListenKind::PlayingNow => "playing_now",
        ListenKind::Listens if entries.len() == 1 => "single",
        ListenKind::Listens => "import",
    };
    let payload: Vec<Value> = entries
        .iter()
        .map(|entry| {
            let mut info = json!({
                "submission_client": "YTDugem",
                "submission_client_version": env!("CARGO_PKG_VERSION"),
            });
            if entry.duration > 0 {
                info["duration_ms"] = json!(u64::from(entry.duration) * 1000);
            }
            let mut metadata = json!({
                "artist_name": entry.artist,
                "track_name": entry.track,
                "additional_info": info,
            });
            if !entry.album.is_empty() {
                metadata["release_name"] = json!(entry.album);
            }
            let mut listen = json!({ "track_metadata": metadata });
            if kind == ListenKind::Listens {
                listen["listened_at"] = json!(entry.timestamp);
            }
            listen
        })
        .collect();
    json!({ "listen_type": listen_type, "payload": payload })
}

fn listenbrainz_message(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    value["error"]
        .as_str()
        .or_else(|| value["message"].as_str())
        .map(str::to_owned)
}

/// A ListenBrainz reply: success, or why it failed.
pub fn parse_listenbrainz(status: u16, body: &str) -> Result<(), Failure> {
    let message = || {
        let detail = listenbrainz_message(body).unwrap_or_else(|| format!("HTTP {status}"));
        format!("ListenBrainz: {detail}")
    };
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(Failure::Auth(message())),
        429 | 500..=599 => Err(Failure::Retry(message())),
        _ => Err(Failure::Rejected(message())),
    }
}

/// The user name a `validate-token` reply vouches for.
pub fn parse_validation(status: u16, body: &str) -> Result<String, Failure> {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if value["valid"].as_bool() == Some(true) {
        return Ok(value["user_name"].as_str().unwrap_or_default().to_owned());
    }
    match status {
        429 | 500..=599 => Err(Failure::Retry(format!(
            "ListenBrainz returned HTTP {status}"
        ))),
        _ => Err(Failure::Auth(
            "ListenBrainz did not accept this token. Copy it again from listenbrainz.org/settings."
                .into(),
        )),
    }
}

fn valid_header_token(token: &str) -> Result<(), Failure> {
    if token.is_empty() || token.len() > 256 || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Failure::Auth(
            "That does not look like a ListenBrainz token".into(),
        ));
    }
    Ok(())
}

/// Checks a user token with `/1/validate-token`; returns the account name.
pub fn listenbrainz_validate(token: &str) -> Result<String, Failure> {
    valid_header_token(token)?;
    let response = client()?
        .get(LISTENBRAINZ_VALIDATE)
        .header("Authorization", format!("Token {token}"))
        .send()
        .map_err(network)?;
    let status = response.status().as_u16();
    let body = response.text().map_err(network)?;
    parse_validation(status, &body)
}

pub fn listenbrainz_submit(
    token: &str,
    kind: ListenKind,
    entries: &[Entry],
) -> Result<(), Failure> {
    valid_header_token(token)?;
    let response = client()?
        .post(LISTENBRAINZ_SUBMIT)
        .header("Authorization", format!("Token {token}"))
        .json(&listenbrainz_body(kind, entries))
        .send()
        .map_err(network)?;
    let status = response.status().as_u16();
    let body = response.text().map_err(network)?;
    parse_listenbrainz(status, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scrobble::Service;

    fn keys() -> LastFmKeys {
        LastFmKeys {
            api_key: "K".into(),
            secret: "S3CR3T".into(),
        }
    }

    fn entry(artist: &str, track: &str, album: &str, duration: u32, at: i64) -> Entry {
        Entry {
            service: Service::LastFm,
            artist: artist.into(),
            track: track.into(),
            album: album.into(),
            duration,
            timestamp: at,
        }
    }

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/services/fixtures/scrobble")
            .join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn signature_matches_the_last_fm_documentation_example() {
        // https://www.last.fm/api/authspec: md5("api_keyxxxxxxxxmethodauth.getSessiontokenyyyyyyilovecher")
        let params: BTreeMap<String, String> = [
            ("method", "auth.getSession"),
            ("token", "yyyyyy"),
            ("api_key", "xxxxxxxx"),
            ("format", "json"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        assert_eq!(
            sign(&params, "ilovecher"),
            "1333ebf6f7dec747486b6ce965cca66b"
        );
    }

    #[test]
    fn scrobble_form_is_signed_over_sorted_utf8_parameters() {
        let form = lastfm_form(
            "track.scrobble",
            scrobble_params(&[entry(
                "Beyoncé ＆ Jay-Z",
                "Crazy in Love",
                "X",
                0,
                1_700_000_000,
            )]),
            &keys(),
            Some("S"),
        );
        let get = |k: &str| {
            form.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("artist[0]"), Some("Beyoncé ＆ Jay-Z"));
        assert_eq!(get("timestamp[0]"), Some("1700000000"));
        assert_eq!(get("album[0]"), Some("X"));
        assert_eq!(get("duration[0]"), None, "unknown duration is left out");
        assert_eq!(get("sk"), Some("S"));
        assert_eq!(get("format"), Some("json"));
        // Computed independently with Python's hashlib over the same concatenation.
        assert_eq!(get("api_sig"), Some("9e14e473eb0487a8700fdde51af79654"));
    }

    #[test]
    fn batches_are_indexed_and_now_playing_is_not() {
        let entries = [
            entry("A", "One", "", 200, 10),
            entry("B", "Two", "Album", 0, 20),
        ];
        let params = scrobble_params(&entries);
        assert_eq!(params["track[1]"], "Two");
        assert_eq!(params["timestamp[1]"], "20");
        assert_eq!(params["duration[0]"], "200");
        assert!(!params.contains_key("album[0]"));
        assert_eq!(params.len(), 8);
        let now = now_playing_params(&entries[1]);
        assert_eq!(now["album"], "Album");
        assert!(!now.contains_key("duration") && !now.contains_key("timestamp"));
    }

    #[test]
    fn last_fm_errors_are_read_from_json_and_xml() {
        let json = fixture("lastfm-invalid-session.json");
        assert_eq!(
            lastfm_error(&json),
            Some((9, "Invalid session key - Please re-authenticate".into()))
        );
        assert!(matches!(parse_lastfm(403, &json), Err(Failure::Auth(_))));
        let xml = fixture("lastfm-offline.xml");
        assert_eq!(
            lastfm_error(&xml),
            Some((
                11,
                "Service Offline - This service is temporarily offline. Try again later.".into()
            ))
        );
        assert!(matches!(parse_lastfm(503, &xml), Err(Failure::Retry(_))));
        assert!(matches!(
            parse_lastfm(200, r#"{"error":6,"message":"Invalid parameters"}"#),
            Err(Failure::Rejected(_))
        ));
        assert!(matches!(
            parse_lastfm(502, "<html>Bad gateway</html>"),
            Err(Failure::Retry(_))
        ));
        assert!(matches!(parse_lastfm(404, ""), Err(Failure::Rejected(_))));
        let accepted = fixture("lastfm-scrobble-accepted.json");
        assert!(parse_lastfm(200, &accepted).is_ok());
        // A pending token is error 14, which the session poll treats as "not yet".
        assert_eq!(
            lastfm_error(r#"{"error":14,"message":"Unauthorized Token"}"#)
                .unwrap()
                .0,
            14
        );
    }

    #[test]
    fn session_and_auth_url_are_read_safely() {
        let reply: Value = serde_json::from_str(&fixture("lastfm-session.json")).unwrap();
        assert_eq!(
            parse_session(&reply),
            Some(("listener".into(), "d580d57f32848f5dcf574d1ce18d78b2".into()))
        );
        assert_eq!(parse_session(&json!({"session": {"name": "x"}})), None);
        assert_eq!(
            lastfm_auth_url(&keys(), "abc123").as_deref(),
            Some("https://www.last.fm/api/auth/?api_key=K&token=abc123")
        );
        assert_eq!(lastfm_auth_url(&keys(), "abc&evil=1"), None);
    }

    #[test]
    fn listenbrainz_payloads_match_the_api() {
        let one = [entry("Artist", "Song", "Album", 215, 1_700_000_000)];
        assert_eq!(
            listenbrainz_body(ListenKind::Listens, &one),
            json!({
                "listen_type": "single",
                "payload": [{
                    "listened_at": 1_700_000_000,
                    "track_metadata": {
                        "artist_name": "Artist",
                        "track_name": "Song",
                        "release_name": "Album",
                        "additional_info": {
                            "submission_client": "YTDugem",
                            "submission_client_version": env!("CARGO_PKG_VERSION"),
                            "duration_ms": 215_000,
                        },
                    },
                }],
            })
        );
        let now = listenbrainz_body(
            ListenKind::PlayingNow,
            &[entry("A", "\"Quoted\"\n", "", 0, 5)],
        );
        assert_eq!(now["listen_type"], "playing_now");
        assert!(now["payload"][0].get("listened_at").is_none());
        assert!(now["payload"][0]["track_metadata"]
            .get("release_name")
            .is_none());
        assert!(now["payload"][0]["track_metadata"]["additional_info"]
            .get("duration_ms")
            .is_none());
        assert_eq!(
            now["payload"][0]["track_metadata"]["track_name"],
            "\"Quoted\"\n"
        );
        let two = [entry("A", "1", "", 0, 1), entry("B", "2", "", 0, 2)];
        let batch = listenbrainz_body(ListenKind::Listens, &two);
        assert_eq!(batch["listen_type"], "import");
        assert_eq!(batch["payload"][1]["listened_at"], 2);
    }

    #[test]
    fn listenbrainz_replies_are_classified() {
        assert_eq!(
            parse_listenbrainz(200, &fixture("listenbrainz-ok.json")),
            Ok(())
        );
        assert_eq!(
            parse_listenbrainz(401, &fixture("listenbrainz-unauthorized.json")),
            Err(Failure::Auth(
                "ListenBrainz: Invalid authorization token.".into()
            ))
        );
        assert!(matches!(
            parse_listenbrainz(400, r#"{"code":400,"error":"bad"}"#),
            Err(Failure::Rejected(_))
        ));
        assert!(matches!(
            parse_listenbrainz(503, ""),
            Err(Failure::Retry(_))
        ));
        assert_eq!(
            parse_validation(200, &fixture("listenbrainz-valid.json")),
            Ok("listener".into())
        );
        assert!(matches!(
            parse_validation(200, &fixture("listenbrainz-invalid.json")),
            Err(Failure::Auth(_))
        ));
        assert!(matches!(parse_validation(502, ""), Err(Failure::Retry(_))));
        assert!(valid_header_token("a b").is_err());
        assert!(valid_header_token("0b9a-4e1c").is_ok());
    }

    /// Run with `cargo test listenbrainz_rejects_an_invalid_token -- --ignored`: a made-up
    /// token against the real API. No account is used.
    #[test]
    #[ignore = "network"]
    fn listenbrainz_rejects_an_invalid_token() {
        let result = listenbrainz_validate("00000000-0000-0000-0000-000000000000");
        assert!(matches!(result, Err(Failure::Auth(_))), "{result:?}");
    }
}
