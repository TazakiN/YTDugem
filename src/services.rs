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
    /// Whether the page's save-to-library toggle says it is saved; None without such a toggle.
    pub saved: Option<bool>,
    /// Header description text, empty when the page has none.
    pub description: String,
    /// `PUBLIC`/`UNLISTED`/`PRIVATE`, from the owner's editable header only.
    pub privacy: Option<String>,
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

/// One server-defined group of Explore's mood and genre buttons (upstream `MoodGenreSection`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoodGenreSection {
    pub title: String,
    pub items: Vec<MoodGenre>,
}
/// A mood or genre button and the exact browse request it stands for (upstream `MoodGenre`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MoodGenre {
    pub title: String,
    pub browse_id: String,
    pub params: Option<String>,
    /// YouTube Music's stripe colour for the button as ARGB (`solid.leftStripeColor`).
    pub color: Option<u32>,
    /// First cover from the category's playlist shelves, resolved in the background.
    pub thumbnail: Option<String>,
}
impl MoodGenre {
    /// Identifies the category: the same browse id with other params is another page.
    pub fn key(&self) -> String {
        format!(
            "{}|{}",
            self.browse_id,
            self.params.as_deref().unwrap_or("")
        )
    }
}

/// Shelves of one browse page that actually hold cards.
fn browse_shelves(browse_id: &str, params: Option<&str>) -> Result<Vec<HomeShelf>> {
    let mut body = json!({ "browseId": browse_id });
    if let Some(params) = params {
        body["params"] = json!(params);
    }
    let v = innertube_value("browse", body)?;
    Ok(filled_shelves(parse_home(&v).shelves))
}
fn filled_shelves(mut shelves: Vec<HomeShelf>) -> Vec<HomeShelf> {
    shelves.retain(|s| !s.items.is_empty());
    shelves
}
/// The Explore page's own shelves (new albums, trending, new videos, ...).
pub fn explore() -> Result<Vec<HomeShelf>> {
    browse_shelves("FEmusic_explore", None)
}
pub fn new_releases() -> Result<Vec<HomeShelf>> {
    browse_shelves("FEmusic_new_releases", None)
}
pub fn charts() -> Result<Vec<HomeShelf>> {
    browse_shelves("FEmusic_charts", None)
}

/// Upstream `YtMusicRepository.moodAndGenres`: the server-defined categories Explore shows.
pub fn moods_and_genres() -> Result<Vec<MoodGenreSection>> {
    let v = innertube_value("browse", json!({"browseId":"FEmusic_moods_and_genres"}))?;
    Ok(parse_mood_and_genres(&v))
}

