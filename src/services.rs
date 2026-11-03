use crate::model::Track;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub mod lyrics;

#[derive(Clone, Debug, PartialEq)]
pub struct LyricLine {
    pub time: f64,
    pub text: String,
    pub words: Vec<crate::model::TimedWord>,
}

#[derive(Clone, Debug, Default)]
pub struct ShelfItem {
    pub title: String,
    pub subtitle: String,
    pub thumbnail: String,
    pub video_id: Option<String>,
    pub browse_id: Option<String>,
    pub params: Option<String>,
    pub duration: f64,
}
impl ShelfItem {
    pub fn track(&self) -> Option<Track> {
        let id = self.video_id.as_ref()?;
        Some(Track {
            id: id.clone(),
            title: self.title.clone(),
            artist: self
                .subtitle
                .split(['•', '·'])
                .next()
                .unwrap_or("")
                .trim()
                .to_owned(),
            album: String::new(),
            duration: self.duration,
            thumbnail: self.thumbnail.clone(),
            source: format!("https://music.youtube.com/watch?v={id}"),
        })
    }
}
#[derive(Clone, Debug, Default)]
pub struct HomeShelf {
    pub title: String,
    pub subtitle: String,
    pub items: Vec<ShelfItem>,
}
#[derive(Clone, Debug, Default)]
pub struct HomeFeed {
    pub shelves: Vec<HomeShelf>,
    pub continuation: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct Collection {
    pub title: String,
    pub subtitle: String,
    pub thumbnail: String,
    pub tracks: Vec<Track>,
    pub continuation: Option<String>,
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(25))
        .user_agent("Mozilla/5.0 YTDugem/0.1")
        .build()?)
}

static SIGN_IN_REJECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True once since YouTube last refused the saved sign-in (expired or signed-out cookies).
pub fn take_sign_in_rejected() -> bool {
    SIGN_IN_REJECTED.swap(false, std::sync::atomic::Ordering::SeqCst)
}

fn send_innertube(
    endpoint: &str,
    body: &Value,
    session: Option<&crate::auth::Session>,
) -> Result<(reqwest::StatusCode, Value)> {
    let response = innertube_request(endpoint, body, session)?.send()?;
    let status = response.status();
    Ok((status, response.json().unwrap_or(Value::Null)))
}

pub(crate) fn innertube_request(
    endpoint: &str,
    body: &Value,
    session: Option<&crate::auth::Session>,
) -> Result<reqwest::blocking::RequestBuilder> {
    let mut body = body.clone();
    body["context"] = match session {
        Some(session) => session.context(),
        None => {
            json!({"client":{"clientName":"WEB_REMIX","clientVersion":"1.20260601.01.00","hl":"en","gl":"US"}})
        }
    };
    let request = client()?
        .post(format!(
            "https://music.youtube.com/youtubei/v1/{endpoint}?prettyPrint=false"
        ))
        .json(&body);
    // `apply` adds its own Origin; a second Origin header makes Google answer 400.
    Ok(match session {
        Some(session) => session.apply(request),
        None => request.header("Origin", "https://music.youtube.com"),
    })
}

fn innertube_value(endpoint: &str, body: Value) -> Result<Value> {
    let session = crate::auth::snapshot();
    let (mut status, mut response) = send_innertube(endpoint, &body, session.as_ref())?;
    if session.is_some() && status == reqwest::StatusCode::UNAUTHORIZED {
        // The saved cookies no longer sign in; still show public music.
        SIGN_IN_REJECTED.store(true, std::sync::atomic::Ordering::SeqCst);
        (status, response) = send_innertube(endpoint, &body, None)?;
    }
    if !status.is_success() || response.get("error").is_some() {
        let detail = response
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("no details");
        bail!("YouTube Music {endpoint} request failed ({status}): {detail}");
    }
    Ok(response)
}

