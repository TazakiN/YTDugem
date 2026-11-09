//! Listening statistics (a YTDugem addition): a compact log of plays kept in `stats.json` beside
//! `library.json`, and the pure aggregation behind the Stats page. Nothing here leaves the
//! computer.
use crate::model::Track;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Plays older than this are dropped on load and save (about two years).
pub const KEEP_SECONDS: i64 = 730 * DAY;
/// At most this many plays are kept; the oldest go first.
pub const MAX_EVENTS: usize = 40_000;
/// A position jump larger than this between two polls is a seek, not listening.
const MAX_STEP: f64 = 5.0;
const DAY: i64 = 86_400;
/// `stats.json` refuses to load past this size instead of reading something unexpected.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// One counted play.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayEvent {
    pub track: Track,
    /// When the play started, in UTC unix seconds.
    pub at: i64,
    /// Seconds of the track actually heard (playback position progress, not wall clock).
    pub listened: f64,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsLog {
    pub events: Vec<PlayEvent>,
}

pub fn stats_path(library: &Path) -> PathBuf {
    library.with_file_name("stats.json")
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

impl StatsLog {
    /// A missing file is an empty log; an unreadable or invalid one is an error and is left as is.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        if std::fs::metadata(path)?.len() > MAX_FILE_BYTES {
            anyhow::bail!("Listening stats file is too large; original file preserved");
        }
        let bytes = std::fs::read(path).context("Cannot read listening stats")?;
        let mut log: Self = serde_json::from_slice(&bytes)
            .context("Invalid listening stats; original file preserved")?;
        log.events
            .retain(|e| e.listened.is_finite() && e.listened >= 0.0);
        log.prune(now());
        Ok(log)
    }

    /// Atomic save with a last-known-good `.bak`, like the library.
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
        if path.exists() {
            std::fs::copy(path, path.with_extension("json.bak"))?;
        }
        std::fs::rename(temp, path).context("Cannot commit listening stats")
    }

    pub fn push(&mut self, event: PlayEvent) {
        self.events.push(event);
        self.prune(now());
    }

    /// Drops plays older than `KEEP_SECONDS` and the oldest past `MAX_EVENTS`.
    pub fn prune(&mut self, now: i64) {
        self.events.retain(|e| e.at >= now - KEEP_SECONDS);
        if self.events.len() > MAX_EVENTS {
            self.events.sort_by_key(|e| e.at);
            let extra = self.events.len() - MAX_EVENTS;
            self.events.drain(..extra);
        }
    }
}

/// Seconds a track must be heard to count: 30 s, or half of a shorter track.
pub fn threshold(duration: f64) -> f64 {
    if duration.is_finite() && duration > 0.0 {
        (duration * 0.5).min(30.0)
    } else {
        30.0
    }
}

/// The play in progress: listening time grows only while the position advances in small steps,
/// so pauses, seeks and stalls never inflate it.
#[derive(Clone, Debug)]
pub struct Listen {
    pub track: Track,
    pub started: i64,
    pub listened: f64,
    last: Option<f64>,
}

