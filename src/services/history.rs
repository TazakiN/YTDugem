//! Registers plays in the signed-in account's YouTube Music history, as upstream's
//! `DesktopPlaybackTracker` does: a `player` request signed with the account returns the
//! play's stats URLs, then the `playback` ping creates the history entry, `atr` follows a few
//! seconds in, and `watchtime` reports how much was heard, every 30 seconds and once at the end.
//! YouTube only returns those URLs when the request quotes the current web player's
//! `signatureTimestamp`.

use super::{client, send_innertube, valid_video_id};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

/// Report watched time once this much new audio has gone by.
const REPORT_EVERY: u64 = 30;
/// Attempts at opening a play before it is written off.
const OPEN_ATTEMPTS: usize = 3;
const OPEN_RETRY_DELAY: Duration = Duration::from_secs(2);
/// What YouTube Music itself schedules `atr` for, when the response does not say.
const DEFAULT_ATR_SECONDS: u64 = 5;
/// How long closing the app waits for the last play's final ping.
const EXIT_WAIT: Duration = Duration::from_secs(3);

/// The stats URLs a player response nominates for one play.
#[derive(Clone, Debug, PartialEq)]
struct Tracking {
    playback: String,
    watchtime: Option<String>,
    atr: Option<String>,
    atr_after: u64,
}

/// One stats ping a play owes YouTube.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ping {
    Atr,
    Watchtime { seconds: u64, last: bool },
}

/// What a play has reported so far; turns playhead positions into the pings now due.
#[derive(Debug, Default)]
struct Reports {
    reported: u64,
    atr_sent: bool,
}

impl Reports {
    fn advance(&mut self, tracking: &Tracking, position: u64) -> Vec<Ping> {
        let mut due = vec![];
        if !self.atr_sent && tracking.atr.is_some() && position >= tracking.atr_after {
            self.atr_sent = true;
            due.push(Ping::Atr);
        }
        if tracking.watchtime.is_some() && position >= self.reported + REPORT_EVERY {
            self.reported = position;
            due.push(Ping::Watchtime {
                seconds: position,
                last: false,
            });
        }
        due
    }
}

/// The play being reported: its `play_request` and video id, and the line to its worker.
struct Play {
    key: (u64, String),
    positions: Sender<u64>,
    done: Receiver<()>,
}

/// Follows playback from the GUI thread; each play's network work runs on its own thread.
#[derive(Default)]
pub struct Reporter {
    play: Option<Play>,
}

impl Reporter {
    /// Each poll: `now` is the play in progress (its `play_request` and video id), `audible`
    /// whether it is playing, and `position` its playhead in seconds. A play is registered once
    /// it is first audible; a different play, or none, closes the previous one.
    pub fn step(&mut self, now: Option<(u64, &str)>, audible: bool, position: f64) {
        let same =
            matches!((&self.play, now), (Some(p), Some((r, id))) if p.key.0 == r && p.key.1 == id);
        if !same {
            self.play = None;
        }
        let Some((request, video_id)) = now else {
            return;
        };
        let position = position.max(0.0) as u64;
        if let Some(play) = &self.play {
            let _ = play.positions.send(position);
            return;
        }
        if !audible || !valid_video_id(video_id) {
            return;
        }
        let Some(session) = crate::auth::snapshot() else {
            return;
        };
        let (positions, rx) = mpsc::channel();
        let (finished, done) = mpsc::channel();
        let id = video_id.to_owned();
        std::thread::spawn(move || {
            report(&id, &session, rx);
            let _ = finished.send(());
        });
        let _ = positions.send(position);
        self.play = Some(Play {
            key: (request, video_id.to_owned()),
            positions,
            done,
        });
    }

    /// On exit: close the play in progress and give its final ping a moment to go out.
    pub fn finish(&mut self) {
        if let Some(play) = self.play.take() {
            drop(play.positions);
            let _ = play.done.recv_timeout(EXIT_WAIT);
        }
    }
}