fn innertube(endpoint: &str, body: Value) -> Result<Vec<Track>> {
    let response = innertube_value(endpoint, body)?;
    let mut tracks = vec![];
    parse_renderers(&response, &mut tracks);
    let mut seen = HashSet::new();
    tracks.retain(|t| seen.insert(t.id.clone()));
    Ok(tracks)
}

fn continuation(v: &Value) -> Option<String> {
    [
        "/continuationContents",
        "/onResponseReceivedActions",
        "/onResponseReceivedEndpoints",
    ]
    .iter()
    .filter_map(|path| v.pointer(path))
    .find_map(|node| find_string(node, "continuation"))
    .or_else(|| v.get("continuation").and_then(Value::as_str))
    .filter(|s| !s.is_empty())
    .map(str::to_owned)
}
fn thumbnails(v: &Value) -> String {
    v.pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .or_else(|| v.pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails"))
        .and_then(Value::as_array)
        .and_then(|a| a.last())
        .and_then(|x| x["url"].as_str())
        .unwrap_or("")
        .to_owned()
}
fn shelf_item(v: &Value) -> ShelfItem {
    let title = text(&v["title"]).to_string();
    let subtitle = text(&v["subtitle"]);
    let nav = v.pointer("/navigationEndpoint/browseEndpoint").or_else(|| {
        v.pointer("/thumbnailRenderer/musicThumbnailRenderer/navigationEndpoint/browseEndpoint")
    });
    let browse_id = nav.and_then(|n| n["browseId"].as_str()).map(str::to_owned);
    let params = nav.and_then(|n| n["params"].as_str()).map(str::to_owned);
    let video_id = primary_video_id(v).map(str::to_owned);
    let length = text(&v["lengthText"]);
    let length = if length.is_empty() {
        text(&v["fixedColumns"][0]["musicResponsiveListItemFixedColumnRenderer"]["text"])
    } else {
        length
    };
    ShelfItem {
        title,
        subtitle,
        thumbnail: thumbnails(v),
        video_id,
        browse_id,
        params,
        duration: duration(&length),
    }
}
fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}
fn primary_video_id(v: &Value) -> Option<&str> {
    [
        "/playlistItemData/videoId",
        "/navigationEndpoint/watchEndpoint/videoId",
        "/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId",
        "/playNavigationEndpoint/watchEndpoint/videoId",
    ]
    .iter()
    .filter_map(|path| v.pointer(path).and_then(Value::as_str))
    .find(|id| valid_video_id(id))
}
fn parse_home(v: &Value) -> HomeFeed {
    let mut out = HomeFeed {
        continuation: continuation(v),
        ..Default::default()
    };
    collect_shelves(v, &mut out.shelves);
    out
}
fn collect_shelves(v: &Value, out: &mut Vec<HomeShelf>) {
    match v {
        Value::Object(m) => {
            for (kind, node) in m {
                if kind == "musicCarouselShelfRenderer" || kind == "musicShelfRenderer" {
                    let title =
                        text(&node["header"]["musicCarouselShelfBasicHeaderRenderer"]["title"]);
                    let title = if title.is_empty() {
                        text(&node["title"])
                    } else {
                        title
                    };
                    let mut items = vec![];
                    collect_cards(&node["contents"], &mut items);
                    if !title.is_empty() || !items.is_empty() {
                        out.push(HomeShelf {
                            title,
                            subtitle: String::new(),
                            items,
                        });
                    }
                } else {
                    collect_shelves(node, out)
                }
            }
        }
        Value::Array(a) => {
            for x in a {
                collect_shelves(x, out)
            }
        }
        _ => {}
    }
}
fn collect_cards(v: &Value, out: &mut Vec<ShelfItem>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == "musicTwoRowItemRenderer" {
                    let item = shelf_item(x);
                    if !item.title.is_empty() {
                        out.push(item)
                    }
                } else if k == "musicResponsiveListItemRenderer" {
                    let mut item = shelf_item(x);
                    let cols = x["flexColumns"].as_array();
                    if let Some(c) = cols {
                        item.title = text(
                            &c.first().unwrap_or(&Value::Null)
                                ["musicResponsiveListItemFlexColumnRenderer"]["text"],
                        );
                        item.subtitle = text(
                            &c.get(1).unwrap_or(&Value::Null)
                                ["musicResponsiveListItemFlexColumnRenderer"]["text"],
                        )
                    }
                    if let Some(id) = primary_video_id(x) {
                        item.video_id = Some(id.into())
                    }
                    if !item.title.is_empty() {
                        out.push(item)
                    }
                } else {
                    collect_cards(x, out)
                }
            }
        }
        Value::Array(a) => {
            for x in a {
                collect_cards(x, out)
            }
        }
        _ => {}
    }
}
/// Upstream `YtMusicRepository.HOME_SUPPLEMENT_BROWSE_IDS` (shared-jvm `<clinit>`).
pub const HOME_SUPPLEMENT_BROWSE_IDS: [&str; 2] = ["FEmusic_new_releases", "FEmusic_explore"];

