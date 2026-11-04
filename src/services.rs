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
pub mod translate;

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
    /// The playlist id (without the `VL` browse prefix) when this collection is a playlist.
    pub playlist_id: Option<String>,
    /// The account owns it: YouTube sends an editable header only to the owner.
    pub editable: bool,
    /// Video id to the playlist entry id (`playlistSetVideoId`) that removal needs.
    pub set_ids: std::collections::HashMap<String, String>,
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

/// The user agent for YouTube's media servers, as a desktop browser sends it.
pub const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const VISIONOS_VERSION: &str = "1.02";
const VISIONOS_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

/// The anonymous visit the player requests belong to. YouTube answers a player request that
/// carries no visitor with a bot check, so one is fetched once and reused.
static VISITOR: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn visitor_data(client: &reqwest::blocking::Client) -> Result<String> {
    if let Some(v) = VISITOR
        .lock()
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| v.clone())
    {
        return Ok(v);
    }
    let body = json!({"context": {"client": {
        "clientName": "WEB_REMIX", "clientVersion": "1.20260601.01.00", "hl": "en", "gl": "US"
    }}});
    let response: Value = client
        .post("https://music.youtube.com/youtubei/v1/visitor_id?prettyPrint=false")
        .header("Origin", "https://music.youtube.com")
        .header("User-Agent", BROWSER_USER_AGENT)
        .json(&body)
        .send()?
        .error_for_status()?
        .json()?;
    let visitor = response
        .pointer("/responseContext/visitorData")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow!("YouTube returned no visitor ID"))?
        .to_owned();
    if let Ok(mut slot) = VISITOR.lock() {
        *slot = visitor.clone();
    }
    Ok(visitor)
}

