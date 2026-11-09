//! Scrobbling to Last.fm and ListenBrainz (upstream `DesktopScrobbling`,
//! `DesktopScrobbleSettings`, `ScrobbleManager`, `PrimaryArtist`).
//!
//! The GUI feeds playback progress to a [`Tracker`] each poll; it decides when a play has been
//! heard long enough, measuring listening time with the same [`stats::Listen`] the Stats page
//! uses, so pauses and seeks never count. Listens go to a background [`Scrobbler`] thread that
//! keeps them in a small persisted queue until each service has accepted them.
use crate::model::Track;
use crate::services::scrobble::{self as api, Failure, ListenKind};
use crate::stats::{self, Listen};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

/// Upstream defaults (`DesktopScrobbleSettings`, Android `AppSettings`).
pub const DEFAULT_MIN_DURATION: u32 = 30;
pub const DEFAULT_DELAY_PERCENT: f32 = 0.5;
pub const DEFAULT_DELAY_SECONDS: u32 = 180;
/// Listens waiting to be sent; the oldest are dropped past this.
pub const MAX_QUEUE: usize = 500;
/// Last.fm accepts at most 50 scrobbles per `track.scrobble` call.
pub const BATCH: usize = 50;
/// How often a non-empty queue is retried.
const RETRY_EVERY: Duration = Duration::from_secs(120);
/// `scrobble-queue.json` refuses to load past this size.
const MAX_QUEUE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    LastFm,
    ListenBrainz,
}

impl Service {
    pub const ALL: [Service; 2] = [Service::LastFm, Service::ListenBrainz];
    pub fn label(self) -> &'static str {
        match self {
            Service::LastFm => "Last.fm",
            Service::ListenBrainz => "ListenBrainz",
        }
    }
    /// The id of its secret in `store::scrobble_secret`.
    pub fn secret_id(self) -> &'static str {
        match self {
            Service::LastFm => "lastfm_session",
            Service::ListenBrainz => "listenbrainz",
        }
    }
}

/// Scrobbling options saved in `library.json` (upstream `DesktopScrobbleSettings` keys).
/// Session keys and tokens are never stored here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrobbleSettings {
    pub lastfm_enabled: bool,
    /// The signed-in Last.fm user, shown in Settings.
    pub lastfm_username: String,
    pub lastfm_scrobble: bool,
    pub lastfm_now_playing: bool,
    pub lastfm_primary_artist: bool,
    pub listenbrainz_enabled: bool,
    pub listenbrainz_username: String,
    /// Upstream always sends ListenBrainz `playing_now`; YTDugem lets it be turned off.
    pub listenbrainz_now_playing: bool,
    pub listenbrainz_primary_artist: bool,
    /// Tracks this long or shorter (seconds) are never scrobbled.
    pub min_duration: u32,
    /// The share of a track to hear before it is scrobbled.
    pub delay_percent: f32,
    /// ... or this many seconds, whichever comes first.
    pub delay_seconds: u32,
}

impl Default for ScrobbleSettings {
    fn default() -> Self {
        Self {
            lastfm_enabled: false,
            lastfm_username: String::new(),
            lastfm_scrobble: true,
            lastfm_now_playing: true,
            lastfm_primary_artist: false,
            listenbrainz_enabled: false,
            listenbrainz_username: String::new(),
            listenbrainz_now_playing: true,
            listenbrainz_primary_artist: false,
            min_duration: DEFAULT_MIN_DURATION,
            delay_percent: DEFAULT_DELAY_PERCENT,
            delay_seconds: DEFAULT_DELAY_SECONDS,
        }
    }
}

impl ScrobbleSettings {
    /// Clamps hand-edited values to upstream's slider ranges.
    pub fn sanitize(&mut self) {
        self.min_duration = self.min_duration.clamp(15, 120);
        self.delay_percent = if self.delay_percent.is_finite() {
            self.delay_percent.clamp(0.1, 1.0)
        } else {
            DEFAULT_DELAY_PERCENT
        };
        self.delay_seconds = self.delay_seconds.clamp(30, 300);
        for name in [&mut self.lastfm_username, &mut self.listenbrainz_username] {
            if name.len() > 256 || name.contains(char::is_control) {
                name.clear();
            }
        }
    }