/// Upstream `InnertubeParser.parseMoodAndGenres`: navigation-button grids, each button keeping
/// the browse params that select its exact category. A signed-in page leads with a "For you"
/// grid of shortcuts to buttons later grids repeat; it is dropped, recognised by repeating a
/// later section's button, with the English title as a backstop.
fn parse_mood_and_genres(v: &Value) -> Vec<MoodGenreSection> {
    let sections = v
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let parsed: Vec<MoodGenreSection> = sections
        .iter()
        .filter_map(|section| {
            let grid = section.get("gridRenderer")?;
            let title = text(&grid["header"]["gridHeaderRenderer"]["title"]);
            let items: Vec<MoodGenre> = grid["items"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .filter_map(mood_genre)
                .collect();
            (!title.trim().is_empty() && !items.is_empty())
                .then_some(MoodGenreSection { title, items })
        })
        .collect();
    parsed
        .iter()
        .enumerate()
        .filter(|(i, section)| {
            let later: HashSet<(&str, Option<&str>)> = parsed[i + 1..]
                .iter()
                .flat_map(|s| &s.items)
                .map(|m| (m.browse_id.as_str(), m.params.as_deref()))
                .collect();
            let shortcuts = section
                .items
                .iter()
                .any(|m| later.contains(&(m.browse_id.as_str(), m.params.as_deref())));
            !shortcuts && !section.title.eq_ignore_ascii_case("For you")
        })
        .map(|(_, s)| s.clone())
        .collect()
}
fn mood_genre(item: &Value) -> Option<MoodGenre> {
    let button = item.get("musicNavigationButtonRenderer")?;
    let endpoint = button
        .pointer("/clickCommand/browseEndpoint")
        .or_else(|| button.pointer("/navigationEndpoint/browseEndpoint"))?;
    let browse_id = endpoint["browseId"].as_str()?.to_owned();
    let title = text(&button["buttonText"]);
    if title.trim().is_empty() {
        return None;
    }
    // Upstream reads the colour with `toLongOrNull` and draws it as a 32-bit ARGB int.
    let stripe = &button["solid"]["leftStripeColor"];
    let color = stripe
        .as_i64()
        .or_else(|| stripe.as_str().and_then(|s| s.trim().parse().ok()))
        .filter(|c| (i64::from(i32::MIN)..=i64::from(u32::MAX)).contains(c))
        .map(|c| c as u32);
    Some(MoodGenre {
        title,
        browse_id,
        params: endpoint["params"].as_str().map(str::to_owned),
        color,
        thumbnail: None,
    })
}

/// Shelves already fetched per category: the artwork prefetch fetches the same page a click
/// opens, as upstream's `moodGenreShelfCache`.
static MOOD_SHELVES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, Vec<HomeShelf>>>,
> = std::sync::LazyLock::new(Default::default);

/// Upstream `YtMusicRepository.moodGenreShelves`: the playlist shelves behind one category.
pub fn mood_genre_shelves(browse_id: &str, params: Option<&str>) -> Result<Vec<HomeShelf>> {
    let key = format!("{browse_id}|{}", params.unwrap_or(""));
    if let Some(shelves) = MOOD_SHELVES.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Ok(shelves);
    }
    let shelves = browse_shelves(browse_id, params)?;
    if let Ok(mut cache) = MOOD_SHELVES.lock() {
        cache.entry(key).or_insert_with(|| shelves.clone());
    }
    Ok(shelves)
}
/// Drops cached category shelves, so a refresh or another account fetches them again.
pub fn forget_mood_genre_shelves() {
    if let Ok(mut cache) = MOOD_SHELVES.lock() {
        cache.clear();
    }
}
/// Upstream `moodGenreArtwork`: a category tile borrows the first cover its shelves hold.
pub fn mood_genre_artwork(browse_id: &str, params: Option<&str>) -> Result<Option<String>> {
    Ok(first_artwork(&mood_genre_shelves(browse_id, params)?))
}
fn first_artwork(shelves: &[HomeShelf]) -> Option<String> {
    shelves
        .iter()
        .flat_map(|s| &s.items)
        .map(|i| i.thumbnail.as_str())
        .find(|t| !t.is_empty())
        .map(str::to_owned)
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
        saved: library_saved(v),
        description: header_description(v, header),
        privacy: find_renderer(v, "musicPlaylistEditHeaderRenderer")
            .and_then(|h| h["privacy"].as_str())
            .map(str::to_owned),
    }
}

/// Port of upstream `parseLibraryState`: the responsive header's save toggle (bookmark icons)
/// and its `isToggled`; None without an enabled toggle or without the play button's playlist.
fn library_saved(v: &Value) -> Option<bool> {
    let buttons = find_renderer(v, "musicResponsiveHeaderRenderer")?
        .get("buttons")?
        .as_array()?;
    let save = buttons
        .iter()
        .filter_map(|b| b.get("toggleButtonRenderer"))
        .find(|b| {
            b.pointer("/defaultIcon/iconType").and_then(Value::as_str) == Some("BOOKMARK_BORDER")
                || b.pointer("/toggledIcon/iconType").and_then(Value::as_str) == Some("BOOKMARK")
        })?;
    let flag = |x: &Value| x.as_bool().or_else(|| x.as_str().map(|s| s == "true"));
    if flag(&save["isDisabled"]) == Some(true) {
        return None;
    }
    let play = buttons
        .iter()
        .find_map(|b| b.pointer("/musicPlayButtonRenderer/playNavigationEndpoint"))?;
    play.pointer("/watchPlaylistEndpoint/playlistId")
        .or_else(|| play.pointer("/watchEndpoint/playlistId"))?;
    Some(flag(&save["isToggled"]) == Some(true))
}

fn header_description(v: &Value, header: &Value) -> String {
    let candidates = [
        header.pointer("/description/musicDescriptionShelfRenderer/description"),
        header.get("description"),
        find_renderer(v, "musicPlaylistEditHeaderRenderer").and_then(|h| h.get("description")),
    ];
    candidates
        .into_iter()
        .flatten()
        .map(text)
        .find(|t| !t.trim().is_empty())
        .unwrap_or_default()
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
    let mut v = account_call("browse", json!({"browseId": "FEmusic_liked_playlists"}))?;
    let mut items: Vec<ShelfItem> = vec![];
    let mut page = 1;
    loop {
        merge_playlist_cards(&v, &mut items);
        let Some(token) = continuation(&v) else { break };
        if page >= MAX_PAGES {
            break;
        }
        page += 1;
        // A failed later page keeps what was already collected (upstream `itemsPaged`).
        match account_call("browse", json!({"continuation": token})) {
            Ok(next) => v = next,
            Err(_) => break,
        }
    }
    Ok(items)
}