/// A direct URL of `video_id`'s best audio and its length in seconds, the way Meiro's
/// `AudioStream` gets it (https://github.com/elianiva/meiro, GPL-3.0, `youtube/playback.go`):
/// one anonymous InnerTube `player` request as the visionOS YouTube app,
/// the same client yt-dlp's fast path uses, without starting yt-dlp (about 1.5 s of process
/// start-up on Windows, plus its extraction). The URL is probed before it is handed out, because
/// the media server now and then refuses one the player just gave.
pub fn audio_stream(video_id: &str) -> Result<(String, f64)> {
    if !valid_video_id(video_id) {
        bail!("Not a YouTube video ID: {video_id}");
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;
    let visitor = visitor_data(&client)?;
    let body = json!({
        "videoId": video_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
        "context": {"client": {
            "clientName": "VISIONOS",
            "clientVersion": VISIONOS_VERSION,
            "hl": "en",
            "gl": "US",
            "visitorData": visitor,
            "deviceMake": "Apple",
            "deviceModel": "RealityDevice17,1",
            "userAgent": VISIONOS_USER_AGENT,
            "osName": "visionOS",
            "osVersion": "26.5.23O471",
        }},
    });
    let response: Value = client
        .post("https://music.youtube.com/youtubei/v1/player?prettyPrint=false")
        .header("User-Agent", VISIONOS_USER_AGENT)
        .header("Origin", "https://music.youtube.com")
        .header("Referer", "https://music.youtube.com/")
        .header("X-YouTube-Client-Name", "101")
        .header("X-YouTube-Client-Version", VISIONOS_VERSION)
        .header("X-Goog-Visitor-Id", visitor)
        .json(&body)
        .send()?
        .error_for_status()?
        .json()?;
    let (url, seconds) = parse_audio_stream(&response)?;
    let probe = client
        .get(&url)
        .header("Range", "bytes=0-0")
        .header("User-Agent", BROWSER_USER_AGENT)
        .send()?;
    let status = probe.status();
    if status != reqwest::StatusCode::PARTIAL_CONTENT && status != reqwest::StatusCode::OK {
        bail!("The audio stream answers HTTP {status}");
    }
    Ok((url, seconds))
}

/// The audio-only format of the highest bitrate that has a direct URL, as yt-dlp's `bestaudio`
/// picks it: in the track's own language, and not the dynamic-range-compressed copy YouTube
/// lists beside it (upstream Meiro `parseAudioStream`).
fn parse_audio_stream(response: &Value) -> Result<(String, f64)> {
    let status = response
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str);
    if status != Some("OK") {
        let reason = response
            .pointer("/playabilityStatus/reason")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        bail!(
            "YouTube will not play this here ({}): {reason}",
            status.unwrap_or("no status")
        );
    }
    let mut best: Option<(&str, i64, f64)> = None;
    for format in response
        .pointer("/streamingData/adaptiveFormats")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let audio = format["mimeType"]
            .as_str()
            .is_some_and(|m| m.starts_with("audio/"));
        let url = format["url"].as_str().filter(|u| u.starts_with("https://"));
        let dub = format["audioTrack"].is_object()
            && !format["audioTrack"]["audioIsDefault"]
                .as_bool()
                .unwrap_or(false);
        let drc = format["isDrc"].as_bool().unwrap_or(false);
        let bitrate = format["bitrate"].as_i64().unwrap_or(0);
        let (Some(url), true, false, false) = (url, audio, dub, drc) else {
            continue;
        };
        if best.is_none_or(|(_, b, _)| bitrate > b) {
            let ms = format["approxDurationMs"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            best = Some((url, bitrate, ms / 1000.0));
        }
    }
    best.map(|(url, _, seconds)| (url.to_owned(), seconds))
        .ok_or_else(|| anyhow!("The player lists no audio with a direct URL"))
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
    let mut collection = parse_collection(&v);
    collection.playlist_id = browse_id.strip_prefix("VL").map(str::to_owned);
    Ok(collection)
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
        playlist_id: None,
        editable: find_renderer(v, "musicEditablePlaylistDetailHeaderRenderer").is_some(),
        set_ids: set_ids(v),
    }
}
/// `playlistItemData` of every playlist row: video id to its entry id. Only rows actually in a
/// playlist carry one (upstream `InnertubeParser`).
fn set_ids(v: &Value) -> std::collections::HashMap<String, String> {
    fn walk(v: &Value, out: &mut std::collections::HashMap<String, String>) {
        match v {
            Value::Object(map) => {
                if let Some(data) = map.get("playlistItemData") {
                    if let (Some(video), Some(set)) = (
                        data["videoId"].as_str(),
                        data["playlistSetVideoId"].as_str(),
                    ) {
                        out.entry(video.to_owned())
                            .or_insert_with(|| set.to_owned());
                    }
                }
                map.values().for_each(|x| walk(x, out));
            }
            Value::Array(items) => items.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = std::collections::HashMap::new();
    walk(v, &mut out);
    out
}

/// Playlists in the signed-in account's library (`FEmusic_liked_playlists`): its own and those
/// it saved. The grid's "New playlist" tile has no browse id and is left out.
pub fn library_playlists() -> Result<Vec<ShelfItem>> {
    let v = account_call("browse", json!({"browseId": "FEmusic_liked_playlists"}))?;
    let mut items = vec![];
    collect_cards(&v, &mut items);
    items.retain(|i| i.browse_id.as_deref().is_some_and(|b| b.starts_with("VL")));
    let mut seen = HashSet::new();
    items.retain(|i| seen.insert(i.browse_id.clone()));
    Ok(items)
}

/// Privacy choices for a new playlist: (API value, label).
pub const PLAYLIST_PRIVACY: [(&str, &str); 3] = [
    ("PRIVATE", "Private"),
    ("UNLISTED", "Unlisted"),
    ("PUBLIC", "Public"),
];

/// Create a playlist, optionally seeded with `video_ids` in the same request; returns its id.
pub fn create_playlist(title: &str, privacy: &str, video_ids: &[String]) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        bail!("A playlist needs a name");
    }
    if !PLAYLIST_PRIVACY.iter().any(|(p, _)| *p == privacy) {
        bail!("Unknown playlist privacy {privacy}");
    }
    let mut body = json!({"title": title, "description": "", "privacyStatus": privacy});
    if !video_ids.is_empty() {
        body["videoIds"] = json!(video_ids);
    }
    let v = account_call("playlist/create", body)?;
    // Normally top-level; sometimes only inside the navigation command (upstream).
    v["playlistId"]
        .as_str()
        .or_else(|| find_string(&v, "playlistId"))
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("Playlist created but YouTube returned no id"))
}