pub fn home() -> Result<HomeFeed> {
    let v = innertube_value("browse", json!({"browseId":"FEmusic_home"}))?;
    Ok(parse_home(&v))
}
pub fn home_supplement(browse_id: &str) -> Result<Vec<HomeShelf>> {
    let v = innertube_value("browse", json!({"browseId":browse_id}))?;
    Ok(parse_home(&v).shelves)
}
pub fn home_more(token: &str) -> Result<HomeFeed> {
    let v = innertube_value("browse", json!({"continuation":token}))?;
    Ok(parse_home(&v))
}
pub fn recent() -> Result<Vec<Track>> {
    if crate::auth::snapshot().is_none() {
        return Ok(vec![]);
    }
    innertube("browse", json!({"browseId":"FEmusic_history"}))
}
pub fn collection(browse_id: &str, params: Option<&str>) -> Result<Collection> {
    let mut b = json!({"browseId":browse_id});
    if let Some(p) = params {
        b["params"] = json!(p)
    };
    let v = innertube_value("browse", b)?;
    Ok(parse_collection(&v))
}
pub fn collection_more(token: &str) -> Result<Collection> {
    let v = innertube_value("browse", json!({"continuation":token}))?;
    Ok(parse_collection(&v))
}
fn parse_collection(v: &Value) -> Collection {
    let mut tracks = vec![];
    parse_renderers(v, &mut tracks);
    let header = find_renderer(v, "musicResponsiveHeaderRenderer")
        .or_else(|| find_renderer(v, "musicDetailHeaderRenderer"))
        .or_else(|| find_renderer(v, "musicImmersiveHeaderRenderer"))
        .unwrap_or(&Value::Null);
    Collection {
        title: text(&header["title"]),
        subtitle: text(&header["subtitle"]),
        thumbnail: thumbnails(header),
        tracks,
        continuation: continuation(v),
    }
}
fn find_renderer<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => map
            .get(key)
            .or_else(|| map.values().find_map(|v| find_renderer(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_renderer(v, key)),
        _ => None,
    }
}
pub fn radio(video_id: &str) -> Result<Vec<Track>> {
    let v = innertube_value(
        "next",
        json!({"videoId":video_id,"playlistId":format!("RDAMVM{video_id}"),"isAudioOnly":true}),
    )?;
    let mut tracks = vec![];
    parse_renderers(&v, &mut tracks);
    collect_panel(&v, &mut tracks);
    let mut seen = HashSet::new();
    tracks.retain(|t| seen.insert(t.id.clone()));
    Ok(tracks)
}