    /// Seconds of listening after which a track of `duration` seconds is scrobbled: the
    /// delay share of it or the delay cap, whichever is less. `None` for tracks of unknown
    /// length or no longer than the minimum, which are never scrobbled (upstream
    /// `durationMs > minDuration * 1000`).
    pub fn threshold(&self, duration: f64) -> Option<f64> {
        (duration.is_finite() && duration > f64::from(self.min_duration))
            .then(|| (duration * f64::from(self.delay_percent)).min(f64::from(self.delay_seconds)))
    }

    fn primary_only(&self, service: Service) -> bool {
        match service {
            Service::LastFm => self.lastfm_primary_artist,
            Service::ListenBrainz => self.listenbrainz_primary_artist,
        }
    }
}

/// The first credited artist, for services that handle joint credits badly (upstream
/// `String.primaryArtist()`): the text before the first `,`, ` & ` or ` ＆ `. A name with no
/// separator is kept whole ("AC/DC"), and so is one whose first credit would be blank.
pub fn primary_artist(artist: &str) -> String {
    let chars: Vec<(usize, char)> = artist.char_indices().collect();
    let cut = chars.iter().enumerate().find_map(|(i, &(at, c))| {
        let spaced = || {
            i > 0
                && chars[i - 1].1.is_whitespace()
                && chars.get(i + 1).is_some_and(|(_, n)| n.is_whitespace())
        };
        (c == ',' || ((c == '&' || c == '＆') && spaced())).then_some(at)
    });
    match cut.map(|at| artist[..at].trim()) {
        Some(first) if !first.is_empty() => first.to_owned(),
        _ => artist.to_owned(),
    }
}

/// Trimmed, with runs of whitespace (including newlines) folded to one space.
pub fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One listen as one service will receive it (artist already cut back when asked).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub service: Service,
    pub artist: String,
    pub track: String,
    #[serde(default)]
    pub album: String,
    /// Whole seconds; 0 when unknown.
    #[serde(default)]
    pub duration: u32,
    /// When the play started, UTC unix seconds.
    pub timestamp: i64,
}

impl Entry {
    /// `None` when the track has no artist or title, as local files without tags.
    pub fn new(
        service: Service,
        settings: &ScrobbleSettings,
        track: &Track,
        duration: f64,
        timestamp: i64,
    ) -> Option<Self> {
        let mut artist = clean(&track.artist);
        if settings.primary_only(service) {
            artist = primary_artist(&artist);
        }
        let title = clean(&track.title);
        if artist.is_empty() || title.is_empty() {
            return None;
        }
        Some(Self {
            service,
            artist,
            track: title,
            album: clean(&track.album),
            duration: if duration.is_finite() && duration > 0.0 {
                duration.round().min(f64::from(u32::MAX)) as u32
            } else {
                0
            },
            timestamp,
        })
    }
}

/// What the tracker asks to be sent.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// The track started playing.
    NowPlaying { track: Track, duration: f64 },
    /// The track has been heard long enough; `started` is when its play began.
    Scrobble {
        track: Track,
        duration: f64,
        started: i64,
    },
}

struct Play {
    request: u64,
    listen: Listen,
    announced: bool,
    scrobbled: bool,
}

/// Follows the play in progress and decides when to announce and scrobble it.
#[derive(Default)]
pub struct Tracker {
    play: Option<Play>,
}