pub fn delete_playlist(playlist_id: &str) -> Result<()> {
    account_call(
        "playlist/delete",
        json!({"playlistId": playlist_id.trim_start_matches("VL")}),
    )
    .map(|_| ())
}

pub fn rename_playlist(playlist_id: &str, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("A playlist needs a name");
    }
    edit_playlist(
        playlist_id,
        json!([{"action": "ACTION_SET_PLAYLIST_NAME", "playlistName": name}]),
    )
    .map(|_| ())
}

/// Add a track; returns the new entry's set-video-id when YouTube reports it, so the row can be
/// removed again without reopening the playlist.
pub fn add_to_playlist(playlist_id: &str, video_id: &str) -> Result<Option<String>> {
    let v = edit_playlist(
        playlist_id,
        json!([{"action": "ACTION_ADD_VIDEO", "addedVideoId": video_id}]),
    )?;
    Ok(v["playlistEditResults"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.get("playlistEditVideoAddedResultData"))
        .find(|d| d["videoId"].as_str() == Some(video_id))
        .and_then(|d| d["setVideoId"].as_str())
        .map(str::to_owned))
}

pub fn remove_from_playlist(playlist_id: &str, video_id: &str, set_id: &str) -> Result<()> {
    edit_playlist(
        playlist_id,
        json!([{"action": "ACTION_REMOVE_VIDEO", "setVideoId": set_id, "removedVideoId": video_id}]),
    )
    .map(|_| ())
}

/// `browse/edit_playlist` answers 200 with a non-success `status` when it refuses (usually a
/// playlist the account saved but does not own), so the body is checked too.
fn edit_playlist(playlist_id: &str, actions: Value) -> Result<Value> {
    let v = account_call(
        "browse/edit_playlist",
        json!({"playlistId": playlist_id.trim_start_matches("VL"), "actions": actions}),
    )?;
    edit_status(&v)?;
    Ok(v)
}

fn edit_status(v: &Value) -> Result<()> {
    match v["status"].as_str() {
        None | Some("STATUS_SUCCEEDED") => Ok(()),
        Some(status) => bail!("YouTube Music refused the playlist edit ({status}); only playlists you own can be edited"),
    }
}

/// A request that only makes sense signed in: never retried anonymously.
fn account_call(endpoint: &str, body: Value) -> Result<Value> {
    let session = crate::auth::snapshot()
        .ok_or_else(|| anyhow!("Sign in to YouTube Music to use your playlists"))?;
    let (status, response) = send_innertube(endpoint, &body, Some(&session))?;
    if status == reqwest::StatusCode::UNAUTHORIZED {
        SIGN_IN_REJECTED.store(true, std::sync::atomic::Ordering::SeqCst);
        bail!("Your YouTube sign-in has expired; sign in again to edit playlists");
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

/// Typed-query suggestions for the search palette (upstream `searchSuggestions`). Deliberately
/// unauthenticated, so each debounced keystroke stays out of the account's search history.
pub fn search_suggestions(input: &str) -> Result<Vec<String>> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(vec![]);
    }
    if input.len() > 500 {
        bail!("Search text is too long");
    }
    let (status, response) = send_innertube(
        "music/get_search_suggestions",
        &json!({"input": input}),
        None,
    )?;
    if !status.is_success() {
        bail!("Suggestions unavailable ({status})");
    }
    Ok(parse_suggestions(&response))
}

/// The query strings of `searchSuggestionRenderer` rows, in order, without repeats. Entity rows
/// (songs, artists) are ignored: the palette fills the search with a query, as upstream does.
fn parse_suggestions(response: &Value) -> Vec<String> {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                if let Some(row) = map.get("searchSuggestionRenderer") {
                    let query = row["navigationEndpoint"]["searchEndpoint"]["query"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            row["suggestion"]["runs"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|r| r["text"].as_str())
                                .collect()
                        });
                    let query = query.trim().to_owned();
                    if !query.is_empty() && !out.contains(&query) {
                        out.push(query);
                    }
                }
                map.values().for_each(|x| walk(x, out));
            }
            Value::Array(items) => items.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = vec![];
    walk(response, &mut out);
    out.truncate(8);
    out
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
    run_streaming(command, timeout, None, &mut |_| {})
}