impl Listen {
    pub fn new(track: Track, started: i64) -> Self {
        Self {
            track,
            started,
            listened: 0.0,
            last: None,
        }
    }
    pub fn key(&self) -> &str {
        self.track.key()
    }
    /// Feed the latest playback position; `playing` is false while paused, ended or loading.
    pub fn advance(&mut self, position: f64, playing: bool) {
        if !position.is_finite() {
            return;
        }
        if let Some(last) = self.last.filter(|_| playing) {
            let step = position - last;
            if step > 0.0 && step <= MAX_STEP {
                self.listened += step;
            }
        }
        self.last = Some(position);
    }
    /// The play to record, if enough of the track was heard.
    pub fn finish(self, duration: f64) -> Option<PlayEvent> {
        let duration = if duration > 0.0 {
            duration
        } else {
            self.track.duration
        };
        (self.listened >= threshold(duration)).then_some(PlayEvent {
            track: self.track,
            at: self.started,
            listened: self.listened,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Period {
    Week,
    #[default]
    Month,
    Year,
    All,
}

impl Period {
    pub const ALL: [Period; 4] = [Period::Week, Period::Month, Period::Year, Period::All];
    pub fn label(self) -> &'static str {
        match self {
            Period::Week => "Last 7 days",
            Period::Month => "Last 30 days",
            Period::Year => "This year",
            Period::All => "All time",
        }
    }
    /// The first second (UTC) the period covers; `offset` is the local UTC offset in seconds.
    pub fn start(self, now: i64, offset: i64) -> Option<i64> {
        match self {
            Period::Week => Some(now - 7 * DAY),
            Period::Month => Some(now - 30 * DAY),
            Period::Year => Some(year_start(local_year(now, offset), offset)),
            Period::All => None,
        }
    }
}

/// The plays of a period (ending now).
pub fn in_period(events: &[PlayEvent], period: Period, now: i64, offset: i64) -> Vec<&PlayEvent> {
    let start = period.start(now, offset);
    events
        .iter()
        .filter(|e| start.is_none_or(|s| e.at >= s) && e.at <= now)
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub seconds: f64,
    pub plays: usize,
    pub tracks: usize,
    pub artists: usize,
}

pub fn summary(events: &[&PlayEvent]) -> Summary {
    let tracks: std::collections::HashSet<&str> = events.iter().map(|e| e.track.key()).collect();
    let artists: std::collections::HashSet<String> = events
        .iter()
        .flat_map(|e| artists_of(&e.track))
        .map(|a| a.0)
        .collect();
    Summary {
        seconds: events.iter().map(|e| e.listened).sum(),
        plays: events.len(),
        tracks: tracks.len(),
        artists: artists.len(),
    }
}

/// One row of a top list.
#[derive(Clone, Debug, PartialEq)]
pub struct Ranked {
    pub title: String,
    pub subtitle: String,
    pub thumbnail: String,
    pub plays: usize,
    pub seconds: f64,
    /// The most recent play, used to break ties.
    pub last: i64,
    /// A track to play (top tracks) and, for artists, the linked page if known.
    pub track: Track,
    pub browse_id: Option<String>,
}

/// Lower-cased key and display name of each credited artist.
fn artists_of(track: &Track) -> Vec<(String, String, Option<String>)> {
    if !track.artists.is_empty() {
        return track
            .artists
            .iter()
            .filter(|a| !a.name.trim().is_empty())
            .map(|a| {
                let id = Some(a.browse_id.clone()).filter(|id| !id.is_empty());
                (a.name.trim().to_lowercase(), a.name.trim().to_owned(), id)
            })
            .collect();
    }
    let name = track.artist.trim();
    if name.is_empty() {
        vec![]
    } else {
        vec![(name.to_lowercase(), name.to_owned(), None)]
    }
}

/// Most plays first; ties go to more time listened, then the most recent play, then the title.
fn rank(map: HashMap<String, Ranked>, n: usize) -> Vec<Ranked> {
    let mut rows: Vec<Ranked> = map.into_values().collect();
    rows.sort_by(|a, b| {
        b.plays
            .cmp(&a.plays)
            .then(b.seconds.total_cmp(&a.seconds))
            .then(b.last.cmp(&a.last))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    rows.truncate(n);
    rows
}

fn tally(
    map: &mut HashMap<String, Ranked>,
    key: String,
    event: &PlayEvent,
    make: impl FnOnce() -> Ranked,
) {
    let row = map.entry(key).or_insert_with(make);
    row.plays += 1;
    row.seconds += event.listened;
    if event.at >= row.last {
        row.last = event.at;
        if !event.track.thumbnail.is_empty() {
            row.thumbnail = event.track.thumbnail.clone();
        }
    }
}

fn ranked(title: &str, subtitle: &str, event: &PlayEvent) -> Ranked {
    Ranked {
        title: title.to_owned(),
        subtitle: subtitle.to_owned(),
        thumbnail: event.track.thumbnail.clone(),
        plays: 0,
        seconds: 0.0,
        last: event.at,
        track: event.track.clone(),
        browse_id: None,
    }
}

pub fn top_tracks(events: &[&PlayEvent], n: usize) -> Vec<Ranked> {
    let mut map = HashMap::new();
    for e in events {
        tally(&mut map, e.track.key().to_owned(), e, || {
            ranked(&e.track.title, &e.track.artist, e)
        });
    }
    rank(map, n)
}

pub fn top_artists(events: &[&PlayEvent], n: usize) -> Vec<Ranked> {
    let mut map = HashMap::new();
    for e in events {
        for (key, name, id) in artists_of(&e.track) {
            tally(&mut map, key.clone(), e, || ranked(&name, "", e));
            if let Some(row) = map.get_mut(&key).filter(|r| r.browse_id.is_none()) {
                row.browse_id = id;
            }
        }
    }
    rank(map, n)
}

pub fn top_albums(events: &[&PlayEvent], n: usize) -> Vec<Ranked> {
    let mut map = HashMap::new();
    for e in events {
        let album = e.track.album.trim();
        if album.is_empty() {
            continue;
        }
        let artist = artists_of(&e.track)
            .first()
            .map(|a| a.1.clone())
            .unwrap_or_default();
        let key = format!("{}\n{}", album.to_lowercase(), artist.to_lowercase());
        tally(&mut map, key, e, || ranked(album, &artist, e));
    }
    rank(map, n)
}

/// Seconds listened per local hour of the day (0–23).
pub fn by_hour(events: &[&PlayEvent], offset: i64) -> [f64; 24] {
    let mut hours = [0.0; 24];
    for e in events {
        let local = e.at + offset;
        hours[(local.rem_euclid(DAY) / 3600) as usize] += e.listened;
    }
    hours
}

/// Seconds listened per local weekday, Monday first.
pub fn by_weekday(events: &[&PlayEvent], offset: i64) -> [f64; 7] {
    let mut days = [0.0; 7];
    for e in events {
        days[weekday(e.at, offset)] += e.listened;
    }
    days
}

/// Local weekday of a UTC time, 0 = Monday (1970-01-01 was a Thursday).
pub fn weekday(at: i64, offset: i64) -> usize {
    ((at + offset).div_euclid(DAY) + 3).rem_euclid(7) as usize
}

pub fn local_year(at: i64, offset: i64) -> i64 {
    civil_from_days((at + offset).div_euclid(DAY)).0
}

/// UTC unix time of local midnight on 1 January of `year`.
pub fn year_start(year: i64, offset: i64) -> i64 {
    days_from_civil(year, 1, 1) * DAY - offset
}

/// Howard Hinnant's `days_from_civil`: days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse: (year, month, day) of days since 1970-01-01.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        m,
        d,
    )
}

/// The computer's current offset from UTC in seconds (east positive). Older plays are bucketed
/// with today's offset, so an hour may shift across a daylight-saving change.
pub fn local_offset() -> i64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        /// `TIME_ZONE_ID_DAYLIGHT` (winnt.h); windows-sys does not export it.
        const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
        // SAFETY: the struct is plain data the call fills in.
        let mut info: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
        let id = unsafe { GetTimeZoneInformation(&mut info) };
        if id == TIME_ZONE_ID_INVALID {
            return 0;
        }
        let mut bias = info.Bias;
        if id == TIME_ZONE_ID_DAYLIGHT {
            bias += info.DaylightBias;
        }
        -(bias as i64) * 60
    }
    #[cfg(not(windows))]
    {
        // Linux is a compatibility target only: bucket in UTC there.
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ArtistRef;

    fn track(id: &str, artist: &str, album: &str) -> Track {
        Track {
            id: id.into(),
            title: format!("Song {id}"),
            artist: artist.into(),
            album: album.into(),
            duration: 200.0,
            ..Default::default()
        }
    }
    fn play(id: &str, artist: &str, at: i64, listened: f64) -> PlayEvent {
        PlayEvent {
            track: track(id, artist, "Album"),
            at,
            listened,
        }
    }

    #[test]
    fn play_counts_after_thirty_seconds_or_half_a_short_track() {
        assert_eq!(threshold(200.0), 30.0);
        assert_eq!(threshold(40.0), 20.0);
        assert_eq!(threshold(0.0), 30.0);
        assert_eq!(threshold(f64::NAN), 30.0);
        let mut listen = Listen::new(track("a", "A", ""), 100);
        for i in 0..=290 {
            listen.advance(i as f64 * 0.1, true);
        }
        assert!(listen.clone().finish(200.0).is_none(), "29 s is not a play");
        listen.advance(29.5, true);
        listen.advance(30.1, true);
        let event = listen.finish(200.0).unwrap();
        assert_eq!(event.at, 100);
        assert!((event.listened - 30.1).abs() < 1e-6);
        let mut short = Listen::new(track("b", "B", ""), 0);
        short.advance(0.0, true);
        short.advance(4.0, true);
        short.advance(8.0, true);
        assert!(short.finish(16.0).is_some(), "half of a 16 s track");
    }

    #[test]
    fn pauses_and_seeks_do_not_count_as_listening() {
        let mut listen = Listen::new(track("a", "A", ""), 0);
        listen.advance(0.0, true);
        listen.advance(1.0, true);
        // Paused: the position stays put and nothing is added, however long it waits.
        for _ in 0..100 {
            listen.advance(1.0, false);
        }
        // A seek forward and one back.
        listen.advance(120.0, true);
        listen.advance(121.0, true);
        listen.advance(10.0, true);
        listen.advance(11.0, true);
        // Progress reported while paused (a seek while paused) is not listening either.
        listen.advance(14.0, false);
        assert!((listen.listened - 3.0).abs() < 1e-9, "{}", listen.listened);
        listen.advance(f64::NAN, true);
        assert!((listen.listened - 3.0).abs() < 1e-9);
    }

    #[test]
    fn periods_filter_by_time_and_local_year() {
        let offset = 7 * 3600;
        // 2026-11-09 12:00 UTC.
        let now = days_from_civil(2026, 11, 9) * DAY + 12 * 3600;
        let events = vec![
            play("old", "A", now - 400 * DAY, 60.0),
            play("month", "A", now - 20 * DAY, 60.0),
            play("week", "A", now - 2 * DAY, 60.0),
            // 2026-01-01 00:30 local (2025-12-31 17:30 UTC): this year locally.
            play("newyear", "A", year_start(2026, offset) + 1800, 60.0),
            // 2025-12-31 23:30 local: last year.
            play("lastyear", "A", year_start(2026, offset) - 1800, 60.0),
            play("future", "A", now + 10, 60.0),
        ];
        let ids = |p| -> Vec<String> {
            in_period(&events, p, now, offset)
                .iter()
                .map(|e| e.track.id.clone())
                .collect()
        };
        assert_eq!(ids(Period::Week), ["week"]);
        assert_eq!(ids(Period::Month), ["month", "week"]);
        assert_eq!(ids(Period::Year), ["month", "week", "newyear"]);
        assert_eq!(ids(Period::All).len(), 5);
        assert_eq!(local_year(year_start(2026, offset) - 1, offset), 2025);
        assert_eq!(local_year(year_start(2026, offset), offset), 2026);
    }

    #[test]
    fn top_lists_rank_by_plays_then_time_then_recency() {
        let mut events = vec![
            play("a", "Alpha", 10, 100.0),
            play("a", "Alpha", 20, 100.0),
            play("b", "Beta", 30, 150.0),
            play("b", "Beta", 40, 100.0),
            play("c", "Gamma", 50, 60.0),
            play("d", "Delta", 60, 60.0),
        ];
        events[4].track.album = "Other".into();
        let refs: Vec<&PlayEvent> = events.iter().collect();
        let tracks = top_tracks(&refs, 10);
        let ids: Vec<&str> = tracks.iter().map(|r| r.track.id.as_str()).collect();
        // b and a tie on plays; b listened longer. c and d tie on both; d played later.
        assert_eq!(ids, ["b", "a", "d", "c"]);
        assert_eq!(tracks[0].plays, 2);
        assert_eq!(top_tracks(&refs, 2).len(), 2);
        let artists: Vec<String> = top_artists(&refs, 10)
            .into_iter()
            .map(|r| r.title)
            .collect();
        assert_eq!(artists, ["Beta", "Alpha", "Delta", "Gamma"]);
        let albums = top_albums(&refs, 10);
        assert_eq!(albums[0].plays, 2);
        assert_eq!(albums.len(), 4, "album is keyed with its artist");
        let s = summary(&refs);
        assert_eq!((s.plays, s.tracks, s.artists), (6, 4, 4));
        assert_eq!(s.seconds, 570.0);
    }

    #[test]
    fn linked_artists_count_each_credit_and_keep_their_page() {
        let mut duet = track("x", "A & B", "");
        duet.artists = vec![
            ArtistRef {
                name: "A".into(),
                browse_id: "UCa".into(),
            },
            ArtistRef {
                name: "B".into(),
                browse_id: String::new(),
            },
        ];
        let events = [PlayEvent {
            track: duet,
            at: 0,
            listened: 40.0,
        }];
        let refs: Vec<&PlayEvent> = events.iter().collect();
        let artists = top_artists(&refs, 5);
        assert_eq!(artists.len(), 2);
        let a = artists.iter().find(|r| r.title == "A").unwrap();
        assert_eq!(a.browse_id.as_deref(), Some("UCa"));
        assert!(artists
            .iter()
            .any(|r| r.title == "B" && r.browse_id.is_none()));
        assert!(top_albums(&refs, 5).is_empty(), "no album, no row");
    }

    #[test]
    fn hours_and_weekdays_are_bucketed_in_local_time() {
        // 2026-11-09 (a Monday) 23:30 UTC is Tuesday 06:30 at UTC+7 and Monday 18:30 at UTC-5.
        let at = days_from_civil(2026, 11, 9) * DAY + 23 * 3600 + 1800;
        let events = [play("a", "A", at, 90.0)];
        let refs: Vec<&PlayEvent> = events.iter().collect();
        assert_eq!(weekday(at, 0), 0);
        let east = by_hour(&refs, 7 * 3600);
        assert_eq!(east[6], 90.0);
        assert_eq!(by_weekday(&refs, 7 * 3600)[1], 90.0);
        let west = by_hour(&refs, -5 * 3600);
        assert_eq!(west[18], 90.0);
        assert_eq!(by_weekday(&refs, -5 * 3600)[0], 90.0);
        assert_eq!(by_hour(&refs, 0)[23], 90.0);
        assert_eq!(civil_from_days(days_from_civil(2024, 2, 29)), (2024, 2, 29));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    #[test]
    fn log_is_bounded_and_corruption_is_not_overwritten() {
        let now = 1_000_000_000;
        let mut log = StatsLog {
            events: vec![play("old", "A", now - KEEP_SECONDS - 1, 40.0)],
        };
        log.events
            .extend((0..MAX_EVENTS + 5).map(|i| play("n", "A", now - 1000 + i as i64, 40.0)));
        log.prune(now);
        assert_eq!(log.events.len(), MAX_EVENTS);
        assert!(log.events.iter().all(|e| e.at >= now - 1000 + 5));

        let dir = std::env::temp_dir().join(format!("ytdugem-stats-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = stats_path(&dir.join("library.json"));
        assert_eq!(path.file_name().unwrap(), "stats.json");
        assert!(StatsLog::load(&path).unwrap().events.is_empty());
        let fresh = StatsLog {
            events: vec![play("a", "A", super::now(), 40.0)],
        };
        fresh.save(&path).unwrap();
        fresh.save(&path).unwrap();
        assert!(path.with_extension("json.bak").exists());
        assert_eq!(StatsLog::load(&path).unwrap().events, fresh.events);
        std::fs::write(&path, "broken").unwrap();
        assert!(StatsLog::load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken");
        // An old file without unknown fields still loads.
        std::fs::write(&path, "{}").unwrap();
        assert!(StatsLog::load(&path).unwrap().events.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