impl Tracker {
    /// Each poll: `current` is the play request id, which changes for every new play (also
    /// of the same track), and its track.
    /// `duration` is the player's (0 while unknown). Returns what to send now.
    pub fn tick(
        &mut self,
        current: Option<(u64, &Track)>,
        position: f64,
        duration: f64,
        playing: bool,
        settings: &ScrobbleSettings,
        now: i64,
    ) -> Vec<Action> {
        let Some((request, track)) = current else {
            self.play = None;
            return vec![];
        };
        let same = self
            .play
            .as_ref()
            .is_some_and(|p| p.request == request && p.listen.key() == track.key());
        if !same {
            self.play = Some(Play {
                request,
                listen: Listen::new(track.clone(), now),
                announced: false,
                scrobbled: false,
            });
        }
        let Some(play) = self.play.as_mut() else {
            return vec![];
        };
        if duration.is_finite() && duration > 0.0 {
            play.listen.track.duration = duration;
        }
        play.listen.advance(position, playing);
        let mut actions = vec![];
        let length = play.listen.track.duration;
        if playing && !play.announced {
            play.announced = true;
            actions.push(Action::NowPlaying {
                track: play.listen.track.clone(),
                duration: length,
            });
        }
        if !play.scrobbled
            && settings
                .threshold(length)
                .is_some_and(|needed| play.listen.listened >= needed)
        {
            play.scrobbled = true;
            actions.push(Action::Scrobble {
                track: play.listen.track.clone(),
                duration: length,
                started: play.listen.started,
            });
        }
        actions
    }
}

/// Listens not yet accepted, oldest first, persisted in `scrobble-queue.json`.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queue {
    pub entries: Vec<Entry>,
}

pub fn queue_path(library: &Path) -> PathBuf {
    library.with_file_name("scrobble-queue.json")
}

impl Queue {
    /// A missing file is an empty queue; an unreadable one is an error and is left as is.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        if std::fs::metadata(path)?.len() > MAX_QUEUE_BYTES {
            anyhow::bail!("Scrobble queue file is too large");
        }
        let bytes = std::fs::read(path).context("Cannot read the scrobble queue")?;
        let mut queue: Self = serde_json::from_slice(&bytes).context("Invalid scrobble queue")?;
        queue.bound();
        Ok(queue)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temp)?;
            file.write_all(&serde_json::to_vec(self)?)?;
            file.sync_all()?;
        }
        std::fs::rename(temp, path).context("Cannot save the scrobble queue")
    }

    pub fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        self.bound();
    }

    fn bound(&mut self) {
        if self.entries.len() > MAX_QUEUE {
            let extra = self.entries.len() - MAX_QUEUE;
            self.entries.drain(..extra);
        }
    }

    /// The oldest `BATCH` listens for `service`.
    pub fn batch(&self, service: Service) -> Vec<Entry> {
        self.entries
            .iter()
            .filter(|e| e.service == service)
            .take(BATCH)
            .cloned()
            .collect()
    }

    /// Removes the first `n` listens of `service` (a batch just sent or refused).
    pub fn remove(&mut self, service: Service, n: usize) {
        let mut left = n;
        self.entries.retain(|e| {
            if left > 0 && e.service == service {
                left -= 1;
                false
            } else {
                true
            }
        });
    }

    pub fn forget(&mut self, service: Service) {
        self.entries.retain(|e| e.service != service);
    }
}

/// What the scrobbler tells the GUI.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// The service refused its credentials: it is switched off until reconnected.
    AuthFailed(Service, String),
    /// The queue could not be read or written.
    QueueError(String),
}

/// Sends batches from the queue for every service in `active` until one has nothing left or
/// fails. Accepted and refused (bad request) batches leave the queue; network trouble keeps
/// them for the next try; a refused credential drops the service from `active`.
pub fn flush(
    queue: &mut Queue,
    active: &mut HashSet<Service>,
    mut send: impl FnMut(Service, &[Entry]) -> Result<(), Failure>,
) -> Vec<Notice> {
    let mut notices = vec![];
    for service in Service::ALL {
        while active.contains(&service) {
            let batch = queue.batch(service);
            if batch.is_empty() {
                break;
            }
            match send(service, &batch) {
                Ok(()) => queue.remove(service, batch.len()),
                Err(Failure::Rejected(e)) => {
                    eprintln!(
                        "YTDugem: {} refused {} listens: {e}",
                        service.label(),
                        batch.len()
                    );
                    queue.remove(service, batch.len());
                }
                Err(Failure::Retry(_)) => break,
                Err(Failure::Auth(e)) => {
                    active.remove(&service);
                    notices.push(Notice::AuthFailed(service, e));
                }
            }
        }
    }
    notices
}