/// Upstream `YtMusicRepository.MAX_PAGES`.
const MAX_PAGES: usize = 10;

/// Add one library page's playlist cards to `items`, skipping the "New playlist" tile (no `VL`
/// browse id) and ones already collected.
fn merge_playlist_cards(v: &Value, items: &mut Vec<ShelfItem>) {
    let mut page = vec![];
    collect_cards(v, &mut page);
    for item in page {
        if item
            .browse_id
            .as_deref()
            .is_some_and(|b| b.starts_with("VL"))
            && !items.iter().any(|i| i.browse_id == item.browse_id)
        {
            items.push(item);
        }
    }
}

/// Save a playlist or album to the library, or take it back out (upstream `ratePlaylist`).
pub fn set_playlist_saved(playlist_id: &str, saved: bool) -> Result<()> {
    let endpoint = if saved {
        "like/like"
    } else {
        "like/removelike"
    };
    account_call(
        endpoint,
        json!({"target": {"playlistId": playlist_id.trim_start_matches("VL")}}),
    )
    .map(|_| ())
}

/// Every track of a playlist: the first page and all continuations, merged.
pub fn playlist_all(browse_id: &str) -> Result<Collection> {
    let mut all = collection(browse_id, None)?;
    let mut seen: HashSet<String> = all.tracks.iter().map(|t| t.id.clone()).collect();
    let mut token = all.continuation.take();
    let mut page = 1;
    while let Some(t) = token {
        if page >= MAX_PAGES {
            bail!("playlist too long");
        }
        page += 1;
        let more = collection_more(&t)?;
        all.tracks.extend(
            more.tracks
                .into_iter()
                .filter(|t| seen.insert(t.id.clone())),
        );
        for (video, set) in more.set_ids {
            all.set_ids.entry(video).or_insert(set);
        }
        token = more.continuation;
    }
    Ok(all)
}

/// One step of a reorder: put entry `set_video_id` directly before `before`, or at the end.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PlaylistMove {
    pub set_video_id: String,
    pub before: Option<String>,
}

/// Port of upstream `playlistMoves`: the fewest moves turning `current` into `target` (both
/// entry ids). Empty when unchanged or when they are not the same set of unique entries.
fn playlist_moves(current: &[String], target: &[String]) -> Vec<PlaylistMove> {
    if current == target {
        return vec![];
    }
    let cur: HashSet<&String> = current.iter().collect();
    let tgt: HashSet<&String> = target.iter().collect();
    if current.len() != target.len() || cur != tgt || cur.len() != current.len() {
        return vec![];
    }
    let position: std::collections::HashMap<&String, usize> =
        current.iter().enumerate().map(|(i, id)| (id, i)).collect();
    let values: Vec<usize> = target.iter().map(|id| position[id]).collect();
    let stay: HashSet<usize> = longest_increasing_run(&values).into_iter().collect();
    let mut moves = vec![];
    for index in (0..target.len()).rev() {
        if stay.contains(&index) {
            continue;
        }
        moves.push(PlaylistMove {
            set_video_id: target[index].clone(),
            before: target.get(index + 1).cloned(),
        });
    }
    moves
}

/// Indices into `values` of one longest strictly increasing subsequence.
fn longest_increasing_run(values: &[usize]) -> Vec<usize> {
    if values.is_empty() {
        return vec![];
    }
    let mut tails: Vec<usize> = vec![];
    let mut previous = vec![usize::MAX; values.len()];
    for i in 0..values.len() {
        let low = tails.partition_point(|&t| values[t] < values[i]);
        if low > 0 {
            previous[i] = tails[low - 1];
        }
        if low == tails.len() {
            tails.push(i);
        } else {
            tails[low] = i;
        }
    }
    let mut run = vec![];
    let mut at = *tails.last().unwrap();
    while at != usize::MAX {
        run.push(at);
        at = previous[at];
    }
    run.reverse();
    run
}