fn collect_panel(v: &Value, out: &mut Vec<Track>) {
    match v {
        Value::Object(map) => {
            if let Some(row) = map.get("playlistPanelVideoRenderer") {
                if let Some(id) = row["videoId"].as_str().filter(|id| valid_video_id(id)) {
                    let title = text(&row["title"]);
                    if !title.is_empty() {
                        out.push(Track {
                            id: id.into(),
                            title,
                            artist: text(&row["longBylineText"]),
                            album: String::new(),
                            duration: duration(&text(&row["lengthText"])),
                            thumbnail: row
                                .pointer("/thumbnail/thumbnails")
                                .and_then(Value::as_array)
                                .and_then(|a| a.last())
                                .and_then(|t| t["url"].as_str())
                                .unwrap_or("")
                                .into(),
                            source: format!("https://music.youtube.com/watch?v={id}"),
                        });
                    }
                }
            }
            for child in map.values() {
                collect_panel(child, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_panel(child, out);
            }
        }
        _ => {}
    }
}

fn text(value: &Value) -> String {
    value["runs"]
        .as_array()
        .map(|runs| runs.iter().filter_map(|r| r["text"].as_str()).collect())
        .unwrap_or_else(|| value["simpleText"].as_str().unwrap_or("").to_owned())
}

fn find_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_str)
            .or_else(|| map.values().find_map(|v| find_string(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_string(v, key)),
        _ => None,
    }
}

fn duration(value: &str) -> f64 {
    let mut total = 0.0;
    for part in value.split(':') {
        let Ok(n) = part.trim().parse::<u32>() else {
            return 0.0;
        };
        total = total * 60.0 + f64::from(n);
    }
    total
}

fn parse_renderers(value: &Value, tracks: &mut Vec<Track>) {
    match value {
        Value::Object(map) => {
            for kind in ["musicResponsiveListItemRenderer", "musicTwoRowItemRenderer"] {
                if let Some(row) = map.get(kind) {
                    if let Some(id) = primary_video_id(row) {
                        let columns = row["flexColumns"].as_array();
                        let title = columns
                            .and_then(|c| c.first())
                            .map(|c| text(&c["musicResponsiveListItemFlexColumnRenderer"]["text"]))
                            .unwrap_or_else(|| text(&row["title"]));
                        if title.is_empty() {
                            continue;
                        }
                        let subtitle = columns
                            .and_then(|c| c.get(1))
                            .map(|c| text(&c["musicResponsiveListItemFlexColumnRenderer"]["text"]))
                            .unwrap_or_else(|| text(&row["subtitle"]));
                        let artist = subtitle
                            .split(['•', '·'])
                            .next()
                            .unwrap_or("")
                            .trim()
                            .to_owned();
                        let fixed = row["fixedColumns"][0]
                            ["musicResponsiveListItemFixedColumnRenderer"]["text"]
                            .clone();
                        let length = text(&fixed);
                        let parts: Vec<_> = subtitle.split(['•', '·']).map(str::trim).collect();
                        let seconds = if length.is_empty() {
                            parts
                                .iter()
                                .rev()
                                .find(|s| s.contains(':'))
                                .map_or(0.0, |s| duration(s))
                        } else {
                            duration(&length)
                        };
                        let album = if parts.len() >= 3 {
                            parts[1].to_owned()
                        } else {
                            String::new()
                        };
                        let thumbnail = row.pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails").or_else(||row.pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails"))
                            .and_then(Value::as_array).and_then(|t| t.last()).and_then(|t|t["url"].as_str()).unwrap_or("").to_owned();
                        tracks.push(Track {
                            id: id.to_owned(),
                            title,
                            artist,
                            album,
                            duration: seconds,
                            thumbnail,
                            source: format!("https://music.youtube.com/watch?v={id}"),
                        });
                    }
                }
            }
            for v in map.values() {
                parse_renderers(v, tracks);
            }
        }
        Value::Array(items) => {
            for v in items {
                parse_renderers(v, tracks);
            }
        }
        _ => {}
    }
}