enum Job {
    NowPlaying(Entry),
    Submit(Vec<Entry>),
    Forget(Service),
    Active(HashSet<Service>),
}

/// The background sender. Dropping it ends the thread once its current request finishes.
pub struct Scrobbler {
    jobs: Sender<Job>,
    pending: Arc<AtomicUsize>,
    active: HashSet<Service>,
}

impl Scrobbler {
    pub fn start(queue_path: PathBuf, notify: impl Fn(Notice) + Send + 'static) -> Self {
        let (jobs, rx) = mpsc::channel();
        let pending = Arc::new(AtomicUsize::new(0));
        let count = pending.clone();
        std::thread::spawn(move || worker(queue_path, rx, count, notify));
        Self {
            jobs,
            pending,
            active: HashSet::new(),
        }
    }

    /// Listens waiting to be sent.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }

    /// Which services may be sent to; queued listens of others wait.
    pub fn set_active(&mut self, active: HashSet<Service>) {
        if active != self.active {
            self.active = active.clone();
            let _ = self.jobs.send(Job::Active(active));
        }
    }

    pub fn now_playing(&self, entry: Entry) {
        let _ = self.jobs.send(Job::NowPlaying(entry));
    }

    pub fn submit(&self, entries: Vec<Entry>) {
        if !entries.is_empty() {
            let _ = self.jobs.send(Job::Submit(entries));
        }
    }

    /// Drops the queued listens of a disconnected service.
    pub fn forget(&self, service: Service) {
        let _ = self.jobs.send(Job::Forget(service));
    }
}

/// The stored secret for `service`, else the environment's.
fn credential(service: Service) -> Option<String> {
    let stored = crate::store::scrobble_secret(service.secret_id())
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty());
    stored.or_else(|| match service {
        Service::LastFm => api::env_lastfm_session(),
        Service::ListenBrainz => api::env_listenbrainz_token(),
    })
}

fn send(service: Service, kind: ListenKind, entries: &[Entry]) -> Result<(), Failure> {
    let secret = credential(service)
        .ok_or_else(|| Failure::Auth(format!("{} is not connected", service.label())))?;
    match service {
        Service::LastFm => {
            let keys = api::lastfm_keys()
                .ok_or_else(|| Failure::Auth("This build has no Last.fm API key".into()))?;
            match kind {
                ListenKind::PlayingNow => entries
                    .iter()
                    .try_for_each(|e| api::lastfm_now_playing(&keys, &secret, e)),
                ListenKind::Listens => api::lastfm_scrobble(&keys, &secret, entries),
            }
        }
        Service::ListenBrainz => api::listenbrainz_submit(&secret, kind, entries),
    }
}