/// As [`run`], handing each stdout line to `on_line` as it arrives; setting `cancel` kills the
/// process.
pub(crate) fn run_streaming(
    command: &mut Command,
    timeout: Duration,
    cancel: Option<&std::sync::atomic::AtomicBool>,
    on_line: &mut dyn FnMut(&str),
) -> Result<std::process::Output> {
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
    // Lines from either stream are handed to `on_line` (yt-dlp reports progress on stderr when
    // `--print` makes it quiet).
    let (lines_tx, lines) = std::sync::mpsc::channel::<String>();
    let read = |pipe: Box<dyn Read + Send>, tx: std::sync::mpsc::Sender<String>| {
        thread::spawn(move || {
            use std::io::BufRead;
            let mut bytes = vec![];
            let mut reader = std::io::BufReader::new(pipe.take(8 * 1024 * 1024));
            let mut line = String::new();
            while reader.read_line(&mut line)? > 0 {
                bytes.extend_from_slice(line.as_bytes());
                let _ = tx.send(line.trim_end().to_owned());
                line.clear();
            }
            Ok::<_, std::io::Error>(bytes)
        })
    };
    let out = read(Box::new(stdout), lines_tx.clone());
    let err = read(Box::new(stderr), lines_tx);
    let start = Instant::now();
    let status = loop {
        for line in lines.try_iter() {
            on_line(&line);
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::SeqCst)) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Cancelled");
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Audio tool timed out after {} seconds", timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(25));
    };
    let stdout = out
        .join()
        .map_err(|_| anyhow!("Audio output reader failed"))??;
    for line in lines.try_iter() {
        on_line(&line);
    }
    Ok(std::process::Output {
        status,
        stdout,
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

/// Download formats offered in Settings: (id, label). `best` keeps YouTube's own audio
/// (usually Opus) without re-encoding, which is what upstream's quality-first default amounts to
/// with a YouTube source.
pub const DOWNLOAD_FORMATS: [(&str, &str); 3] = [
    ("best", "Original (best quality, no re-encode)"),
    ("mp3", "MP3 (320 kbps, re-encoded)"),
    ("m4a", "M4A / AAC (re-encoded)"),
];

#[cfg(test)]
pub fn download(track: &Track, directory: &Path) -> Result<PathBuf> {
    download_with(
        track,
        directory,
        "mp3",
        &std::sync::atomic::AtomicBool::new(false),
        &mut |_| {},
    )
}

/// Download `track` into `directory` as `format` (see [`DOWNLOAD_FORMATS`]), reporting progress
/// as a 0..=1 fraction. Setting `cancel` kills the transfer and returns an error.
pub fn download_with(
    track: &Track,
    directory: &Path,
    format: &str,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<PathBuf> {
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
    let format = DOWNLOAD_FORMATS
        .iter()
        .find(|(id, _)| *id == format)
        .map_or("best", |(id, _)| id);
    // As for playback: the cookie-less `visionos` client yields the real audio stream quickly;
    // signed-in extraction often only offers a 360p video (format 18), whose audio is poorer.
    // Restricted tracks fail the first attempt and are retried with the account.
    let fast = download_attempt(track, &directory, format, false, cancel, progress);
    match fast {
        Ok(path) => Ok(path),
        Err(_) if cancel.load(std::sync::atomic::Ordering::SeqCst) => bail!("Download cancelled"),
        Err(_) => download_attempt(track, &directory, format, true, cancel, progress),
    }
}

fn download_attempt(
    track: &Track,
    directory: &Path,
    format: &str,
    signed_in: bool,
    cancel: &std::sync::atomic::AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<PathBuf> {
    let program = std::env::var_os("BITCHORD_YTDLP").unwrap_or_else(|| "yt-dlp".into());
    let mut command = Command::new(program);
    let cookies = if signed_in {
        crate::auth::cookie_file()?
    } else {
        command.args(["--extractor-args", "youtube:player_client=visionos"]);
        None
    };
    if let Some(ref file) = cookies {
        command.arg("--cookies").arg(file.path());
    }
    command.args([
        "--no-warnings",
        // `--print` implies `--quiet`, which hides progress; `--progress` brings it back (on stderr).
        "--progress",
        "--newline",
        "--progress-template",
        "download:YTDUGEM-PROGRESS %(progress.downloaded_bytes)s %(progress.total_bytes,progress.total_bytes_estimate)s",
        "--no-playlist",
        "--no-overwrites",
        "--restrict-filenames",
        "--socket-timeout",
        "20",
        "--retries",
        "2",
        "-f",
        "bestaudio/best",
        "-x",
        "--audio-format",
        format,
    ]);
    if format == "mp3" {
        command.args(["--audio-quality", "0"]);
    }
    command
        .args(["--embed-metadata", "--print", "after_move:filepath", "-o"])
        .arg(directory.join("%(title).150B [%(id)s].%(ext)s"))
        .arg("--")
        .arg(&track.source);
    let out = run_streaming(
        &mut command,
        Duration::from_secs(900),
        Some(cancel),
        &mut |line| {
            if let Some(fraction) = parse_progress(line) {
                progress(fraction);
            }
        },
    )?;
    if !out.status.success() {
        bail!(
            "Download failed: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .filter(|l| !l.starts_with("YTDUGEM-PROGRESS") && !l.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    let path = String::from_utf8_lossy(&out.stdout)
        .lines()
        .rfind(|l| !l.starts_with("YTDUGEM-PROGRESS"))
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .ok_or_else(|| anyhow!("Download completed but no audio file was reported"))?;
    if !fs::canonicalize(&path)?.starts_with(directory) {
        bail!("Download reported a path outside your download folder");
    }
    progress(1.0);
    Ok(path)
}

/// `YTDUGEM-PROGRESS <downloaded> <total>` from the progress template; "NA" when unknown.
fn parse_progress(line: &str) -> Option<f32> {
    let mut parts = line.strip_prefix("YTDUGEM-PROGRESS ")?.split_whitespace();
    let done: f64 = parts.next()?.parse().ok()?;
    let total: f64 = parts.next()?.parse().ok()?;
    (total > 0.0).then(|| (done / total).clamp(0.0, 1.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires yt-dlp, FFmpeg and internet access to YouTube"]
    fn downloads_with_progress_and_cancels() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = std::env::temp_dir().join(format!("ytdugem-dl-{}", std::process::id()));
        let track = Track {
            id: "jNQXAC9IVRw".into(),
            source: "https://music.youtube.com/watch?v=jNQXAC9IVRw".into(),
            ..Default::default()
        };
        let mut seen = vec![];
        let started = Instant::now();
        let path = download_with(&track, &dir, "best", &AtomicBool::new(false), &mut |f| {
            seen.push(f)
        })
        .unwrap();
        println!(
            "downloaded {} in {:.1}s, {} progress updates, last {:?}",
            path.display(),
            started.elapsed().as_secs_f32(),
            seen.len(),
            seen.last()
        );
        assert!(path.is_file() && fs::metadata(&path).unwrap().len() > 10_000);
        assert_eq!(seen.last(), Some(&1.0));
        assert!(seen.windows(2).all(|w| w[0] <= w[1] || w[1] == 0.0));

        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let other = Track {
            id: "dQw4w9WgXcQ".into(),
            source: "https://music.youtube.com/watch?v=dQw4w9WgXcQ".into(),
            ..Default::default()
        };
        let result = download_with(&other, &dir, "mp3", &cancel, &mut |_| {
            cancel.store(true, Ordering::SeqCst)
        });
        println!(
            "cancelled after {:.1}s: {:?}",
            started.elapsed().as_secs_f32(),
            result.as_ref().err()
        );
        assert!(result.is_err());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn owned_playlists_expose_entry_ids_and_refusals_are_errors() {
        let owned = json!({
            "header":{"musicEditablePlaylistDetailHeaderRenderer":{"header":{"musicResponsiveHeaderRenderer":{"title":{"simpleText":"Mine"}}}}},
            "contents":[{"musicResponsiveListItemRenderer":{
                "playlistItemData":{"videoId":"abcdefghijk","playlistSetVideoId":"SET1"},
                "flexColumns":[{"musicResponsiveListItemFlexColumnRenderer":{"text":{"runs":[{"text":"Song"}]}}}],
                "overlay":{"musicItemThumbnailOverlayRenderer":{"content":{"musicPlayButtonRenderer":{"playNavigationEndpoint":{"watchEndpoint":{"videoId":"abcdefghijk"}}}}}}
            }}]
        });
        let c = parse_collection(&owned);
        assert!(c.editable);
        assert_eq!(
            c.set_ids.get("abcdefghijk").map(String::as_str),
            Some("SET1")
        );
        let saved =
            json!({"header":{"musicResponsiveHeaderRenderer":{"title":{"simpleText":"Theirs"}}}});
        assert!(!parse_collection(&saved).editable);

        assert!(edit_status(&json!({"status":"STATUS_SUCCEEDED"})).is_ok());
        assert!(edit_status(&json!({})).is_ok());
        assert!(edit_status(&json!({"status":"STATUS_FAILED"})).is_err());
    }

    #[test]
    fn parses_typeahead_queries_and_skips_entity_rows() {
        let response = json!({
            "contents": [{"searchSuggestionsSectionRenderer": {"contents": [
                {"searchSuggestionRenderer": {
                    "navigationEndpoint": {"searchEndpoint": {"query": "aku milikmu"}},
                    "suggestion": {"runs": [{"text": "aku mili"}, {"text": "kmu"}]}}},
                {"searchSuggestionRenderer": {
                    "suggestion": {"runs": [{"text": "aku milikmu dewa 19"}]}}},
                {"searchSuggestionRenderer": {
                    "navigationEndpoint": {"searchEndpoint": {"query": "aku milikmu"}}}},
                {"musicResponsiveListItemRenderer": {"title": "Song"}}
            ]}}]
        });
        assert_eq!(
            parse_suggestions(&response),
            ["aku milikmu", "aku milikmu dewa 19"]
        );
        assert!(parse_suggestions(&json!({})).is_empty());
    }

    #[test]
    fn picks_the_best_plain_audio_format_like_meiro() {
        let response = json!({
            "playabilityStatus": {"status": "OK"},
            "streamingData": {"adaptiveFormats": [
                {"mimeType": "video/mp4", "bitrate": 900000, "url": "https://v/video"},
                {"mimeType": "audio/webm; codecs=\"opus\"", "bitrate": 130000, "url": "https://v/opus", "approxDurationMs": "212000"},
                {"mimeType": "audio/webm; codecs=\"opus\"", "bitrate": 140000, "url": "https://v/drc", "isDrc": true},
                {"mimeType": "audio/mp4", "bitrate": 150000, "url": "https://v/dub", "audioTrack": {"audioIsDefault": false}},
                {"mimeType": "audio/mp4", "bitrate": 128000, "url": "https://v/aac", "approxDurationMs": "212000"},
                {"mimeType": "audio/webm", "bitrate": 999999, "signatureCipher": "s=..."}
            ]}
        });
        assert_eq!(
            parse_audio_stream(&response).unwrap(),
            ("https://v/opus".to_owned(), 212.0)
        );
        let refused =
            json!({"playabilityStatus": {"status": "LOGIN_REQUIRED", "reason": "Sign in"}});
        assert!(parse_audio_stream(&refused)
            .unwrap_err()
            .to_string()
            .contains("Sign in"));
    }

    #[test]
    #[ignore = "requires internet access to YouTube Music; no account"]
    fn live_direct_audio_stream() {
        for _ in 0..2 {
            let started = Instant::now();
            let (url, seconds) = audio_stream("dQw4w9WgXcQ").unwrap();
            println!(
                "direct resolve {:.2}s, {seconds:.0}s long, {}",
                started.elapsed().as_secs_f32(),
                &url[..60]
            );
            assert!(seconds > 200.0);
        }
    }

    #[test]
    fn parses_download_progress_template() {
        assert_eq!(parse_progress("YTDUGEM-PROGRESS 512 1024"), Some(0.5));
        assert_eq!(parse_progress("YTDUGEM-PROGRESS 2048 1024.0"), Some(1.0));
        assert_eq!(parse_progress("YTDUGEM-PROGRESS 100 NA"), None);
        assert_eq!(parse_progress("YTDUGEM-PROGRESS 0 0"), None);
        assert_eq!(parse_progress(r"C:\Music\song.opus"), None);
    }
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