fn move_actions(moves: &[PlaylistMove]) -> Value {
    Value::Array(
        moves
            .iter()
            .map(|m| {
                let mut a =
                    json!({"action": "ACTION_MOVE_VIDEO_BEFORE", "setVideoId": m.set_video_id});
                if let Some(before) = &m.before {
                    a["movedSetVideoIdSuccessor"] = json!(before);
                }
                a
            })
            .collect(),
    )
}

/// Rearrange a playlist from `current` to `target` (entry ids), 50 moves per request.
pub fn reorder_playlist(playlist_id: &str, current: &[String], target: &[String]) -> Result<()> {
    for batch in playlist_moves(current, target).chunks(50) {
        edit_playlist(playlist_id, move_actions(batch))?;
    }
    Ok(())
}

pub fn set_playlist_privacy(playlist_id: &str, privacy: &str) -> Result<()> {
    if !PLAYLIST_PRIVACY.iter().any(|(p, _)| *p == privacy) {
        bail!("Unknown playlist privacy {privacy}");
    }
    edit_playlist(
        playlist_id,
        json!([{"action": "ACTION_SET_PLAYLIST_PRIVACY", "playlistPrivacy": privacy}]),
    )
    .map(|_| ())
}

pub fn set_playlist_description(playlist_id: &str, description: &str) -> Result<()> {
    edit_playlist(
        playlist_id,
        json!([{"action": "ACTION_SET_PLAYLIST_DESCRIPTION", "playlistDescription": description}]),
    )
    .map(|_| ())
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LikeStatus {
    Like,
    Dislike,
    #[default]
    Indifferent,
}

/// Like, dislike or clear the rating of a song on the signed-in account.
#[allow(dead_code)] // used by the player sheet (in progress)
pub fn rate_song(video_id: &str, status: LikeStatus) -> Result<()> {
    if !valid_video_id(video_id) {
        bail!("Not a YouTube video ID: {video_id}");
    }
    let endpoint = match status {
        LikeStatus::Like => "like/like",
        LikeStatus::Dislike => "like/dislike",
        LikeStatus::Indifferent => "like/removelike",
    };
    account_call(endpoint, json!({"target": {"videoId": video_id}})).map(|_| ())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SongInfo {
    pub like: Option<LikeStatus>,
    pub album_browse_id: Option<String>,
    pub artist_browse_id: Option<String>,
}

/// The rating (when signed in) and the album and artist pages of a song.
#[allow(dead_code)] // used by the player sheet (in progress)
pub fn song_info(video_id: &str) -> Result<SongInfo> {
    if !valid_video_id(video_id) {
        bail!("Not a YouTube video ID: {video_id}");
    }
    let v = innertube_value("next", json!({"videoId": video_id, "isAudioOnly": true}))?;
    Ok(parse_song_info(&v, video_id))
}

fn find_panel_video<'a>(v: &'a Value, video_id: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => map
            .get("playlistPanelVideoRenderer")
            .filter(|r| r["videoId"].as_str() == Some(video_id))
            .or_else(|| map.values().find_map(|v| find_panel_video(v, video_id))),
        Value::Array(items) => items.iter().find_map(|v| find_panel_video(v, video_id)),
        _ => None,
    }
}