fn worker(
    path: PathBuf,
    rx: mpsc::Receiver<Job>,
    pending: Arc<AtomicUsize>,
    notify: impl Fn(Notice),
) {
    let mut queue = match Queue::load(&path) {
        Ok(queue) => queue,
        Err(e) => {
            // Kept aside rather than overwritten, so nothing is silently lost.
            let aside = path.with_extension("json.corrupt");
            let _ = std::fs::rename(&path, &aside);
            notify(Notice::QueueError(format!(
                "{e:#}. It was moved to {} and a new queue started.",
                aside.display()
            )));
            Queue::default()
        }
    };
    let mut active = HashSet::new();
    let mut save_failed = false;
    loop {
        pending.store(queue.entries.len(), Ordering::Relaxed);
        let timeout = if queue.entries.is_empty() {
            Duration::from_secs(3600)
        } else {
            RETRY_EVERY
        };
        let before = queue.entries.clone();
        match rx.recv_timeout(timeout) {
            Ok(Job::NowPlaying(entry)) => {
                if active.contains(&entry.service) {
                    if let Err(Failure::Auth(e)) = send(
                        entry.service,
                        ListenKind::PlayingNow,
                        std::slice::from_ref(&entry),
                    ) {
                        active.remove(&entry.service);
                        notify(Notice::AuthFailed(entry.service, e));
                    }
                }
                // Now playing is best effort; a good moment to retry what is queued.
            }
            Ok(Job::Submit(entries)) => entries.into_iter().for_each(|e| queue.push(e)),
            Ok(Job::Forget(service)) => queue.forget(service),
            Ok(Job::Active(services)) => active = services,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        for notice in flush(&mut queue, &mut active, |service, batch| {
            send(service, ListenKind::Listens, batch)
        }) {
            notify(notice);
        }
        if queue.entries != before {
            match queue.save(&path) {
                Ok(()) => save_failed = false,
                Err(e) if !save_failed => {
                    save_failed = true;
                    notify(Notice::QueueError(format!("{e:#}")));
                }
                Err(_) => {}
            }
        }
    }
}

/// The whole Last.fm desktop sign-in, run off the GUI thread: get a token, open the approval
/// page in the browser, then wait (up to ten minutes) for the user to approve it. Saves the
/// session key and returns the Last.fm user name. YTDugem never sees the password.
pub fn lastfm_sign_in(cancelled: &std::sync::atomic::AtomicBool) -> Result<String> {
    let keys = api::lastfm_keys().context("This build has no Last.fm API key")?;
    let token = api::lastfm_token(&keys)?;
    let url = api::lastfm_auth_url(&keys, &token).context("Last.fm sent an unusable token")?;
    crate::update::open_browser(&url)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(600);
    loop {
        for _ in 0..12 {
            if cancelled.load(Ordering::Relaxed) {
                anyhow::bail!("Last.fm sign-in cancelled");
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        match api::lastfm_session(&keys, &token) {
            Ok(Some((name, key))) => {
                crate::store::save_scrobble_secret(Service::LastFm.secret_id(), &key)?;
                return Ok(name);
            }
            Ok(None) | Err(Failure::Retry(_)) => {}
            Err(e) => return Err(e.into()),
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("Timed out waiting for approval on Last.fm");
        }
    }
}

/// Checks and saves a ListenBrainz token, or removes it when blank. Returns the account name.
pub fn save_listenbrainz_token(token: &str) -> Result<Option<String>> {
    let token = token.trim();
    if token.is_empty() {
        crate::store::save_scrobble_secret(Service::ListenBrainz.secret_id(), "")?;
        return Ok(None);
    }
    let user = api::listenbrainz_validate(token)?;
    crate::store::save_scrobble_secret(Service::ListenBrainz.secret_id(), token)?;
    Ok(Some(user))
}

/// Removes the saved Last.fm session key.
pub fn forget_lastfm() -> Result<()> {
    crate::store::save_scrobble_secret(Service::LastFm.secret_id(), "")
}

/// Whether each service has a credential (stored or from the environment), for Settings.
pub fn configured() -> Result<(bool, bool)> {
    let stored = |service: Service| -> Result<bool> {
        Ok(crate::store::scrobble_secret(service.secret_id())?
            .is_some_and(|s| !s.trim().is_empty()))
    };
    Ok((
        stored(Service::LastFm)? || api::env_lastfm_session().is_some(),
        stored(Service::ListenBrainz)? || api::env_listenbrainz_token().is_some(),
    ))
}

pub fn now() -> i64 {
    stats::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(artist: &str, title: &str, duration: f64) -> Track {
        Track {
            id: "abcdefghijk".into(),
            title: title.into(),
            artist: artist.into(),
            album: "Album".into(),
            duration,
            ..Default::default()
        }
    }

    // Ported from upstream DesktopScrobbleTest.kt.
    #[test]
    fn a_joint_credit_is_cut_back_to_the_first_artist() {
        assert_eq!(primary_artist("Kendrick Lamar, SZA"), "Kendrick Lamar");
        assert_eq!(primary_artist("Calvin Harris & Dua Lipa"), "Calvin Harris");
        assert_eq!(primary_artist("Yoasobi ＆ Ikura"), "Yoasobi");
    }

    #[test]
    fn a_name_with_no_separator_is_left_alone() {
        assert_eq!(primary_artist("AC/DC"), "AC/DC");
        assert_eq!(
            primary_artist("Florence + The Machine"),
            "Florence + The Machine"
        );
        // `&` needs whitespace on both sides to separate credits.
        assert_eq!(primary_artist("Mumford&Sons"), "Mumford&Sons");
    }

    #[test]
    fn a_band_whose_own_name_holds_a_separator_is_cut_too() {
        assert_eq!(primary_artist("Simon & Garfunkel"), "Simon");
        assert_eq!(primary_artist("Earth, Wind & Fire"), "Earth");
    }

    #[test]
    fn an_empty_leading_credit_falls_back_to_the_whole_name() {
        assert_eq!(primary_artist(", SZA"), ", SZA");
    }

    #[test]
    fn the_defaults_are_androids_own() {
        let s = ScrobbleSettings::default();
        assert_eq!(
            (s.min_duration, s.delay_percent, s.delay_seconds),
            (30, 0.5, 180)
        );
        assert!(!s.lastfm_enabled && !s.listenbrainz_enabled);
        assert!(s.lastfm_scrobble && s.lastfm_now_playing);
    }

    #[test]
    fn settings_are_clamped_and_old_libraries_get_defaults() {
        let mut s: ScrobbleSettings =
            serde_json::from_str(r#"{"delay_percent": 7.0, "min_duration": 1}"#).unwrap();
        s.sanitize();
        assert_eq!(
            (s.delay_percent, s.min_duration, s.delay_seconds),
            (1.0, 15, 180)
        );
        assert!(s.lastfm_now_playing);
    }

    #[test]
    fn entries_are_cleaned_and_need_artist_and_title() {
        let mut s = ScrobbleSettings::default();
        let t = track("  Calvin Harris &  Dua Lipa ", "One \n Kiss", 214.6);
        let e = Entry::new(Service::LastFm, &s, &t, 214.6, 99).unwrap();
        assert_eq!(
            (e.artist.as_str(), e.track.as_str()),
            ("Calvin Harris & Dua Lipa", "One Kiss")
        );
        assert_eq!((e.duration, e.timestamp), (215, 99));
        s.lastfm_primary_artist = true;
        assert_eq!(
            Entry::new(Service::LastFm, &s, &t, 0.0, 1).unwrap().artist,
            "Calvin Harris"
        );
        assert_eq!(
            Entry::new(Service::ListenBrainz, &s, &t, 0.0, 1)
                .unwrap()
                .artist,
            "Calvin Harris & Dua Lipa"
        );
        // A local file without an artist tag is not scrobbled.
        assert_eq!(
            Entry::new(Service::LastFm, &s, &track(" ", "Song", 200.0), 200.0, 1),
            None
        );
    }

    #[test]
    fn threshold_is_the_share_or_the_cap_and_skips_short_tracks() {
        let s = ScrobbleSettings::default();
        assert_eq!(s.threshold(200.0), Some(100.0));
        assert_eq!(
            s.threshold(600.0),
            Some(180.0),
            "capped at the maximum delay"
        );
        assert_eq!(
            s.threshold(30.0),
            None,
            "30 s is not longer than the minimum"
        );
        assert_eq!(s.threshold(31.0), Some(15.5));
        assert_eq!(s.threshold(0.0), None, "unknown length");
        assert_eq!(s.threshold(f64::NAN), None);
        let four_minutes = ScrobbleSettings {
            delay_seconds: 240,
            ..Default::default()
        };
        assert_eq!(four_minutes.threshold(3600.0), Some(240.0));
    }

    /// Plays `seconds` of position progress in 0.5 s polls from `from`.
    fn play(
        tracker: &mut Tracker,
        s: &ScrobbleSettings,
        t: &Track,
        from: f64,
        seconds: f64,
        playing: bool,
    ) -> Vec<Action> {
        let mut actions = vec![];
        let mut position = from;
        while position <= from + seconds {
            actions.extend(tracker.tick(Some((1, t)), position, t.duration, playing, s, 1000));
            if playing {
                position += 0.5;
            } else {
                position += seconds + 1.0;
            }
        }
        actions
    }

    fn scrobbles(actions: &[Action]) -> usize {
        actions
            .iter()
            .filter(|a| matches!(a, Action::Scrobble { .. }))
            .count()
    }

    #[test]
    fn announces_on_start_and_scrobbles_once_after_the_threshold() {
        let s = ScrobbleSettings::default();
        let t = track("A", "B", 200.0);
        let mut tracker = Tracker::default();
        // Loading: nothing yet.
        assert!(tracker
            .tick(Some((1, &t)), 0.0, 0.0, false, &s, 1000)
            .is_empty());
        let first = play(&mut tracker, &s, &t, 0.0, 99.0, true);
        assert!(matches!(first.as_slice(), [Action::NowPlaying { .. }]));
        let rest = play(&mut tracker, &s, &t, 99.5, 100.0, true);
        assert_eq!(scrobbles(&rest), 1);
        assert!(rest.iter().any(
            |a| matches!(a, Action::Scrobble { started: 1000, duration, .. } if *duration == 200.0)
        ));
        // Replaying from the start in the same play does not scrobble again.
        assert_eq!(scrobbles(&play(&mut tracker, &s, &t, 0.0, 150.0, true)), 0);
        // A new play of the same track does.
        let again: Vec<Action> = (0..=400)
            .flat_map(|i| tracker.tick(Some((2, &t)), f64::from(i) * 0.5, 200.0, true, &s, 2000))
            .collect();
        assert_eq!(scrobbles(&again), 1);
    }

    #[test]
    fn long_tracks_scrobble_at_the_cap() {
        let s = ScrobbleSettings::default();
        let t = track("A", "Long mix", 3600.0);
        let mut tracker = Tracker::default();
        assert_eq!(scrobbles(&play(&mut tracker, &s, &t, 0.0, 179.0, true)), 0);
        assert_eq!(scrobbles(&play(&mut tracker, &s, &t, 179.5, 2.0, true)), 1);
    }

    #[test]
    fn pauses_and_seeks_do_not_count() {
        let s = ScrobbleSettings::default();
        let t = track("A", "B", 200.0);
        let mut tracker = Tracker::default();
        play(&mut tracker, &s, &t, 0.0, 50.0, true);
        // Paused while the position somehow moves: not listening.
        assert_eq!(
            scrobbles(&play(&mut tracker, &s, &t, 50.0, 100.0, false)),
            0
        );
        // Seek to near the end: the jump does not count either.
        assert!(tracker
            .tick(Some((1, &t)), 190.0, 200.0, true, &s, 1000)
            .iter()
            .all(|a| !matches!(a, Action::Scrobble { .. })));
        assert_eq!(scrobbles(&play(&mut tracker, &s, &t, 190.0, 9.0, true)), 0);
        // Seek back and keep listening: 50 + 9 + 41 s reaches the 100 s threshold.
        assert_eq!(scrobbles(&play(&mut tracker, &s, &t, 10.0, 42.0, true)), 1);
    }

    #[test]
    fn short_and_untitled_tracks_never_scrobble() {
        let s = ScrobbleSettings::default();
        let short = track("A", "Jingle", 25.0);
        let mut tracker = Tracker::default();
        let actions = play(&mut tracker, &s, &short, 0.0, 25.0, true);
        assert_eq!(scrobbles(&actions), 0);
        assert_eq!(actions.len(), 1, "still announced as now playing");
        assert!(tracker.tick(None, 0.0, 0.0, false, &s, 0).is_empty());
    }

    fn entry(service: Service, at: i64) -> Entry {
        Entry {
            service,
            artist: "A".into(),
            track: "T".into(),
            album: String::new(),
            duration: 0,
            timestamp: at,
        }
    }

    #[test]
    fn queue_is_bounded_and_batches_keep_order() {
        let mut q = Queue::default();
        for i in 0..(MAX_QUEUE as i64 + 20) {
            q.push(entry(
                if i % 2 == 0 {
                    Service::LastFm
                } else {
                    Service::ListenBrainz
                },
                i,
            ));
        }
        assert_eq!(q.entries.len(), MAX_QUEUE);
        assert_eq!(q.entries[0].timestamp, 20, "the oldest are dropped");
        let batch = q.batch(Service::LastFm);
        assert_eq!(batch.len(), BATCH);
        assert!(batch.windows(2).all(|w| w[0].timestamp < w[1].timestamp));
        q.remove(Service::LastFm, BATCH);
        assert_eq!(q.entries.len(), MAX_QUEUE - BATCH);
        assert_eq!(q.batch(Service::LastFm)[0].timestamp, 120);
        q.forget(Service::ListenBrainz);
        assert!(q.entries.iter().all(|e| e.service == Service::LastFm));
    }

    #[test]
    fn flush_sends_in_batches_keeps_failures_and_stops_on_auth_errors() {
        let mut q = Queue::default();
        for i in 0..120 {
            q.push(entry(Service::LastFm, i));
        }
        for i in 0..3 {
            q.push(entry(Service::ListenBrainz, i));
        }
        let mut active: HashSet<Service> = Service::ALL.into_iter().collect();
        // Last.fm: first batch accepted, second hits a network error.
        let mut calls = vec![];
        let notices = flush(&mut q, &mut active, |service, batch| {
            calls.push((service, batch.len(), batch[0].timestamp));
            match (service, calls.len()) {
                (Service::LastFm, 1) => Ok(()),
                (Service::LastFm, _) => Err(Failure::Retry("offline".into())),
                (Service::ListenBrainz, _) => Err(Failure::Auth("bad token".into())),
            }
        });
        assert_eq!(
            calls,
            [
                (Service::LastFm, 50, 0),
                (Service::LastFm, 50, 50),
                (Service::ListenBrainz, 3, 0)
            ]
        );
        assert_eq!(
            notices,
            [Notice::AuthFailed(
                Service::ListenBrainz,
                "bad token".into()
            )]
        );
        assert!(!active.contains(&Service::ListenBrainz));
        assert_eq!(
            q.batch(Service::LastFm)[0].timestamp,
            50,
            "kept with its time"
        );
        assert_eq!(
            q.batch(Service::ListenBrainz).len(),
            3,
            "kept for after reconnecting"
        );
        // Next try: everything goes, a refused batch is dropped, not retried forever.
        let mut sent = 0;
        flush(&mut q, &mut active, |_, batch| {
            sent += batch.len();
            if batch[0].timestamp == 100 {
                Err(Failure::Rejected("bad".into()))
            } else {
                Ok(())
            }
        });
        assert_eq!(sent, 70);
        assert!(q.batch(Service::LastFm).is_empty());
    }

    #[test]
    fn queue_round_trips_and_corruption_is_an_error() {
        let dir = std::env::temp_dir().join(format!("ytdugem-scrobble-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = queue_path(&dir.join("library.json"));
        assert_eq!(Queue::load(&path).unwrap(), Queue::default());
        let mut q = Queue::default();
        q.push(entry(Service::LastFm, 5));
        q.save(&path).unwrap();
        assert_eq!(Queue::load(&path).unwrap(), q);
        std::fs::write(&path, b"{nope").unwrap();
        assert!(Queue::load(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