/// One play from start to end: open it, then report the positions sent until the GUI hangs up.
fn report(video_id: &str, session: &crate::auth::Session, positions: Receiver<u64>) {
    let mut tracking = None;
    for attempt in 1..=OPEN_ATTEMPTS {
        match open(video_id, session) {
            Ok(t) => {
                tracking = t;
                break;
            }
            Err(e) => {
                eprintln!("YTDugem: history: registering {video_id} failed (attempt {attempt}/{OPEN_ATTEMPTS}): {e:#}");
                if attempt < OPEN_ATTEMPTS {
                    std::thread::sleep(OPEN_RETRY_DELAY);
                }
            }
        }
    }
    let Some(tracking) = tracking else {
        return;
    };
    let cpn = new_cpn();
    if let Err(e) = ping(session, &tracking.playback, &cpn, None) {
        eprintln!("YTDugem: history: playback ping for {video_id} failed: {e:#}");
        return;
    }
    let mut reports = Reports::default();
    let mut last = 0;
    for position in positions {
        last = position;
        for due in reports.advance(&tracking, position) {
            send(session, &tracking, &cpn, due, video_id);
        }
    }
    let last = Ping::Watchtime {
        seconds: last,
        last: true,
    };
    send(session, &tracking, &cpn, last, video_id);
}

fn send(session: &crate::auth::Session, tracking: &Tracking, cpn: &str, due: Ping, video_id: &str) {
    let result = match due {
        Ping::Atr => tracking
            .atr
            .as_deref()
            .map(|url| atr(session, url, cpn))
            .unwrap_or(Ok(())),
        Ping::Watchtime { seconds, last } => tracking
            .watchtime
            .as_deref()
            .map(|url| ping(session, url, cpn, Some((seconds, last))))
            .unwrap_or(Ok(())),
    };
    if let Err(e) = result {
        eprintln!("YTDugem: history: {due:?} ping for {video_id} failed: {e:#}");
    }
}

/// The play's stats URLs, or `None` when YouTube offers none for it (a verdict, not a failure).
fn open(video_id: &str, session: &crate::auth::Session) -> Result<Option<Tracking>> {
    let sts = signature_timestamp()?;
    let body = json!({
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
        "playbackContext": {"contentPlaybackContext": {
            "html5Preference": "HTML5_PREF_WANTS",
            "referer": format!("https://music.youtube.com/watch?v={video_id}"),
            "signatureTimestamp": sts,
        }},
    });
    let (status, response) = send_innertube("player", &body, Some(session))?;
    if !status.is_success() {
        bail!("player request failed ({status})");
    }
    let tracking = parse_tracking(&response);
    if tracking.is_none() {
        // A stale timestamp gets no tracking block; the next play fetches a fresh one.
        forget_signature_timestamp();
        eprintln!(
            "YTDugem: history: YouTube offered no tracking for {video_id} ({})",
            response
                .pointer("/playabilityStatus/status")
                .and_then(Value::as_str)
                .unwrap_or("no status")
        );
    }
    Ok(tracking)
}

fn parse_tracking(response: &Value) -> Option<Tracking> {
    let block = response.get("playbackTracking")?;
    let url = |key: &str| {
        block
            .pointer(&format!("/{key}/baseUrl"))
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("https://"))
            .map(str::to_owned)
    };
    let atr_after = block
        .pointer("/atrUrl/elapsedMediaTimeSeconds")
        .and_then(|v| v.as_u64().or_else(|| v.as_str()?.parse().ok()))
        .unwrap_or(DEFAULT_ATR_SECONDS);
    Some(Tracking {
        playback: url("videostatsPlaybackUrl")?,
        watchtime: url("videostatsWatchtimeUrl"),
        atr: url("atrUrl"),
        atr_after,
    })
}