pub fn search(query: &str) -> Result<Vec<Track>> {
    let query = query.trim();
    if query.is_empty() || query.len() > 500 {
        bail!("Enter a search between 1 and 500 characters");
    }
    innertube(
        "search",
        json!({"query":query,"params":"EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D"}),
    )
}

#[cfg(test)]
pub fn browse() -> Result<Vec<Track>> {
    innertube("browse", json!({"browseId":"FEmusic_explore"}))
}

const AUDIO: &[&str] = &[
    "mp3", "m4a", "flac", "wav", "ogg", "opus", "aac", "wma", "aiff", "alac",
];
fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| AUDIO.iter().any(|ext| s.eq_ignore_ascii_case(ext)))
}

fn files(path: &Path, out: &mut Vec<PathBuf>, depth: usize) -> Result<()> {
    // ponytail: bounded traversal skips symlinks; use a file watcher only if live rescans are needed.
    if depth > 32 {
        bail!("Music directory is nested more than 32 levels");
    }
    for entry in fs::read_dir(path).with_context(|| format!("reading {}", path.display()))? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            files(&entry.path(), out, depth + 1)?;
        } else if kind.is_file() && is_audio(&entry.path()) {
            out.push(entry.path());
        }
    }
    Ok(())
}

pub(crate) fn run(command: &mut Command, timeout: Duration) -> Result<std::process::Output> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Starting audio tool; install yt-dlp and FFmpeg")?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    // Drain both pipes while waiting: a full pipe must not deadlock a download.
    let out = thread::spawn(move || {
        let mut bytes = vec![];
        stdout
            .take(8 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let err = thread::spawn(move || {
        let mut bytes = vec![];
        stderr
            .take(8 * 1024 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Audio tool timed out after {} seconds", timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(25));
    };
    Ok(std::process::Output {
        status,
        stdout: out
            .join()
            .map_err(|_| anyhow!("Audio output reader failed"))??,
        stderr: err
            .join()
            .map_err(|_| anyhow!("Audio error reader failed"))??,
    })
}

pub fn import_local(path: &Path) -> Result<Vec<Track>> {
    let path = fs::canonicalize(path).with_context(|| format!("Opening {}", path.display()))?;
    let mut paths = vec![];
    if path.is_file() {
        if !is_audio(&path) {
            bail!("Not a supported audio file");
        }
        paths.push(path);
    } else {
        files(&path, &mut paths, 0)?;
    }
    paths.sort();
    let mut tracks = vec![];
    for p in paths {
        let fallback = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown title")
            .to_owned();
        let mut command = Command::new("ffprobe");
        command
            .args([
                "-v",
                "quiet",
                "-show_entries",
                "format=duration:format_tags=title,artist,album",
                "-of",
                "json",
            ])
            .arg(&p);
        let meta = run(&mut command, Duration::from_secs(5))
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
            .unwrap_or(Value::Null);
        let tags = &meta["format"]["tags"];
        let tag = |key: &str| {
            tags[key]
                .as_str()
                .or_else(|| tags[key.to_uppercase()].as_str())
                .unwrap_or("")
                .to_owned()
        };
        let title = tag("title");
        tracks.push(Track {
            id: p.to_string_lossy().into_owned(),
            title: if title.is_empty() { fallback } else { title },
            artist: tag("artist"),
            album: tag("album"),
            duration: meta["format"]["duration"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            thumbnail: String::new(),
            source: p.to_string_lossy().into_owned(),
        });
    }
    Ok(tracks)
}

#[allow(dead_code)] // Public compatibility entry point retained for existing callers.
pub fn lyrics(track: &Track) -> Result<Vec<LyricLine>> {
    Ok(lyrics::lookup(
        track,
        &crate::store::lyrics_order_default(),
        &crate::store::lyrics_order_default(),
        Some("lrclib"),
    )?
    .lines)
}

#[cfg(test)]
fn parse_lyrics(text: &str) -> Vec<LyricLine> {
    let mut lines = vec![];
    for line in text.lines() {
        let mut rest = line;
        let mut times = vec![];
        while let Some(stamp) = rest.strip_prefix('[').and_then(|s| s.split_once(']')) {
            let Some((minutes, seconds)) = stamp
                .0
                .split_once(':')
                .and_then(|(m, s)| Some((m.parse::<u32>().ok()?, s.parse::<f64>().ok()?)))
            else {
                break;
            };
            if !seconds.is_finite() || !(0.0..60.0).contains(&seconds) {
                break;
            }
            times.push(f64::from(minutes) * 60.0 + seconds);
            rest = stamp.1;
        }
        for time in times {
            lines.push(LyricLine {
                time,
                text: rest.trim().to_owned(),
                words: vec![],
            });
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    if lines.is_empty() {
        lines = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| LyricLine {
                time: 0.0,
                text: l.to_owned(),
                words: vec![],
            })
            .collect();
    }
    lines
}

pub fn download(track: &Track, directory: &Path) -> Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let directory = fs::canonicalize(directory)?;
    let local = Path::new(&track.source);
    if local.is_file() {
        let dest = directory.join(
            local
                .file_name()
                .ok_or_else(|| anyhow!("Invalid file name"))?,
        );
        if fs::canonicalize(local)? == dest {
            return Ok(dest);
        }
        if dest.exists() {
            bail!("Download already exists: {}", dest.display());
        }
        fs::copy(local, &dest)?;
        return Ok(dest);
    }
    if !track.source.starts_with("https://") {
        bail!("Only local files and HTTPS media can be downloaded");
    }
    let program = std::env::var_os("BITCHORD_YTDLP").unwrap_or_else(|| "yt-dlp".into());
    let mut command = Command::new(program);
    let cookies = crate::auth::cookie_file()?;
    if let Some(ref file) = cookies {
        command.arg("--cookies").arg(file.path());
    }
    command
        .args([
            "--no-warnings",
            "--no-progress",
            "--no-playlist",
            "--no-overwrites",
            "--restrict-filenames",
            "--socket-timeout",
            "20",
            "--retries",
            "2",
            "-x",
            "--audio-format",
            "mp3",
            "--audio-quality",
            "0",
            "--embed-metadata",
            "--print",
            "after_move:filepath",
            "-o",
        ])
        .arg(directory.join("%(title).150B [%(id)s].%(ext)s"))
        .arg("--")
        .arg(&track.source);
    let out = run(&mut command, Duration::from_secs(600))?;
    if !out.status.success() {
        bail!(
            "Download failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let path = String::from_utf8_lossy(&out.stdout)
        .lines()
        .last()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .ok_or_else(|| anyhow!("Download completed but no audio file was reported"))?;
    if !fs::canonicalize(&path)?.starts_with(&directory) {
        bail!("Download reported a path outside your download folder");
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_lyrics_and_rejects_invalid_timestamps() {
        assert_eq!(
            parse_lyrics("[00:01.20][00:02.30]hello\nworld"),
            vec![
                LyricLine {
                    time: 1.2,
                    text: "hello".into(),
                    words: vec![],
                },
                LyricLine {
                    time: 2.3,
                    text: "hello".into(),
                    words: vec![],
                }
            ]
        );
        assert_eq!(parse_lyrics("hello\nworld").len(), 2);
        assert_eq!(parse_lyrics("[00:NaN]bad")[0].time, 0.0);
        assert_eq!(duration("1:02:03"), 3723.0);
    }
    #[test]
    fn parses_youtube_music_renderer() {
        let fixture = json!({"musicResponsiveListItemRenderer": {"playlistItemData":{"videoId":"abcdefghijk"},"flexColumns":[{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Track"}]}}},{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Artist • Album"}]}}}],"fixedColumns":[{"musicResponsiveListItemFixedColumnRenderer":{"text":{"runs":[{"text":"3:45"}]}}}]}});
        let mut tracks = vec![];
        parse_renderers(&fixture, &mut tracks);
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].artist, "Artist");
        assert_eq!(tracks[0].duration, 225.0);
        let mut fixture = fixture;
        let row = &mut fixture["musicResponsiveListItemRenderer"];
        row.as_object_mut().unwrap().remove("fixedColumns");
        row["flexColumns"][1]["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"][0]
            ["text"] = json!("Artist • Album • 3:45");
        let mut tracks = vec![];
        parse_renderers(&fixture, &mut tracks);
        assert_eq!(tracks[0].duration, 225.0);
        assert_eq!(tracks[0].album, "Album");
    }
    #[test]
    fn parses_home_cards_and_collection_rows_without_menu_ids() {
        let album = json!({"musicTwoRowItemRenderer":{"title":{"runs":[{"text":"Album"}]},"subtitle":{"runs":[{"text":"Artist"}]},"thumbnailRenderer":{"musicThumbnailRenderer":{"thumbnail":{"thumbnails":[{"url":"cover"}]}}},"navigationEndpoint":{"browseEndpoint":{"browseId":"MPREb_album","params":"p"}},"menu":{"items":[{"navigationEndpoint":{"watchEndpoint":{"videoId":"not-primary"}}}]}}});
        let song = json!({"musicResponsiveListItemRenderer":{"playlistItemData":{"videoId":"abcdefghijk"},"flexColumns":[{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Song"}]}}},{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Artist • Album • 3:10"}]}}}]}});
        let feed = parse_home(
            &json!({"contents":{"musicCarouselShelfRenderer":{"header":{"musicCarouselShelfBasicHeaderRenderer":{"title":{"simpleText":"For you"}}},"contents":[album,song]}},"continuationContents":{"nextContinuationData":{"continuation":"next-token"}}}),
        );
        assert_eq!(
            feed.shelves[0].items[0].browse_id.as_deref(),
            Some("MPREb_album")
        );
        assert_eq!(feed.shelves[0].items[0].thumbnail, "cover");
        assert_eq!(feed.shelves[0].items[1].track().unwrap().id, "abcdefghijk");
        assert_eq!(feed.continuation.as_deref(), Some("next-token"));
        let c = parse_collection(
            &json!({"header":{"musicDetailHeaderRenderer":{"title":{"simpleText":"Album"}}},"contents":{"musicShelfRenderer":{"contents":[song]}},"continuation":"more"}),
        );
        assert_eq!(c.title, "Album");
        assert_eq!(c.tracks.len(), 1);
        assert_eq!(c.continuation.as_deref(), Some("more"));
    }
    #[test]
    fn ignores_shelf_tokens_and_reads_nested_collection_and_radio_rows() {
        let response = json!({
            "contents":{"musicCarouselShelfRenderer":{"continuations":[{"nextContinuationData":{"continuation":"shelf-token"}}]}},
            "continuationContents":{"sectionListContinuation":{"continuations":[{"nextContinuationData":{"continuation":"page-token"}}]}},
            "header":{"musicEditablePlaylistDetailHeaderRenderer":{"header":{"musicResponsiveHeaderRenderer":{"title":{"simpleText":"My mix"},"thumbnail":{"musicThumbnailRenderer":{"thumbnail":{"thumbnails":[{"url":"art"}]}}}}}}},
            "playlistPanelRenderer":{"contents":[{"playlistPanelVideoRenderer":{"videoId":"abcdefghijk","title":{"simpleText":"Song"},"longBylineText":{"runs":[{"text":"Artist"}]},"lengthText":{"simpleText":"2:03"}}}]}
        });
        assert_eq!(continuation(&response).as_deref(), Some("page-token"));
        let collection = parse_collection(&response);
        assert_eq!(collection.title, "My mix");
        assert_eq!(collection.thumbnail, "art");
        let mut tracks = vec![];
        collect_panel(&response, &mut tracks);
        assert_eq!(tracks[0].id, "abcdefghijk");
        assert_eq!(tracks[0].duration, 123.0);
    }
    #[test]
    fn local_scan_skips_unsupported_and_symlink_loops() {
        let dir = std::env::temp_dir().join(format!("bitchord-scan-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("track.WAV"), b"test").unwrap();
        fs::write(dir.join("skip.txt"), b"test").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&dir, dir.join("loop")).unwrap();
        let mut paths = vec![];
        files(&dir, &mut paths, 0).unwrap();
        assert_eq!(paths.len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn local_download_preserves_original_and_refuses_overwrite() {
        let dir = std::env::temp_dir().join(format!("bitchord-copy-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let original = dir.join("track.wav");
        fs::write(&original, b"audio bytes").unwrap();
        let track = Track {
            source: original.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let copied = download(&track, &dir.join("downloads")).unwrap();
        assert_eq!(fs::read(copied).unwrap(), b"audio bytes");
        assert!(download(&track, &dir.join("downloads")).is_err());
        assert_eq!(fs::read(&original).unwrap(), b"audio bytes");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "requires internet access to LRCLIB"]
    fn live_lyrics() {
        let lines = lyrics(&Track {
            title: "Get Lucky".into(),
            artist: "Daft Punk".into(),
            duration: 369.0,
            ..Default::default()
        })
        .unwrap();
        assert!(!lines.is_empty());
        assert!(lines.iter().any(|line| line.time > 0.0));
        println!("LRCLIB: {} timed lyric lines", lines.len());
    }
    #[test]
    #[ignore = "requires internet access to YouTube Music and LRCLIB"]
    fn live_music_search_and_browse() {
        let tracks = search("Daft Punk Get Lucky").unwrap();
        assert!(!tracks.is_empty());
        assert!(tracks.iter().any(|track| track.duration > 0.0));
        assert!(tracks[0].source.starts_with("https://music.youtube.com/"));
        println!(
            "search: {} tracks; first: {} by {}",
            tracks.len(),
            tracks[0].title,
            tracks[0].artist
        );
        let tracks = browse().unwrap();
        assert!(!tracks.is_empty());
        println!("explore: {} tracks", tracks.len());
    }
    #[test]
    #[ignore = "requires internet access to YouTube Music; guest home may be empty"]
    fn live_guest_home_supplements_and_collection() {
        let feed = home().unwrap();
        println!(
            "guest home: {} shelves / {} items",
            feed.shelves.len(),
            feed.shelves.iter().map(|s| s.items.len()).sum::<usize>()
        );
        assert!(feed
            .shelves
            .iter()
            .all(|s| !s.items.iter().any(|i| i.video_id.as_deref() == Some(""))));
        let releases = home_supplement("FEmusic_new_releases").unwrap();
        assert!(!releases.is_empty());
        println!(
            "new releases: {} shelves / {} items",
            releases.len(),
            releases.iter().map(|s| s.items.len()).sum::<usize>()
        );
        let album = releases
            .iter()
            .flat_map(|s| &s.items)
            .filter(|i| i.subtitle.starts_with("Album"))
            .find_map(|i| i.browse_id.as_deref())
            .unwrap();
        let collection = collection(album, None).unwrap();
        assert!(
            !collection.title.is_empty(),
            "Collection must expose its title"
        );
        assert!(
            collection.tracks.len() > 1,
            "An album must expose its track list"
        );
        println!("collection: {} tracks", collection.tracks.len());
        let seed = releases
            .iter()
            .flat_map(|s| &s.items)
            .find_map(|i| i.video_id.as_deref())
            .unwrap();
        let radio = radio(seed).unwrap();
        assert!(!radio.is_empty());
        println!("radio: {} tracks", radio.len());
        assert!(
            radio.len() > 1,
            "Radio must include recommendations, not just its seed"
        );
    }
}