fn parse_song_info(v: &Value, video_id: &str) -> SongInfo {
    let like = find_renderer(v, "likeButtonRenderer")
        .and_then(|r| r["likeStatus"].as_str())
        .and_then(|s| match s {
            "LIKE" => Some(LikeStatus::Like),
            "DISLIKE" => Some(LikeStatus::Dislike),
            "INDIFFERENT" => Some(LikeStatus::Indifferent),
            _ => None,
        });
    let mut endpoints: Vec<&Value> = vec![];
    if let Some(panel) = find_panel_video(v, video_id) {
        if let Some(items) = panel
            .pointer("/menu/menuRenderer/items")
            .and_then(Value::as_array)
        {
            endpoints.extend(items.iter().filter_map(|i| {
                i.pointer("/menuNavigationItemRenderer/navigationEndpoint/browseEndpoint")
            }));
        }
        if let Some(runs) = panel
            .pointer("/longBylineText/runs")
            .and_then(Value::as_array)
        {
            endpoints.extend(
                runs.iter()
                    .filter_map(|r| r.pointer("/navigationEndpoint/browseEndpoint")),
            );
        }
    }
    let page_of = |kinds: &[&str]| {
        endpoints.iter().find_map(|e| {
            let page = e
                .pointer("/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
                .and_then(Value::as_str)?;
            let id = e["browseId"].as_str().filter(|id| !id.is_empty())?;
            kinds.contains(&page).then(|| id.to_owned())
        })
    };
    SongInfo {
        like,
        album_browse_id: page_of(&["MUSIC_PAGE_TYPE_ALBUM"]),
        artist_browse_id: page_of(&["MUSIC_PAGE_TYPE_ARTIST"])
            .or_else(|| page_of(&["MUSIC_PAGE_TYPE_USER_CHANNEL"])),
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

/// `path` without the Windows verbatim prefix `fs::canonicalize` adds (`\\?\C:\…` → `C:\…`),
/// so it compares equal to user-entered paths such as the download folder. UNC paths are kept.
pub fn plain_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Delete the downloaded audio file `source` from disk. Refuses anything outside `directory`,
/// so a library entry that points elsewhere can never remove the user's own music.
pub fn delete_download(source: &str, directory: &Path) -> Result<PathBuf> {
    let path = plain_path(Path::new(source));
    if !path.starts_with(directory) {
        bail!("That file is not in your download folder");
    }
    if !path.is_file() || !is_audio(&path) {
        bail!("No downloaded audio file at {}", path.display());
    }
    fs::remove_file(&path)?;
    Ok(path)
}

pub fn import_local(path: &Path) -> Result<Vec<Track>> {
    let path =
        plain_path(&fs::canonicalize(path).with_context(|| format!("Opening {}", path.display()))?);
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
                "-select_streams",
                "a:0",
                "-show_entries",
                // Ogg/Opus keep their tags on the audio stream rather than the container.
                "format=duration:format_tags=title,artist,album:stream_tags=title,artist,album",
                "-of",
                "json",
            ])
            .arg(&p);
        let meta = run(&mut command, Duration::from_secs(5))
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
            .unwrap_or(Value::Null);
        let tag = |key: &str| {
            [&meta["format"]["tags"], &meta["streams"][0]["tags"]]
                .into_iter()
                .flat_map(|tags| [&tags[key], &tags[key.to_uppercase()]])
                .filter_map(Value::as_str)
                .find(|v| !v.trim().is_empty())
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
    fn parses_song_info() {
        let cfg = |page: &str| json!({"browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": page}}});
        let ep = |id: &str, page: &str| {
            let mut e = cfg(page);
            e["browseId"] = json!(id);
            e
        };
        let v = json!({"contents": [
            {"playlistPanelVideoRenderer": {"videoId": "other", "menu": {"menuRenderer": {"items": [
                {"menuNavigationItemRenderer": {"navigationEndpoint": {"browseEndpoint": ep("MPREb_wrong", "MUSIC_PAGE_TYPE_ALBUM")}}}]}}}},
            {"playlistPanelVideoRenderer": {"videoId": "lYBUbBu4W08",
                "longBylineText": {"runs": [
                    {"text": "Artist", "navigationEndpoint": {"browseEndpoint": ep("UCartist", "MUSIC_PAGE_TYPE_USER_CHANNEL")}},
                    {"text": " - "}]},
                "menu": {"menuRenderer": {"items": [
                    {"menuServiceItemRenderer": {}},
                    {"menuNavigationItemRenderer": {"navigationEndpoint": {"browseEndpoint": ep("MPREb_right", "MUSIC_PAGE_TYPE_ALBUM")}}},
                    {"menuNavigationItemRenderer": {"navigationEndpoint": {"browseEndpoint": ep("UCreal", "MUSIC_PAGE_TYPE_ARTIST")}}}]}}}},
            {"likeButtonRenderer": {"likeStatus": "DISLIKE"}}]});
        assert_eq!(
            parse_song_info(&v, "lYBUbBu4W08"),
            SongInfo {
                like: Some(LikeStatus::Dislike),
                album_browse_id: Some("MPREb_right".into()),
                artist_browse_id: Some("UCreal".into()),
            }
        );
        assert_eq!(parse_song_info(&json!({}), "x"), SongInfo::default());
    }

    #[test]
    #[ignore = "requires internet access to YouTube Music; no account"]
    fn live_song_info() {
        let info = song_info("lYBUbBu4W08").unwrap();
        println!("{info:?}");
    }

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
    fn playlist_moves_port() {
        let v = |s: &str| s.chars().map(|c| c.to_string()).collect::<Vec<_>>();
        let apply = |cur: &[String], moves: &[PlaylistMove]| {
            let mut l = cur.to_vec();
            for m in moves {
                l.retain(|x| *x != m.set_video_id);
                let at = m
                    .before
                    .as_ref()
                    .and_then(|b| l.iter().position(|x| x == b))
                    .unwrap_or(l.len());
                l.insert(at, m.set_video_id.clone());
            }
            l
        };
        assert!(playlist_moves(&v("abc"), &v("abc")).is_empty());
        assert!(playlist_moves(&v("abc"), &v("abd")).is_empty());
        assert!(playlist_moves(&v("abc"), &v("ab")).is_empty());
        assert!(playlist_moves(&v("aab"), &v("aba")).is_empty());
        let one = playlist_moves(&v("abcde"), &v("acdeb"));
        assert_eq!(
            one,
            vec![PlaylistMove {
                set_video_id: "b".into(),
                before: None
            }]
        );
        for (a, b) in [
            ("abcde", "edcba"),
            ("abcdef", "badcfe"),
            ("abcd", "dabc"),
            ("abcde", "cabed"),
        ] {
            let moves = playlist_moves(&v(a), &v(b));
            assert_eq!(apply(&v(a), &moves), v(b), "{a} -> {b}");
        }
        assert_eq!(
            move_actions(&one),
            json!([{"action":"ACTION_MOVE_VIDEO_BEFORE","setVideoId":"b"}])
        );
        assert_eq!(
            move_actions(&playlist_moves(&v("abc"), &v("cab"))),
            json!([{"action":"ACTION_MOVE_VIDEO_BEFORE","setVideoId":"c","movedSetVideoIdSuccessor":"a"}])
        );
    }

    #[test]
    fn library_pages_merge_and_skip_tiles() {
        let card = |id: Option<&str>, t: &str| {
            let mut c = json!({"title":{"runs":[{"text":t}]}});
            if let Some(id) = id {
                c["navigationEndpoint"] = json!({"browseEndpoint":{"browseId":id}});
            }
            json!({"musicTwoRowItemRenderer": c})
        };
        let mut items = vec![];
        merge_playlist_cards(
            &json!({"items":[card(None,"New"),card(Some("VLPL1"),"One")]}),
            &mut items,
        );
        merge_playlist_cards(
            &json!({"items":[card(Some("VLPL1"),"One"),card(Some("VLPL2"),"Two")]}),
            &mut items,
        );
        let ids: Vec<_> = items.iter().filter_map(|i| i.browse_id.clone()).collect();
        assert_eq!(ids, ["VLPL1", "VLPL2"]);
    }

    #[test]
    fn collection_exposes_saved_description_and_privacy() {
        let page = |toggled: Value| {
            json!({"header":{"musicResponsiveHeaderRenderer":{
            "title":{"runs":[{"text":"T"}]},
            "description":{"musicDescriptionShelfRenderer":{"description":{"runs":[{"text":"About it"}]}}},
            "buttons":[
                {"musicPlayButtonRenderer":{"playNavigationEndpoint":{"watchPlaylistEndpoint":{"playlistId":"OLAK5uy_x"}}}},
                {"toggleButtonRenderer":{"isToggled":toggled,"defaultIcon":{"iconType":"BOOKMARK_BORDER"},"toggledIcon":{"iconType":"BOOKMARK"}}}
            ]}}})
        };
        let c = parse_collection(&page(json!(true)));
        assert_eq!(c.saved, Some(true));
        assert_eq!(c.description, "About it");
        assert_eq!(parse_collection(&page(json!(false))).saved, Some(false));
        assert_eq!(parse_collection(&json!({})).saved, None);
        let owned = json!({"header":{"musicEditablePlaylistDetailHeaderRenderer":{
            "editHeader":{"musicPlaylistEditHeaderRenderer":{"privacy":"UNLISTED","description":{"runs":[{"text":"Mine"}]}}},
            "header":{"musicResponsiveHeaderRenderer":{"title":{"simpleText":"Mine"}}}}}});
        let c = parse_collection(&owned);
        assert_eq!(c.privacy.as_deref(), Some("UNLISTED"));
        assert_eq!(c.description, "Mine");
        assert_eq!(c.saved, None);
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
    fn delete_download_only_touches_files_in_folder() {
        let dir = std::env::temp_dir().join(format!("bitchord-del-{}", std::process::id()));
        let downloads = dir.join("downloads");
        fs::create_dir_all(&downloads).unwrap();
        let inside = downloads.join("song.opus");
        let outside = dir.join("mine.opus");
        fs::write(&inside, b"a").unwrap();
        fs::write(&outside, b"b").unwrap();
        let verbatim = format!(r"\\?\{}", inside.display());
        assert!(delete_download(&outside.to_string_lossy(), &downloads).is_err());
        assert!(outside.exists());
        assert!(delete_download(&verbatim, &downloads).is_ok());
        assert!(!inside.exists());
        assert!(delete_download(&verbatim, &downloads).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn plain_path_drops_only_drive_verbatim_prefix() {
        assert_eq!(
            plain_path(Path::new(r"\\?\C:\Music\a.opus")),
            Path::new(r"C:\Music\a.opus")
        );
        assert_eq!(
            plain_path(Path::new(r"C:\Music\a.opus")),
            Path::new(r"C:\Music\a.opus")
        );
        let unc = Path::new(r"\\?\UNC\server\share\a.opus");
        assert_eq!(plain_path(unc), unc);
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
    fn mood_button(title: &str, id: &str, params: Option<&str>, color: Value) -> Value {
        let mut endpoint = json!({ "browseId": id });
        if let Some(p) = params {
            endpoint["params"] = json!(p);
        }
        json!({"musicNavigationButtonRenderer":{
            "buttonText":{"runs":[{"text":title}]},
            "solid":{"leftStripeColor":color},
            "clickCommand":{"browseEndpoint":endpoint}
        }})
    }
    fn mood_page(grids: Vec<(&str, Vec<Value>)>) -> Value {
        let contents: Vec<Value> = grids
            .into_iter()
            .map(|(title, items)| {
                json!({"gridRenderer":{
                    "header":{"gridHeaderRenderer":{"title":{"runs":[{"text":title}]}}},
                    "items":items
                }})
            })
            .collect();
        json!({"contents":{"singleColumnBrowseResultsRenderer":{"tabs":[{"tabRenderer":{"content":{"sectionListRenderer":{"contents":contents}}}}]}}})
    }
    #[test]
    fn parses_mood_and_genre_grids_with_params_and_colours() {
        let cat = "FEmusic_moods_and_genres_category";
        let page = mood_page(vec![
            (
                "Moods & moments",
                vec![
                    mood_button("Chill", cat, Some("p-chill"), json!(4283584335u64)),
                    // A colour sent as a string and a signed Java int both decode to ARGB.
                    mood_button("Focus", cat, Some("p-focus"), json!("-16777216")),
                    // No colour, and the older navigationEndpoint form.
                    json!({"musicNavigationButtonRenderer":{"buttonText":{"runs":[{"text":"Sleep"}]},"navigationEndpoint":{"browseEndpoint":{"browseId":"FEsleep"}}}}),
                ],
            ),
            (
                "Genres",
                vec![
                    mood_button("Pop", cat, Some("p-pop"), json!("not a colour")),
                    // Skipped: no title, no endpoint, not a button.
                    mood_button("", cat, Some("p-x"), Value::Null),
                    json!({"musicNavigationButtonRenderer":{"buttonText":{"runs":[{"text":"Broken"}]}}}),
                    json!({"musicTwoRowItemRenderer":{}}),
                ],
            ),
            ("Empty", vec![]),
            ("", vec![mood_button("Untitled", "FEx", None, Value::Null)]),
        ]);
        let sections = parse_mood_and_genres(&page);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].title, "Moods & moments");
        let chill = &sections[0].items[0];
        assert_eq!(chill.title, "Chill");
        assert_eq!(chill.browse_id, cat);
        assert_eq!(chill.params.as_deref(), Some("p-chill"));
        assert_eq!(chill.color, Some(4283584335));
        assert_eq!(chill.thumbnail, None);
        assert_eq!(chill.key(), format!("{cat}|p-chill"));
        assert_eq!(sections[0].items[1].color, Some(0xFF00_0000));
        assert_eq!(sections[0].items[2].params, None);
        assert_eq!(sections[0].items[2].color, None);
        assert_eq!(sections[1].items.len(), 1);
        assert_eq!(sections[1].items[0].color, None);
        assert!(parse_mood_and_genres(&json!({})).is_empty());
    }
    #[test]
    fn drops_signed_in_shortcut_grid() {
        let pop = || mood_button("Pop", "FEcat", Some("p-pop"), Value::Null);
        let page = mood_page(vec![
            // A localised "For you" heading is caught by repeating a later button.
            ("Pour vous", vec![pop()]),
            (
                "For You",
                vec![mood_button("Rare", "FEcat", Some("p-rare"), Value::Null)],
            ),
            (
                "Genres",
                vec![
                    pop(),
                    mood_button("Pop", "FEcat", Some("p-other"), Value::Null),
                ],
            ),
        ]);
        let sections = parse_mood_and_genres(&page);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].title, "Genres");
        assert_eq!(sections[0].items.len(), 2);
    }
    #[test]
    fn mood_artwork_is_first_real_cover_and_empty_shelves_drop() {
        let shelf = |title: &str, thumbnail: Option<&str>| HomeShelf {
            title: title.into(),
            subtitle: String::new(),
            items: thumbnail
                .map(|t| ShelfItem {
                    title: "x".into(),
                    thumbnail: t.into(),
                    ..Default::default()
                })
                .into_iter()
                .collect(),
        };
        let shelves = filled_shelves(vec![
            shelf("Empty", None),
            shelf("A", Some("")),
            shelf("B", Some("https://cover")),
        ]);
        assert_eq!(shelves.len(), 2);
        assert_eq!(first_artwork(&shelves).as_deref(), Some("https://cover"));
        assert_eq!(first_artwork(&shelves[..1]), None);
        // A category page is ordinary carousels of playlist cards.
        let category = parse_home(
            &json!({"contents":{"sectionListRenderer":{"contents":[{"gridRenderer":{"items":[]}},{"musicCarouselShelfRenderer":{"header":{"musicCarouselShelfBasicHeaderRenderer":{"title":{"runs":[{"text":"Featured playlists"}]}}},"contents":[{"musicTwoRowItemRenderer":{"title":{"runs":[{"text":"Chill Mix"}]},"thumbnailRenderer":{"musicThumbnailRenderer":{"thumbnail":{"thumbnails":[{"url":"https://mix"}]}}},"navigationEndpoint":{"browseEndpoint":{"browseId":"VLRDCLAK"}}}}]}}]}}}),
        );
        assert_eq!(category.shelves[0].title, "Featured playlists");
        assert_eq!(
            category.shelves[0].items[0].browse_id.as_deref(),
            Some("VLRDCLAK")
        );
        assert_eq!(
            first_artwork(&category.shelves).as_deref(),
            Some("https://mix")
        );
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
    #[ignore = "requires FFmpeg and ffprobe"]
    fn imports_opus_stream_tags() {
        let dir = std::env::temp_dir().join(format!("bitchord-opus-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("file_name [id].opus");
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "quiet",
                "-f",
                "lavfi",
                "-i",
                "anullsrc=d=1",
                "-c:a",
                "libopus",
            ])
            .args([
                "-metadata",
                "title=Real Title",
                "-metadata",
                "artist=Real Artist",
            ])
            .arg(&file)
            .status()
            .unwrap();
        assert!(status.success());
        let tracks = import_local(&file).unwrap();
        fs::remove_dir_all(dir).unwrap();
        assert_eq!(tracks[0].title, "Real Title");
        assert_eq!(tracks[0].artist, "Real Artist");
        assert!(!tracks[0].source.starts_with(r"\\?\"));
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
    }
    #[test]
    #[ignore = "requires internet access to YouTube Music"]
    fn live_explore() {
        let sections = moods_and_genres().unwrap();
        let count: usize = sections.iter().map(|s| s.items.len()).sum();
        println!(
            "moods & genres: {} sections / {count} categories",
            sections.len()
        );
        assert!(count > 0);
        let category = &sections[0].items[0];
        let params = category.params.as_deref();
        let shelves = mood_genre_shelves(&category.browse_id, params).unwrap();
        assert!(!shelves.is_empty(), "A category must open playlist shelves");
        let artwork = mood_genre_artwork(&category.browse_id, params).unwrap();
        println!(
            "{}: {} shelves; artwork {}",
            category.title,
            shelves.len(),
            artwork.is_some()
        );
        for (name, shelves) in [
            ("explore", explore().unwrap()),
            ("charts", charts().unwrap()),
            ("new releases", new_releases().unwrap()),
        ] {
            let titles: Vec<String> = shelves
                .iter()
                .map(|s| format!("{} ({})", s.title, s.items.len()))
                .collect();
            println!("{name}: {}", titles.join(", "));
            assert!(!shelves.is_empty(), "{name} must have shelves");
        }
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