/// A `playback` ping, or with `watched` (seconds heard, whether it is the play's last report)
/// a `watchtime` one, sent as the web client sends them.
fn ping(
    session: &crate::auth::Session,
    url: &str,
    cpn: &str,
    watched: Option<(u64, bool)>,
) -> Result<()> {
    let mut query: Vec<(&str, String)> = vec![
        ("ver", "2".into()),
        ("c", "WEB_REMIX".into()),
        ("cver", crate::auth::WEB_VERSION.into()),
        ("cpn", cpn.into()),
        ("cplayer", "UNIPLAYER".into()),
        ("cbr", "Chrome".into()),
        ("cbrver", "141.0.0.0".into()),
        ("cos", "Windows".into()),
        ("cosver", "10.0".into()),
        ("hl", "en_US".into()),
        ("cr", "US".into()),
    ];
    if let Some((seconds, last)) = watched {
        query.push(("st", "0".into()));
        query.push(("et", seconds.to_string()));
        query.push(("cmt", seconds.to_string()));
        query.push(("state", if last { "paused" } else { "playing" }.into()));
        if last {
            query.push(("final", "1".into()));
        }
    }
    get(session, url, &query)
}

/// The `atr` ping: its URL already carries the client fields.
fn atr(session: &crate::auth::Session, url: &str, cpn: &str) -> Result<()> {
    get(session, url, &[("cpn", cpn.into())])
}

fn get(session: &crate::auth::Session, url: &str, query: &[(&str, String)]) -> Result<()> {
    let response = session.apply(client()?.get(url).query(query)).send()?;
    if !response.status().is_success() {
        bail!("HTTP {}", response.status());
    }
    Ok(())
}

/// A client playback nonce: 16 characters from YouTube's URL-safe alphabet, one per play.
fn new_cpn() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_";
    (0..16)
        .map(|_| ALPHABET[rand::random_range(0..ALPHABET.len())] as char)
        .collect()
}

static SIGNATURE_TIMESTAMP: std::sync::Mutex<Option<u64>> = std::sync::Mutex::new(None);

/// The current web player's `signatureTimestamp`, as NewPipe (upstream's resolver) finds it:
/// the player id from the iframe API script, then the number in that player's `base.js`.
fn signature_timestamp() -> Result<u64> {
    if let Some(sts) = SIGNATURE_TIMESTAMP.lock().ok().and_then(|s| *s) {
        return Ok(sts);
    }
    let client = client()?;
    let api = client
        .get("https://www.youtube.com/iframe_api")
        .send()?
        .error_for_status()?
        .text()?;
    let player = player_id(&api).ok_or_else(|| anyhow!("no player id in the iframe API"))?;
    let js = client
        .get(format!(
            "https://www.youtube.com/s/player/{player}/player_ias.vflset/en_US/base.js"
        ))
        .send()?
        .error_for_status()?
        .text()?;
    let sts = parse_signature_timestamp(&js)
        .ok_or_else(|| anyhow!("no signatureTimestamp in player {player}"))?;
    if let Ok(mut slot) = SIGNATURE_TIMESTAMP.lock() {
        *slot = Some(sts);
    }
    Ok(sts)
}

fn forget_signature_timestamp() {
    if let Ok(mut slot) = SIGNATURE_TIMESTAMP.lock() {
        *slot = None;
    }
}

/// The 8-character player id in `player\/<id>\/` (or unescaped) of the iframe API script.
fn player_id(api: &str) -> Option<&str> {
    api.match_indices("player").find_map(|(i, m)| {
        let rest = api[i + m.len()..].trim_start_matches('\\');
        let id = rest.strip_prefix('/')?.get(..8)?;
        if !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return None;
        }
        rest[1 + id.len()..]
            .trim_start_matches('\\')
            .starts_with('/')
            .then_some(id)
    })
}

/// The number after `signatureTimestamp:` (or `=`) in a player's `base.js`.
fn parse_signature_timestamp(js: &str) -> Option<u64> {
    js.match_indices("signatureTimestamp").find_map(|(i, m)| {
        let rest = js[i + m.len()..].strip_prefix([':', '='])?;
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        rest[..digits].parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracking() -> Tracking {
        Tracking {
            playback: "https://s.youtube.com/api/stats/playback?docid=x".into(),
            watchtime: Some("https://s.youtube.com/api/stats/watchtime?docid=x".into()),
            atr: Some("https://s.youtube.com/api/stats/atr?docid=x".into()),
            atr_after: 5,
        }
    }

    #[test]
    fn reads_the_tracking_block_of_a_player_response() {
        let response = json!({"playbackTracking": {
            "videostatsPlaybackUrl": {"baseUrl": "https://s.youtube.com/api/stats/playback?docid=x"},
            "videostatsWatchtimeUrl": {"baseUrl": "https://s.youtube.com/api/stats/watchtime?docid=x"},
            "atrUrl": {"baseUrl": "https://s.youtube.com/api/stats/atr?docid=x", "elapsedMediaTimeSeconds": 5},
        }});
        assert_eq!(parse_tracking(&response), Some(tracking()));
        let unplayable = json!({"playabilityStatus": {"status": "UNPLAYABLE"}});
        assert_eq!(
            parse_tracking(&unplayable),
            None,
            "no block, no play to register"
        );
        let no_playback = json!({"playbackTracking": {"atrUrl": {"baseUrl": "https://x"}}});
        assert_eq!(parse_tracking(&no_playback), None);
    }

    #[test]
    fn reports_atr_once_then_watchtime_every_thirty_seconds() {
        let t = tracking();
        let mut r = Reports::default();
        let pings: Vec<(u64, Vec<Ping>)> = [1, 4, 5, 6, 29, 30, 45, 59, 61, 20]
            .into_iter()
            .map(|p| (p, r.advance(&t, p)))
            .filter(|(_, due)| !due.is_empty())
            .collect();
        let watch = |seconds| Ping::Watchtime {
            seconds,
            last: false,
        };
        assert_eq!(
            pings,
            [
                (5, vec![Ping::Atr]),
                (30, vec![watch(30)]),
                (61, vec![watch(61)])
            ],
            "a seek back reports nothing until it passes the last report again"
        );
    }

    #[test]
    fn a_play_without_watchtime_or_atr_urls_owes_only_the_playback_ping() {
        let t = Tracking {
            watchtime: None,
            atr: None,
            ..tracking()
        };
        let mut r = Reports::default();
        assert!((0..300).all(|p| r.advance(&t, p).is_empty()));
    }

    #[test]
    fn finds_the_player_id_and_its_signature_timestamp() {
        let api = r#"var scriptUrl = 'https:\/\/www.youtube.com\/s\/player\/b485e332\/www-widgetapi.vflset\/www-widgetapi.js';"#;
        assert_eq!(player_id(api), Some("b485e332"));
        assert_eq!(
            player_id("/s/player/b485e332/www-widgetapi.js"),
            Some("b485e332")
        );
        assert_eq!(player_id("player/ABC/"), None);
        let js = "a.sts=1;var x={signatureTimestamp:20779,foo:1}";
        assert_eq!(parse_signature_timestamp(js), Some(20779));
        assert_eq!(
            parse_signature_timestamp("signatureTimestamp=123;"),
            Some(123)
        );
        assert_eq!(parse_signature_timestamp("signatureTimestamp:x"), None);
    }

    #[test]
    fn a_nonce_is_sixteen_url_safe_characters() {
        let cpn = new_cpn();
        assert_eq!(cpn.len(), 16);
        assert!(cpn
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        assert_ne!(cpn, new_cpn());
    }

    #[test]
    #[ignore = "needs network access"]
    fn live_signature_timestamp() {
        let sts = signature_timestamp().unwrap();
        assert!(sts > 19_000, "{sts}");
    }
}
