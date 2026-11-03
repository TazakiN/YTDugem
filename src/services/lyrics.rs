use crate::{
    model::{TimedWord, Track},
    services::LyricLine,
};
use anyhow::{anyhow, Result};
use hmac::Mac;
use serde_json::Value;
use std::{io::Read, time::Duration};

#[derive(Clone, Debug, PartialEq)]
pub struct Lyrics {
    pub provider: &'static str,
    pub lines: Vec<LyricLine>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Provider {
    pub id: &'static str,
    pub label: &'static str,
    pub detail: &'static str,
    pub word_timed: bool,
    pub key_id: Option<&'static str>,
}

pub fn key_configured(key_id: &str) -> Result<bool> {
    crate::store::key_configured(key_id)
}
pub fn save_key(key_id: &str, value: &str) -> Result<()> {
    crate::store::save_key(key_id, value)
}

pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "lrc_red",
        label: "lrc.red",
        detail: "Apple lyrics by recording",
        word_timed: true,
        key_id: None,
    },
    Provider {
        id: "bini_lyrics",
        label: "BiniLyrics",
        detail: "Apple lyrics matched by recording",
        word_timed: true,
        key_id: None,
    },
    Provider {
        id: "betterlyrics",
        label: "BetterLyrics",
        detail: "Apple Music word-timed lyrics",
        word_timed: true,
        key_id: None,
    },
    Provider {
        id: "betterlyrics_portato",
        label: "BetterLyrics Portato",
        detail: "QQ Music karaoke lyrics",
        word_timed: true,
        key_id: None,
    },
    Provider {
        id: "paxsenix",
        label: "PaxSenix",
        detail: "Apple Music lyrics",
        word_timed: true,
        key_id: None,
    },
    Provider {
        id: "paxsenix_spotify",
        label: "PaxSenix: Spotify",
        detail: "Spotify lyrics (API key required)",
        word_timed: false,
        key_id: Some("paxsenix"),
    },
    Provider {
        id: "paxsenix_musixmatch",
        label: "PaxSenix: Musixmatch",
        detail: "Musixmatch lyrics (API key required)",
        word_timed: true,
        key_id: Some("paxsenix"),
    },
    Provider {
        id: "lyrics_plus",
        label: "LyricsPlus",
        detail: "Community-hosted syllable timing",
        word_timed: true,
        key_id: None,
    },
    Provider {
        id: "unison",
        label: "Unison",
        detail: "Community contributed lyrics",
        word_timed: true,
        key_id: None,
    },
    Provider {
        id: "youtube_captions",
        label: "YouTube captions",
        detail: "Timed captions for this video",
        word_timed: false,
        key_id: None,
    },
    Provider {
        id: "youtube_music",
        label: "YouTube Music",
        detail: "Plain lyrics from this video's Lyrics tab",
        word_timed: false,
        key_id: None,
    },
    Provider {
        id: "kugou",
        label: "KuGou",
        detail: "Line-synced lyrics",
        word_timed: false,
        key_id: None,
    },
    Provider {
        id: "lrclib",
        label: "LRCLIB",
        detail: "Community line-synced or plain lyrics",
        word_timed: false,
        key_id: None,
    },
    Provider {
        id: "genius",
        label: "Genius",
        detail: "Plain-text lyrics",
        word_timed: false,
        key_id: None,
    },
    Provider {
        id: "musixmatch",
        label: "Musixmatch",
        detail: "Rich-sync lyrics with subtitle fallback",
        word_timed: true,
        key_id: None,
    },
];

pub fn ordered(saved: &[String]) -> Vec<&'static Provider> {
    let mut result = Vec::new();
    for id in saved {
        if let Some(provider) = PROVIDERS.iter().find(|p| p.id == id) {
            if !result.contains(&provider) {
                result.push(provider);
            }
        }
    }
    for (index, provider) in PROVIDERS.iter().enumerate() {
        if !result.contains(&provider) {
            let previous = PROVIDERS[..index].iter().rev().find(|p| result.contains(p));
            let at = previous.map_or(0, |p| result.iter().position(|x| *x == p).unwrap() + 1);
            result.insert(at, provider);
        }
    }
    result
}

pub fn lookup(
    track: &Track,
    enabled: &[String],
    order: &[String],
    only: Option<&str>,
) -> Result<Lyrics> {
    if only.is_none() {
        if let Some(lines) = embedded(track) {
            return Ok(Lyrics {
                provider: "Downloaded",
                lines,
            });
        }
    }
    let sequence = ordered(order);
    let mut errors = Vec::new();
    for provider in sequence {
        if only.is_some_and(|id| id != provider.id)
            || (only.is_none() && !enabled.iter().any(|id| id == provider.id))
        {
            continue;
        }
        let found = match provider.id {
            "lrc_red" => lrc_red(track),
            "bini_lyrics" => bini_lyrics(track),
            "betterlyrics" => betterlyrics(track),
            "betterlyrics_portato" => betterlyrics_portato(track),
            "paxsenix" => paxsenix(track),
            "paxsenix_spotify" => paxsenix_keyed(track, "spotify", only.is_some()),
            "paxsenix_musixmatch" => paxsenix_keyed(track, "musixmatch", only.is_some()),
            "lyrics_plus" => lyrics_plus(track),
            "youtube_music" => youtube_music(track),
            "youtube_captions" => youtube_captions(track),
            "lrclib" => lrclib(track),
            "unison" => unison(track),
            "kugou" => kugou(track),
            "genius" => genius(track),
            "musixmatch" => musixmatch(track),
            _ => unreachable!("all offered providers have handlers"),
        };
        match found {
            Ok(Some(lines)) if lines.iter().any(|line| !line.text.trim().is_empty()) => {
                return Ok(Lyrics {
                    provider: provider.label,
                    lines,
                });
            }
            Ok(_) => (),
            Err(error) => errors.push(format!("{} unavailable: {error}", provider.label)),
        }
    }
    if only.is_some() {
        if let Some(error) = errors.last() {
            Err(anyhow!("{error}"))
        } else {
            Err(anyhow!("No lyrics from the selected provider"))
        }
    } else if !errors.is_empty() {
        Err(anyhow!("No lyrics found; {}", errors.join("; ")))
    } else {
        Err(anyhow!("No lyrics found from enabled providers"))
    }
}

fn embedded(track: &Track) -> Option<Vec<LyricLine>> {
    let path = std::path::Path::new(&track.source);
    if !path.is_file() {
        return None;
    }
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("m3u8"))
    {
        let sidecar = path.parent()?.join("lyrics.lrc");
        let size = std::fs::metadata(&sidecar).ok()?.len();
        if size == 0 || size > 8 * 1024 * 1024 {
            return None;
        }
        let text = std::fs::read_to_string(sidecar).ok()?;
        let lines = parse_provider(&text)?;
        if usable(&lines) {
            return Some(lines);
        }
    }
    let output = super::run(
        std::process::Command::new("ffprobe")
            .args([
                "-v",
                "quiet",
                "-show_entries",
                "format_tags=lyrics,bitchord_lyrics",
                "-of",
                "json",
            ])
            .arg(path),
        Duration::from_secs(5),
    )
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let root: Value = serde_json::from_slice(&output.stdout).ok()?;
    let tags = &root["format"]["tags"];
    let text = tags["BITCHORD_LYRICS"]
        .as_str()
        .or_else(|| tags["bitchord_lyrics"].as_str())
        .or_else(|| tags["LYRICS"].as_str())
        .or_else(|| tags["lyrics"].as_str())?;
    let lines = parse_provider(text)?;
    usable(&lines).then_some(lines)
}

fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(7))
        .user_agent("YTDugem/0.1 (lyrics lookup)")
        .build()?)
}

const RESPONSE_LIMIT: u64 = 8 * 1024 * 1024;

fn get(url: reqwest::blocking::RequestBuilder) -> Result<Option<String>> {
    let response = url
        .send()
        .map_err(|_| anyhow!("provider network request failed"))?;
    if !successful_or_miss(response.status())? {
        return Ok(None);
    }
    let bytes = read_bounded(response)?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

fn read_bounded(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(RESPONSE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("provider response could not be read"))?;
    if bytes.len() as u64 > RESPONSE_LIMIT {
        return Err(anyhow!("provider response exceeds 8 MiB"));
    }
    Ok(bytes)
}

fn successful_or_miss(status: reqwest::StatusCode) -> Result<bool> {
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if status.is_success() {
        return Ok(true);
    }
    Err(anyhow!(
        "provider request failed (HTTP {})",
        status.as_u16()
    ))
}

fn betterlyrics(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    let url = reqwest::Url::parse("https://lyrics-api.boidu.dev/getLyrics")?;
    let duration = seconds(track);
    let text = get(http()?.get(url).query(&[
        ("s", track.title.as_str()),
        ("a", track.artist.as_str()),
        ("d", duration.as_str()),
        ("al", track.album.as_str()),
    ]))?;
    Ok(text
        .and_then(|body| parse_provider(&body))
        .filter(|lines| usable(lines)))
}

fn seconds(track: &Track) -> String {
    if track.duration.is_finite() && track.duration > 0.0 && track.duration < 86_400.0 {
        (track.duration.round() as u64).to_string()
    } else {
        "0".into()
    }
}

fn lrc_red(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    let url = reqwest::Url::parse("https://lrc.red/search.json")?;
    let body = get(http()?
        .get(url)
        .query(&[("q", format!("{} {}", track.title, track.artist))]))?;
    let Some(body) = body else { return Ok(None) };
    let root: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let hits = root["hits"]
        .as_array()
        .or_else(|| root["results"].as_array())
        .cloned()
        .unwrap_or_default();
    let wanted = normalize(&track.title);
    for hit in hits {
        if normalize(hit["title"].as_str().unwrap_or_default()) != wanted {
            continue;
        }
        let artists = normalize(hit["artist"].as_str().unwrap_or_default());
        if !normalize(&track.artist).is_empty() && !artists.contains(&normalize(&track.artist)) {
            continue;
        }
        let duration = hit["duration"].as_f64().unwrap_or(0.0);
        if track.duration.is_finite()
            && track.duration > 0.0
            && duration > 0.0
            && (duration - track.duration).abs() > 3.0
        {
            continue;
        }
        let isrc = hit["isrc"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_uppercase();
        if !valid_isrc(&isrc) {
            continue;
        }
        let url = format!("https://lrc.red/s/{isrc}.ttml");
        if let Some(text) = get(http()?.get(&url))? {
            let lines = parse_provider(&text).unwrap_or_default();
            if usable(&lines) {
                return Ok(Some(lines));
            }
        }
    }
    Ok(None)
}

fn bini_lyrics(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    let mut url = reqwest::Url::parse("https://lyrics-api.binimum.org/")?;
    url.query_pairs_mut()
        .append_pair("track", &track.title)
        .append_pair("artist", &track.artist);
    if !track.album.is_empty() {
        url.query_pairs_mut().append_pair("album", &track.album);
    }
    let Some(body) = get(http()?.get(url).header(
        reqwest::header::USER_AGENT,
        "BitChord (https://github.com/bitchord)",
    ))?
    else {
        return Ok(None);
    };
    let root: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let results = root["results"]
        .as_array()
        .or_else(|| root["data"].as_array())
        .cloned()
        .unwrap_or_default();
    for result in results {
        if normalize(result["track_name"].as_str().unwrap_or_default()) != normalize(&track.title) {
            continue;
        }
        if !normalize(&track.artist).is_empty()
            && !normalize(result["artist_name"].as_str().unwrap_or_default())
                .contains(&normalize(&track.artist))
        {
            continue;
        }
        let duration = result["duration"].as_f64().unwrap_or(0.0);
        if track.duration.is_finite()
            && track.duration > 0.0
            && duration > 0.0
            && (duration - track.duration).abs() > 8.0
        {
            continue;
        }
        if let Some(url) = result["lyricsUrl"].as_str() {
            let Ok(url) = reqwest::Url::parse(url) else {
                continue;
            };
            if url.scheme() != "https" || url.host_str() != Some("lrc.red") {
                continue;
            }
            if let Some(doc) = get(http()?.get(url))? {
                let lines = parse_provider(&doc).unwrap_or_default();
                if usable(&lines) {
                    return Ok(Some(lines));
                }
            }
        }
    }
    Ok(None)
}

fn valid_isrc(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 12
        && bytes[..2].iter().all(|c| c.is_ascii_uppercase())
        && bytes[2..5].iter().all(|c| c.is_ascii_alphanumeric())
        && bytes[5..].iter().all(u8::is_ascii_digit)
}

fn betterlyrics_portato(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    let url = reqwest::Url::parse("https://lyrics-api.boidu.dev/qq/getLyrics")?;
    let duration = seconds(track);
    let body = get(http()?.get(url).query(&[
        ("s", track.title.as_str()),
        ("a", track.artist.as_str()),
        ("d", duration.as_str()),
        ("al", track.album.as_str()),
    ]))?;
    Ok(body
        .and_then(|s| parse_provider(&s))
        .filter(|lines| usable(lines)))
}

fn lyrics_plus(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    const HOSTS: &[&str] = &[
        "https://lyricsplus.prjktla.my.id",
        "https://lyricsplus.atomix.one",
        "https://lyricsplus.binimum.org",
        "https://lyricsplus.prjktla.workers.dev",
        "https://lyricsplus-seven.vercel.app",
        "https://lyrics-plus-backend.vercel.app",
    ];
    let mut outage = None;
    let mut responded = false;
    for host in HOSTS {
        let url = reqwest::Url::parse(&format!("{host}/v2/lyrics/get"))?;
        let duration = seconds(track);
        let body = match get(http()?.get(url).query(&[
            ("title", track.title.as_str()),
            ("artist", track.artist.as_str()),
            ("duration", duration.as_str()),
            ("album", track.album.as_str()),
        ])) {
            Ok(body) => body,
            Err(error) => {
                outage = Some(error);
                continue;
            }
        };
        responded = true;
        if let Some(lines) = body
            .and_then(|s| parse_lyrics_plus(&s))
            .filter(|lines| usable(lines))
        {
            return Ok(Some(lines));
        }
    }
    if !responded {
        if let Some(error) = outage {
            return Err(error);
        }
    }
    Ok(None)
}

fn paxsenix_keyed(track: &Track, service: &str, explicit: bool) -> Result<Option<Vec<LyricLine>>> {
    let Some(key) = paxsenix_key(crate::store::lyric_key("paxsenix")?, explicit)? else {
        return Ok(None);
    };
    let client = http()?;
    if service == "spotify" {
        let search = paxsenix_search_url("spotify/search", &track.title, &track.artist)?;
        let id = get(client.get(search).bearer_auth(&key))?
            .and_then(|body| serde_json::from_str::<Value>(&body).ok())
            .and_then(|root| paxsenix_best_candidate(&root, track))
            .map(|candidate| candidate.id);
        if let Some(id) = id {
            let lyrics = paxsenix_lyrics_url("lyrics/spotify", &id)?;
            if let Some(lines) = get(client.get(lyrics).bearer_auth(&key))?
                .and_then(|body| parse_provider(&body))
                .filter(|lines| usable(lines))
            {
                return Ok(Some(lines));
            }
        }
    } else {
        let lyrics = paxsenix_musixmatch_url(track)?;
        if let Some(lines) = get(client.get(lyrics).bearer_auth(&key))?
            .and_then(|body| parse_provider(&body))
            .filter(|lines| usable(lines))
        {
            return Ok(Some(lines));
        }
    }
    let fallback = paxsenix_search_url("lyrics/lrcget", &track.title, &track.artist)?;
    Ok(get(client.get(fallback).bearer_auth(&key))?
        .and_then(|body| parse_paxsenix_lrcget(&body, track))
        .filter(|lines| usable(lines)))
}

fn paxsenix_key(key: Option<String>, explicit: bool) -> Result<Option<String>> {
    let key = key.unwrap_or_default();
    let key = key.trim();
    let key = key
        .strip_prefix("Bearer ")
        .or_else(|| {
            key.get(..7)
                .filter(|prefix| prefix.eq_ignore_ascii_case("Bearer "))
                .map(|_| &key[7..])
        })
        .unwrap_or(key)
        .trim();
    if key.is_empty() && explicit {
        Err(anyhow!(
            "Configure a PaxSenix API key in Settings to use this provider"
        ))
    } else if key.is_empty() {
        Ok(None)
    } else {
        Ok(Some(key.to_owned()))
    }
}

fn paxsenix_search_url(path: &str, title: &str, artist: &str) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(&format!("https://api.paxsenix.org/{path}"))?;
    url.query_pairs_mut()
        .append_pair("q", &format!("{title} {artist}"));
    Ok(url)
}

fn paxsenix_lyrics_url(path: &str, id: &str) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(&format!("https://api.paxsenix.org/{path}"))?;
    url.query_pairs_mut().append_pair("id", id);
    Ok(url)
}

fn paxsenix_musixmatch_url(track: &Track) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse("https://api.paxsenix.org/lyrics/musixmatch")?;
    url.query_pairs_mut()
        .append_pair("t", &track.title)
        .append_pair("a", &track.artist)
        .append_pair("d", &seconds(track));
    Ok(url)
}

struct PaxSenixCandidate {
    id: String,
    score: i32,
    duration_distance: f64,
}

fn paxsenix_best_candidate(root: &Value, track: &Track) -> Option<PaxSenixCandidate> {
    fn objects<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
        match value {
            Value::Object(map) => {
                out.push(value);
                for child in map.values() {
                    objects(child, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| objects(item, out)),
            _ => (),
        }
    }
    let mut candidates = Vec::new();
    objects(root, &mut candidates);
    candidates
        .into_iter()
        .filter_map(|item| {
            let metadata = item.get("attributes").unwrap_or(item);
            let id = ["id", "trackId", "track_id", "realId"]
                .iter()
                .find_map(|key| item.get(*key).and_then(json_scalar_string))
                .or_else(|| {
                    ["id", "trackId", "track_id", "realId"]
                        .iter()
                        .find_map(|key| metadata.get(*key).and_then(json_scalar_string))
                })?;
            let title = ["name", "title", "trackName", "track_name"]
                .iter()
                .find_map(|key| metadata.get(*key).and_then(Value::as_str))?;
            let artist = ["artistName", "artist_name", "artist"]
                .iter()
                .find_map(|key| metadata.get(*key).and_then(paxsenix_artist))
                .unwrap_or_default();
            let duration = ["durationInMillis", "durationMs", "duration_ms", "duration"]
                .iter()
                .find_map(|key| metadata.get(*key).and_then(Value::as_f64))
                .map(|duration| {
                    if duration >= 10_000.0 {
                        duration / 1000.0
                    } else {
                        duration
                    }
                });
            let title_score = paxsenix_title_score(title, &track.title);
            let score = title_score
                + paxsenix_text_score(&artist, &track.artist, 15, 5)
                + match (duration, track.duration > 0.0 && track.duration.is_finite()) {
                    (Some(duration), true) if (duration - track.duration).abs() < 3.0 => 10,
                    (Some(duration), true) if (duration - track.duration).abs() < 10.0 => 5,
                    _ => 0,
                };
            Some(PaxSenixCandidate {
                id,
                score: if title_score > 0 { score } else { 0 },
                duration_distance: duration
                    .filter(|_| track.duration > 0.0 && track.duration.is_finite())
                    .map_or(f64::MAX, |duration| (duration - track.duration).abs()),
            })
        })
        .filter(|candidate| candidate.score >= 10)
        .max_by(|a, b| {
            a.score
                .cmp(&b.score)
                .then_with(|| b.duration_distance.total_cmp(&a.duration_distance))
        })
}

fn json_scalar_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_i64().map(|number| number.to_string()))
}

fn paxsenix_artist(value: &Value) -> Option<String> {
    match value {
        Value::String(name) => Some(name.clone()),
        Value::Object(map) => ["name", "artistName", "title"]
            .iter()
            .find_map(|key| map.get(*key).and_then(Value::as_str).map(str::to_owned)),
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(paxsenix_artist)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        _ => None,
    }
}

fn paxsenix_text_score(candidate: &str, wanted: &str, exact: i32, partial: i32) -> i32 {
    let candidate_normalized = normalize(candidate);
    let wanted_normalized = normalize(wanted);
    if candidate_normalized.is_empty() || wanted_normalized.is_empty() {
        0
    } else if candidate_normalized == wanted_normalized {
        exact
    } else if candidate_normalized.contains(&wanted_normalized)
        || wanted_normalized.contains(&candidate_normalized)
    {
        partial
    } else {
        0
    }
}

fn paxsenix_title_score(candidate: &str, wanted: &str) -> i32 {
    let candidate_normalized = normalize(candidate);
    let wanted_normalized = normalize(wanted);
    if candidate_normalized == wanted_normalized && !wanted_normalized.is_empty() {
        20
    } else if paxsenix_featured_title(&candidate_normalized, &wanted_normalized) {
        10
    } else {
        0
    }
}

fn paxsenix_featured_title(candidate: &str, wanted: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix(wanted) else {
        return false;
    };
    ["feat", "ft", "featuring"]
        .iter()
        .any(|marker| suffix.starts_with(marker) && suffix.len() > marker.len())
}

fn parse_paxsenix_lrcget(raw: &str, track: &Track) -> Option<Vec<LyricLine>> {
    let root: Value = serde_json::from_str(raw).ok()?;
    let documents = root.get("lyrics")?.as_array()?;
    documents
        .iter()
        .filter_map(|document| {
            let metadata = document.get("attributes").unwrap_or(document);
            let title = ["name", "title", "trackName", "track_name"]
                .iter()
                .find_map(|key| metadata.get(*key).and_then(Value::as_str))
                .unwrap_or_default();
            let artist = ["artistName", "artist_name", "artist"]
                .iter()
                .find_map(|key| metadata.get(*key).and_then(paxsenix_artist))
                .unwrap_or_default();
            let duration = ["durationInMillis", "durationMs", "duration_ms", "duration"]
                .iter()
                .find_map(|key| metadata.get(*key).and_then(Value::as_f64))
                .map(|duration| {
                    if duration >= 10_000.0 {
                        duration / 1000.0
                    } else {
                        duration
                    }
                });
            let title_score = paxsenix_title_score(title, &track.title);
            let score = title_score
                + paxsenix_text_score(&artist, &track.artist, 15, 5)
                + match (duration, track.duration > 0.0 && track.duration.is_finite()) {
                    (Some(duration), true) if (duration - track.duration).abs() < 3.0 => 10,
                    (Some(duration), true) if (duration - track.duration).abs() < 10.0 => 5,
                    _ => 0,
                };
            if title_score == 0 || score < 10 {
                return None;
            }
            let lyrics = document.get("lyrics").unwrap_or(document);
            let text = match lyrics {
                Value::String(text) => text.clone(),
                _ => lyrics.to_string(),
            };
            let lines = parse_provider(&text).filter(|lines| usable(lines))?;
            let distance = duration
                .filter(|_| track.duration > 0.0 && track.duration.is_finite())
                .map_or(f64::MAX, |duration| (duration - track.duration).abs());
            Some((score, distance, lines))
        })
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.total_cmp(&a.1)))
        .map(|(_, _, lines)| lines)
}

fn paxsenix(_track: &Track) -> Result<Option<Vec<LyricLine>>> {
    paxsenix_apple(_track)
}

fn musixmatch(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    let search = mxm_signed_url(
        "track.search",
        &[
            ("q_track", &track.title),
            ("q_artist", &track.artist),
            ("f_has_lyrics", "1"),
            ("s_track_rating", "desc"),
            ("quorum_factor", "1"),
            ("page_size", "10"),
            ("page", "1"),
        ],
    )?;
    let Some(body) = mxm_request_with_refresh(search)? else {
        return Ok(None);
    };
    let root: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let entries = root
        .pointer("/message/body/track_list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("track"))
        .collect::<Vec<_>>();
    let title = normalize(&track.title);
    let artist = normalize(&track.artist);
    let best = entries
        .into_iter()
        .filter(|item| {
            let candidate_title = normalize(item["track_name"].as_str().unwrap_or(""));
            let candidate_artist = normalize(item["artist_name"].as_str().unwrap_or(""));
            !title.is_empty()
                && (candidate_title == title
                    || candidate_title.contains(&title)
                    || title.contains(&candidate_title))
                && (artist.is_empty() || candidate_artist.contains(&artist))
                && (item["track_length"]
                    .as_f64()
                    .is_none_or(|d| track.duration <= 0.0 || (d - track.duration).abs() <= 10.0))
        })
        .max_by_key(|item| {
            let exact = normalize(item["track_name"].as_str().unwrap_or("")) == title;
            let duration = item["track_length"].as_f64().unwrap_or(track.duration);
            (
                exact,
                std::cmp::Reverse(((duration - track.duration).abs() * 1000.0) as i64),
            )
        });
    let Some(best) = best else {
        return Ok(None);
    };
    let id = best["track_id"]
        .as_i64()
        .or_else(|| best["track_id"].as_str()?.parse().ok());
    let Some(id) = id else { return Ok(None) };

    if best["has_richsync"].as_i64().unwrap_or(0) != 0 {
        let url = mxm_signed_url("track.richsync.get", &[("track_id", &id.to_string())])?;
        if let Some(raw) = mxm_request_with_refresh(url)? {
            if let Some(payload) = serde_json::from_str::<Value>(&raw).ok().and_then(|v| {
                v.pointer("/message/body/richsync/richsync_body")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }) {
                let lines = parse_mxm_richsync(&payload);
                if lines.iter().any(|line| !line.words.is_empty()) {
                    return Ok(Some(lines));
                }
            }
        }
    }
    if best["has_subtitles"].as_i64().unwrap_or(0) == 0 {
        return Ok(None);
    }
    let url = mxm_signed_url(
        "track.subtitle.get",
        &[("track_id", &id.to_string()), ("subtitle_format", "mxm")],
    )?;
    let Some(raw) = mxm_request_with_refresh(url)? else {
        return Ok(None);
    };
    let root: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let Some(subtitle) = root
        .pointer("/message/body/subtitle/subtitle_body")
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let lines = parse_mxm_subtitles(subtitle);
    Ok(usable(&lines).then_some(lines))
}

const MXM_BASE: &str = "https://apic.musixmatch.com/ws/1.1";
const MXM_FALLBACK_SECRET: &str = "f09016176ba43a1cfd1031fbd6b3d26c";
const MXM_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

fn mxm_secret() -> String {
    let cache = MXM_SECRET_CACHE.get_or_init(|| std::sync::Mutex::new(None));
    if let Some(secret) = cache.lock().ok().and_then(|cached| cached.clone()) {
        return secret;
    }
    let secret = {
        let scrape = || -> Option<String> {
            let page = get(http()
                .ok()?
                .get("https://www.musixmatch.com/search")
                .header(reqwest::header::USER_AGENT, MXM_AGENT)
                .header(reqwest::header::COOKIE, "mxm_bab=AB"))
            .ok()??;
            let script = page
                .split("/_next/static/chunks/pages/_app-")
                .nth(1)?
                .split('"')
                .next()?;
            let js = get(http()
                .ok()?
                .get(format!(
                    "https://www.musixmatch.com/_next/static/chunks/pages/_app-{script}"
                ))
                .header(reqwest::header::USER_AGENT, MXM_AGENT)
                .header(reqwest::header::COOKIE, "mxm_bab=AB"))
            .ok()??;
            let encoded = js.split("from(\"").nth(1)?.split("\".split").next()?;
            let decoded = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                encoded.chars().rev().collect::<String>(),
            )
            .ok()?;
            String::from_utf8(decoded).ok().filter(|s| !s.is_empty())
        };
        scrape().unwrap_or_else(|| MXM_FALLBACK_SECRET.to_owned())
    };
    if let Ok(mut cached) = cache.lock() {
        *cached = Some(secret.clone());
    }
    secret
}

fn mxm_token() -> Result<Option<String>> {
    let cache = MXM_TOKEN_CACHE.get_or_init(|| std::sync::Mutex::new(None));
    if let Some(token) = cache
        .lock()
        .map_err(|_| anyhow!("Musixmatch token cache unavailable"))?
        .clone()
    {
        return Ok(Some(token));
    }
    static GUID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let guid = GUID.get_or_init(|| format!("{:032x}", rand::random::<u128>()));
    let url = mxm_signed_url("token.get", &[("guid", guid)])?;
    let Some(body) = get(http()?
        .get(url)
        .header(reqwest::header::USER_AGENT, MXM_AGENT))?
    else {
        return Ok(None);
    };
    let root: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let token = root
        .pointer("/message/body/user_token")
        .and_then(Value::as_str)
        .map(str::to_owned);
    *cache
        .lock()
        .map_err(|_| anyhow!("Musixmatch token cache unavailable"))? = token.clone();
    Ok(token)
}

fn mxm_signed_url(endpoint: &str, params: &[(&str, &str)]) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(&format!("{MXM_BASE}/{endpoint}"))?;
    url.query_pairs_mut()
        .append_pair("app_id", "mobile-app-v1.0")
        .append_pair("format", "json");
    for (key, value) in params {
        url.query_pairs_mut().append_pair(key, value);
    }
    if endpoint != "token.get" {
        let token = mxm_token()?.ok_or_else(|| anyhow!("Musixmatch token unavailable"))?;
        url.query_pairs_mut().append_pair("usertoken", &token);
    }
    mxm_sign(&mut url)?;
    Ok(url)
}

fn mxm_request_with_refresh(url: reqwest::Url) -> Result<Option<String>> {
    let first = get(http()?
        .get(url.clone())
        .header(reqwest::header::USER_AGENT, MXM_AGENT));
    match first {
        Ok(Some(body)) if mxm_unauthorized(&body) => {}
        Err(error)
            if error.to_string().contains("HTTP 401") || error.to_string().contains("HTTP 402") => {
        }
        Ok(response) => return Ok(response),
        Err(error) => return Err(error),
    }
    invalidate_mxm_auth()?;
    let Some(token) = mxm_token()? else {
        return Ok(None);
    };
    let mut refreshed = url;
    let pairs = refreshed
        .query_pairs()
        .filter(|(key, _)| key != "signature" && key != "signature_protocol" && key != "usertoken")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect::<Vec<_>>();
    refreshed.query_pairs_mut().clear();
    for (key, value) in pairs {
        refreshed.query_pairs_mut().append_pair(&key, &value);
    }
    refreshed.query_pairs_mut().append_pair("usertoken", &token);
    mxm_sign(&mut refreshed)?;
    get(http()?
        .get(refreshed)
        .header(reqwest::header::USER_AGENT, MXM_AGENT))
}

static MXM_TOKEN_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<String>>> =
    std::sync::OnceLock::new();
static MXM_SECRET_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<String>>> =
    std::sync::OnceLock::new();

fn invalidate_mxm_auth() -> Result<()> {
    for cache in [MXM_TOKEN_CACHE.get(), MXM_SECRET_CACHE.get()]
        .into_iter()
        .flatten()
    {
        *cache
            .lock()
            .map_err(|_| anyhow!("Musixmatch authentication cache unavailable"))? = None;
    }
    Ok(())
}

fn mxm_sign(url: &mut reqwest::Url) -> Result<()> {
    let normalized = url.as_str().replace("%20", "+").replace(' ', "+");
    let signature = mxm_signature(&normalized, &mxm_secret(), &utc_yyyymmdd())?;
    url.query_pairs_mut()
        .append_pair("signature", &signature)
        .append_pair("signature_protocol", "sha256");
    Ok(())
}

fn mxm_signature(url: &str, secret: &str, date: &str) -> Result<String> {
    let mut mac = <hmac::Hmac<sha2::Sha256> as hmac::Mac>::new_from_slice(secret.as_bytes())
        .map_err(|_| anyhow!("Musixmatch signing unavailable"))?;
    mac.update(format!("{url}{date}").as_bytes());
    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        mac.finalize().into_bytes(),
    ))
}

fn mxm_unauthorized(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/message/header/status_code")
                .and_then(Value::as_i64)
        })
        .is_some_and(|code| code == 401 || code == 402)
}

fn parse_mxm_richsync(raw: &str) -> Vec<LyricLine> {
    let Ok(rows) = serde_json::from_str::<Vec<Value>>(raw) else {
        return vec![];
    };
    rows.iter()
        .filter_map(|row| {
            let start = row["ts"].as_f64()?;
            let end = row["te"].as_f64().unwrap_or(start).max(start);
            let fragments = row["l"].as_array().into_iter().flatten();
            let mut words: Vec<TimedWord> = Vec::new();
            let mut previous_space = false;
            for (index, fragment) in fragments.enumerate() {
                let text = fragment["c"].as_str()?.to_owned();
                let at = start + fragment["o"].as_f64()?;
                let next = row["l"]
                    .as_array()?
                    .get(index + 1)
                    .and_then(|v| v["o"].as_f64())
                    .map(|o| start + o)
                    .unwrap_or(end);
                let run_end = next.max(at).min(end);
                if text.trim().is_empty() {
                    if let Some(last) = words.last_mut() {
                        last.end = run_end;
                    }
                    previous_space = true;
                    continue;
                }
                if words.is_empty()
                    || previous_space
                    || text.chars().next().is_some_and(char::is_whitespace)
                {
                    words.push(TimedWord {
                        start: at,
                        end: run_end,
                        text: text.trim().to_owned(),
                    });
                } else if let Some(last) = words.last_mut() {
                    last.text.push_str(text.trim());
                    last.end = run_end;
                }
                previous_space = text.chars().last().is_some_and(char::is_whitespace);
            }
            let text = row["x"].as_str().unwrap_or("").trim().to_owned();
            let text = if text.is_empty() {
                words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                text
            };
            (!text.is_empty()).then_some(LyricLine {
                time: start,
                text,
                words,
            })
        })
        .collect()
}

fn parse_mxm_subtitles(raw: &str) -> Vec<LyricLine> {
    serde_json::from_str::<Vec<Value>>(raw)
        .unwrap_or_default()
        .iter()
        .filter_map(|row| {
            let text = row["text"].as_str()?.trim();
            let time = row.pointer("/time/total")?.as_f64()?;
            (!text.is_empty() && time.is_finite() && time >= 0.0).then(|| LyricLine {
                time,
                text: text.to_owned(),
                words: vec![],
            })
        })
        .collect()
}

fn utc_yyyymmdd() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400;
    civil_date(days as i64)
}

fn civil_date(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}")
}

fn paxsenix_apple(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    let page = get(http()?.get("https://music.apple.com/us/new"))?.unwrap_or_default();
    let Some(script) = page
        .split("/assets/index~")
        .nth(1)
        .and_then(|tail| tail.split('"').next())
    else {
        return Ok(None);
    };
    let script_url = format!("https://music.apple.com/assets/index~{script}");
    let bundle = get(http()?.get(script_url))?.unwrap_or_default();
    let Some(token) = bundle.match_indices("eyJ").find_map(|(i, _)| {
        let s = &bundle[i..];
        let token = s
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
            .next()?;
        (token.matches('.').count() == 2).then_some(token)
    }) else {
        return Ok(None);
    };
    let mut search = reqwest::Url::parse("https://amp-api.music.apple.com/v1/catalog/us/search")?;
    search
        .query_pairs_mut()
        .append_pair("term", &format!("{} {}", track.title, track.artist))
        .append_pair("types", "songs")
        .append_pair("limit", "10")
        .append_pair("l", "en-US");
    let Some(body) = get(http()?
        .get(search)
        .bearer_auth(token)
        .header("Origin", "https://music.apple.com")
        .header("Referer", "https://music.apple.com/"))?
    else {
        return Ok(None);
    };
    let root: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let songs = root
        .pointer("/results/songs/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    let title = normalize(&track.title);
    let artist = normalize(&track.artist);
    let best = songs
        .filter(|s| {
            let attrs = &s["attributes"];
            normalize(attrs["name"].as_str().unwrap_or("")) == title
                && normalize(attrs["artistName"].as_str().unwrap_or("")).contains(&artist)
                && (!track.duration.is_finite()
                    || track.duration <= 0.0
                    || attrs["durationInMillis"]
                        .as_f64()
                        .is_none_or(|d| (d / 1000.0 - track.duration).abs() <= 8.0))
        })
        .min_by(|a, b| {
            let distance = |item: &Value| {
                (item["attributes"]["durationInMillis"]
                    .as_f64()
                    .unwrap_or(track.duration)
                    / 1000.0
                    - track.duration)
                    .abs()
            };
            distance(a).total_cmp(&distance(b))
        });
    let Some(id) = best.and_then(|s| s["id"].as_str()) else {
        return Ok(None);
    };
    let mut url = reqwest::Url::parse("https://lyrics.paxsenix.org/apple-music/lyrics")?;
    url.query_pairs_mut()
        .append_pair("id", id)
        .append_pair("ttml", "true");
    let body = get(http()?.get(url))?;
    Ok(body
        .and_then(|raw| parse_provider(&raw))
        .filter(|l| usable(l)))
}

fn youtube_captions(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    if !valid_youtube_id(&track.id) {
        return Ok(None);
    }
    let bytes = [vec![10, track.id.len() as u8], track.id.as_bytes().to_vec()].concat();
    let params = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes);
    let page = super::innertube_value("get_transcript", serde_json::json!({"params":params}))?;
    let mut lines = Vec::new();
    for cue in named(&page, "transcriptCueRenderer") {
        if let (Some(ms), Some(text)) = (
            cue["startOffsetMs"]
                .as_str()
                .and_then(|s| s.parse::<u64>().ok())
                .or_else(|| cue["startOffsetMs"].as_u64()),
            Some(youtube_strings(&cue["cue"]).join(" ")),
        ) {
            let text = text.trim_matches([' ', '\n', '♪']);
            if !text.is_empty() {
                lines.push(LyricLine {
                    time: ms as f64 / 1000.0,
                    text: text.into(),
                    words: vec![],
                });
            }
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    Ok(usable(&lines).then_some(lines))
}

fn valid_youtube_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn kugou(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    let query = format!("{} - {}", track.title, track.artist);
    let song_url = reqwest::Url::parse_with_params(
        "https://mobileservice.kugou.com/api/v3/search/song",
        &[
            ("version", "9108"),
            ("plat", "0"),
            ("pagesize", "8"),
            ("showtype", "0"),
            ("keyword", &query),
        ],
    )?;
    let Some(body) = get(http()?.get(song_url))? else {
        return Ok(None);
    };
    let root: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let mut songs = root
        .pointer("/data/info")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    songs.sort_by(|a, b| {
        (a["duration"].as_f64().unwrap_or(0.0) - track.duration)
            .abs()
            .total_cmp(&(b["duration"].as_f64().unwrap_or(0.0) - track.duration).abs())
    });
    for song in songs {
        if !kugou_song_matches(&song, track) {
            continue;
        }
        let duration = song["duration"].as_f64().unwrap_or(0.0);
        if track.duration > 0.0 && duration > 0.0 && (duration - track.duration).abs() > 8.0 {
            continue;
        }
        let hash = song["hash"].as_str().unwrap_or_default();
        if hash.is_empty() {
            continue;
        }
        let search_url = reqwest::Url::parse_with_params(
            "https://lyrics.kugou.com/search",
            &[
                ("ver", "1"),
                ("man", "yes"),
                ("client", "pc"),
                ("hash", hash),
            ],
        )?;
        let Some(body) = get(http()?.get(search_url))? else {
            continue;
        };
        let response: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        for candidate in response["candidates"].as_array().into_iter().flatten() {
            let (id, key) = (
                candidate["id"].as_str().unwrap_or(""),
                candidate["accesskey"].as_str().unwrap_or(""),
            );
            if id.is_empty() || key.is_empty() {
                continue;
            }
            let url = reqwest::Url::parse_with_params(
                "https://lyrics.kugou.com/download",
                &[
                    ("fmt", "lrc"),
                    ("charset", "utf8"),
                    ("client", "pc"),
                    ("ver", "1"),
                    ("id", id),
                    ("accesskey", key),
                ],
            )?;
            let Some(body) = get(http()?.get(url))? else {
                continue;
            };
            let response: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            let Some(encoded) = response["content"].as_str() else {
                continue;
            };
            let Ok(bytes) =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
            else {
                continue;
            };
            let lrc = String::from_utf8_lossy(&bytes);
            let lines = parse_lrc(&lrc);
            if usable(&lines) {
                return Ok(Some(lines));
            }
        }
    }
    Ok(None)
}

fn kugou_song_matches(song: &Value, track: &Track) -> bool {
    let title = normalize(
        song["songname"]
            .as_str()
            .or_else(|| song["song_name"].as_str())
            .unwrap_or_default(),
    );
    let artist = normalize(
        song["singername"]
            .as_str()
            .or_else(|| song["singer_name"].as_str())
            .unwrap_or_default(),
    );
    let wanted_title = normalize(&track.title);
    let wanted_artist = normalize(&track.artist);
    !wanted_title.is_empty()
        && (title == wanted_title || title.contains(&wanted_title) || wanted_title.contains(&title))
        && (wanted_artist.is_empty() || artist.contains(&wanted_artist))
}

fn genius(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    let query = format!("{} {}", track.artist, track.title);
    let url = reqwest::Url::parse_with_params(
        "https://genius.com/api/search/multi",
        &[("q", query.as_str())],
    )?;
    let Some(body) = get(http()?
        .get(url)
        .header(reqwest::header::USER_AGENT, "YTDugem"))?
    else {
        return Ok(None);
    };
    let root: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let hits = root
        .pointer("/response/sections")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|section| section["type"] == "song")
        .flat_map(|s| s["hits"].as_array().into_iter().flatten())
        .collect::<Vec<_>>();
    let wanted_title = normalize(&track.title);
    let wanted_artist = normalize(&track.artist);
    let candidate = hits
        .into_iter()
        .filter_map(|hit| hit.get("result"))
        .find(|item| {
            let title = normalize(item["title"].as_str().unwrap_or(""));
            let artist = normalize(item["artist_names"].as_str().unwrap_or(""));
            !wanted_title.is_empty()
                && title == wanted_title
                && (wanted_artist.is_empty() || artist.contains(&wanted_artist))
        });
    let Some(page) = candidate.and_then(|c| c["url"].as_str()) else {
        return Ok(None);
    };
    let parsed = reqwest::Url::parse(page)?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("genius.com") {
        return Ok(None);
    }
    let Some(html) = get(http()?
        .get(parsed)
        .header(reqwest::header::USER_AGENT, "YTDugem"))?
    else {
        return Ok(None);
    };
    let lines = genius_html_lines(&html);
    Ok(usable(&lines).then_some(lines))
}

fn genius_html_lines(html: &str) -> Vec<LyricLine> {
    let Some(marker) = html.find("data-lyrics-container") else {
        return vec![];
    };
    let Some(mut pos) = html[marker..].find('>').map(|n| marker + n + 1) else {
        return vec![];
    };
    let mut depth = 1usize;
    let mut text = String::new();
    while pos < html.len() {
        let Some(open) = html[pos..].find('<').map(|n| pos + n) else {
            text.push_str(&html[pos..]);
            break;
        };
        text.push_str(&html[pos..open]);
        let Some(close) = html[open..].find('>').map(|n| open + n) else {
            break;
        };
        let tag = html[open + 1..close].trim();
        if tag.starts_with("/div") {
            depth -= 1;
            if depth == 0 {
                break;
            }
            text.push('\n');
        } else if tag.starts_with("div") && !tag.ends_with('/') {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            depth += 1;
        } else if tag.starts_with("br") || tag.starts_with("/p") {
            text.push('\n');
        }
        pos = close + 1;
    }
    text.replace("&amp;", "&")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|text| LyricLine {
            time: 0.0,
            text: text.to_owned(),
            words: vec![],
        })
        .collect()
}

fn lrclib(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    let client = http()?;
    let exact_url = reqwest::Url::parse("https://lrclib.net/api/get")?;
    let duration = seconds(track);
    let exact = get(client.get(exact_url).query(&[
        ("track_name", track.title.as_str()),
        ("artist_name", track.artist.as_str()),
        ("duration", duration.as_str()),
    ]))?;
    if let Some(body) = exact {
        if let Some(lines) =
            parse_lrclib_item(&serde_json::from_str::<Value>(&body).unwrap_or(Value::Null))
        {
            return Ok(Some(lines));
        }
    }
    let search_url = reqwest::Url::parse("https://lrclib.net/api/search")?;
    let response = get(client.get(search_url).query(&[
        ("track_name", track.title.as_str()),
        ("artist_name", track.artist.as_str()),
    ]))?;
    let Some(response) = response else {
        return Ok(None);
    };
    let results: Vec<Value> = serde_json::from_str(&response).unwrap_or_default();
    let item = results
        .iter()
        .filter(|item| valid_lrclib_candidate(item, track))
        .min_by(|a, b| duration_distance(a, track).total_cmp(&duration_distance(b, track)));
    Ok(item.and_then(parse_lrclib_item))
}

fn valid_lrclib_candidate(item: &Value, track: &Track) -> bool {
    let title = item["trackName"].as_str().unwrap_or_default();
    let artist = item["artistName"].as_str().unwrap_or_default();
    if normalize(title) != normalize(&track.title) || normalize(artist) != normalize(&track.artist)
    {
        return false;
    }
    let Some(text) = item["syncedLyrics"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            item["plainLyrics"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
        })
    else {
        return false;
    };
    if !usable(&parse_lrc(text)) {
        return false;
    }
    let duration = item["duration"].as_f64().unwrap_or(0.0);
    track.duration <= 0.0 || duration <= 0.0 || (duration - track.duration).abs() <= 10.0
}

fn duration_distance(item: &Value, track: &Track) -> f64 {
    (item["duration"].as_f64().unwrap_or(track.duration) - track.duration).abs()
}

fn parse_lrclib_item(item: &Value) -> Option<Vec<LyricLine>> {
    let synced = item["syncedLyrics"]
        .as_str()
        .filter(|s| !s.trim().is_empty());
    let plain = item["plainLyrics"]
        .as_str()
        .filter(|s| !s.trim().is_empty());
    let lines = synced
        .map(parse_lrc)
        .filter(|lines| usable(lines))
        .or_else(|| plain.map(parse_lrc).filter(|lines| usable(lines)))?;
    Some(lines)
}

fn youtube_music(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    if track.id.len() != 11
        || !track
            .id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Ok(None);
    }
    let next = super::innertube_value(
        "next",
        serde_json::json!({"videoId":track.id,"playlistId":format!("RDAMVM{}",track.id),"isAudioOnly":true}),
    )?;
    let Some((id, params)) = youtube_lyrics_endpoint(&next) else {
        return Ok(None);
    };
    let mut body = serde_json::json!({"browseId":id});
    if let Some(params) = params {
        body["params"] = params.into();
    }
    let page = super::innertube_value("browse", body)?;
    Ok(parse_youtube_description(&page))
}

fn youtube_lyrics_endpoint(next: &Value) -> Option<(String, Option<String>)> {
    let tab = named(next, "tabRenderer")
        .into_iter()
        .find(|tab| {
            youtube_strings(tab)
                .iter()
                .any(|t| t.eq_ignore_ascii_case("Lyrics"))
        })
        .and_then(|tab| named(tab, "browseEndpoint").into_iter().next())
        .or_else(|| {
            named(next, "tabRenderer")
                .into_iter()
                .skip(1)
                .find_map(|tab| named(tab, "browseEndpoint").into_iter().next())
        });
    let endpoint = tab?;
    let id = endpoint["browseId"].as_str()?.to_owned();
    Some((id, endpoint["params"].as_str().map(str::to_owned)))
}

fn parse_youtube_description(page: &Value) -> Option<Vec<LyricLine>> {
    let shelf = named(page, "musicDescriptionShelfRenderer")
        .into_iter()
        .next()?;
    let text = youtube_strings(&shelf["description"]).join("");
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| LyricLine {
            time: 0.0,
            text: s.to_owned(),
            words: vec![],
        })
        .collect::<Vec<_>>();
    usable(&lines).then_some(lines)
}

fn unison(track: &Track) -> Result<Option<Vec<LyricLine>>> {
    let url = reqwest::Url::parse("https://unison.boidu.dev/lyrics")?;
    let duration = seconds(track);
    let body = get(http()?.get(url).query(&[
        ("song", track.title.as_str()),
        ("artist", track.artist.as_str()),
        ("album", track.album.as_str()),
        ("duration", duration.as_str()),
    ]))?;
    let Some(body) = body else { return Ok(None) };
    let response: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    if response["success"] != true {
        return Ok(None);
    }
    let lyrics = response
        .pointer("/data/lyrics")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let lines = parse_provider(lyrics).unwrap_or_default();
    Ok(usable(&lines).then_some(lines))
}

fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in text.lines() {
        let mut rest = raw;
        let mut stamps = Vec::new();
        while let Some(after) = rest.strip_prefix('[') {
            let Some((stamp, tail)) = after.split_once(']') else {
                break;
            };
            let Some((m, s)) = stamp.split_once(':') else {
                break;
            };
            let (seconds, fraction) = s.split_once(['.', ':']).unwrap_or((s, ""));
            let (Ok(m), Ok(s)) = (m.parse::<u64>(), seconds.parse::<u64>()) else {
                break;
            };
            if s >= 60 {
                break;
            }
            let fraction_ms = if fraction.is_empty() {
                0
            } else {
                if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
                    break;
                }
                let digits = fraction.len() as u32;
                let Ok(fraction) = fraction.parse::<u64>() else {
                    break;
                };
                fraction * 10_u64.pow(3 - digits)
            };
            let time = m as f64 * 60.0 + s as f64 + fraction_ms as f64 / 1000.0;
            if !time.is_finite() {
                break;
            }
            stamps.push(time);
            rest = tail;
        }
        for time in stamps {
            let text = rest.trim().to_owned();
            if !text.is_empty() {
                lines.push(LyricLine {
                    time,
                    text,
                    words: vec![],
                });
            }
        }
    }
    if lines.is_empty() {
        lines = text
            .lines()
            .map(str::trim)
            .filter(|s| {
                !(s.is_empty() || is_lrc_metadata(s) || s.starts_with('[') && s.contains(']'))
            })
            .map(|s| LyricLine {
                time: 0.0,
                text: s.to_owned(),
                words: vec![],
            })
            .collect();
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    lines
}

fn is_lrc_metadata(line: &str) -> bool {
    line.strip_prefix('[')
        .and_then(|s| s.split_once(':'))
        .is_some_and(|(tag, _)| !tag.contains(']') && tag.chars().all(|c| c.is_ascii_alphabetic()))
}

fn parse_provider(raw: &str) -> Option<Vec<LyricLine>> {
    fn extract(value: &Value) -> Option<String> {
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Array(items) => {
                let joined = items
                    .iter()
                    .filter_map(extract)
                    .collect::<Vec<_>>()
                    .join("\n");
                (!joined.is_empty()).then_some(joined)
            }
            Value::Object(map) => [
                "ttml",
                "ttmlContent",
                "lyrics",
                "lrc",
                "content",
                "text",
                "plainLyrics",
                "syncedLyrics",
                "line",
                "lines",
                "lyric",
                "data",
                "result",
                "response",
            ]
            .iter()
            .find_map(|key| map.get(*key).and_then(extract)),
            _ => None,
        }
    }
    let raw = raw.trim_start_matches('\u{feff}').trim();
    if looks_like_error(raw) {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        if let Some(lines) = parse_structured_timed(&value).filter(|lines| usable(lines)) {
            return Some(lines);
        }
    }
    let content = match serde_json::from_str::<Value>(raw) {
        Ok(value) => {
            if value["isError"] == true || value["ok"] == false || !value["error"].is_null() {
                return None;
            }
            extract(&value)?
        }
        Err(_) => raw.to_owned(),
    };
    let content = if content.to_ascii_lowercase().contains("&lt;tt") {
        decode_xml(&content)
    } else {
        content
    };
    if content.trim_start().starts_with('<') && !content.to_ascii_lowercase().contains("<tt") {
        return None;
    }
    if content.to_ascii_lowercase().contains("<!doctype html")
        || content.to_ascii_lowercase().contains("<html")
    {
        return None;
    }
    let lines = if content.to_ascii_lowercase().contains("<tt") {
        parse_ttml(&content)?
    } else {
        let enhanced = parse_enhanced_lrc(&content);
        if enhanced.iter().any(|line| !line.words.is_empty()) {
            enhanced
        } else {
            parse_lrc(&content)
        }
    };
    usable(&lines).then_some(lines)
}

fn looks_like_error(text: &str) -> bool {
    let text = text.trim().to_ascii_lowercase();
    text.len() < 300
        && [
            "not found",
            "404 not",
            "internal server error",
            "service unavailable",
            "access denied",
            "too many requests",
            "rate limit",
            "lyrics unavailable",
            "lyrics not found",
            "error occurred",
            "bad gateway",
        ]
        .iter()
        .any(|phrase| text.contains(phrase))
}

fn parse_lyrics_plus(raw: &str) -> Option<Vec<LyricLine>> {
    let value: Value = serde_json::from_str(raw).ok()?;
    parse_structured_timed(&value)
        .filter(|lines| usable(lines))
        .or_else(|| parse_provider(raw))
}

// LyricsPlus emits millisecond `time`/`duration` rows and PaxSenix Apple
// responses emit `content` rows with millisecond timestamps and timed syllables.
fn parse_structured_timed(root: &Value) -> Option<Vec<LyricLine>> {
    fn find_rows(value: &Value) -> Option<&Vec<Value>> {
        if let Some(rows) = value.get("lyrics").and_then(Value::as_array) {
            if rows
                .iter()
                .any(|row| row.get("syllabus").is_some() || row.get("time").is_some())
            {
                return Some(rows);
            }
        }
        if let Some(rows) = value.get("content").and_then(Value::as_array) {
            if rows.iter().any(|row| row.get("timestamp").is_some()) {
                return Some(rows);
            }
        }
        value.as_object()?.values().find_map(find_rows)
    }
    let rows = find_rows(root)?;
    let lines = rows
        .iter()
        .filter_map(|row| {
            let start = row.get("time").or_else(|| row.get("timestamp"))?.as_i64()?;
            let fragments = row
                .get("syllabus")
                .and_then(Value::as_array)
                .or_else(|| row.get("text").and_then(Value::as_array));
            let words = fragments
                .map(|fragments| {
                    let runs = fragments
                        .iter()
                        .filter_map(|fragment| {
                            let text = fragment.get("text")?.as_str()?.to_owned();
                            let at = fragment
                                .get("time")
                                .or_else(|| fragment.get("timestamp"))?
                                .as_i64()?;
                            let end = fragment
                                .get("endtime")
                                .and_then(Value::as_i64)
                                .or_else(|| {
                                    fragment
                                        .get("duration")
                                        .and_then(Value::as_i64)
                                        .map(|d| at + d)
                                })
                                .unwrap_or(at);
                            Some((at, end, text, fragment.get("part").and_then(Value::as_bool)))
                        })
                        .collect::<Vec<_>>();
                    let mut words: Vec<TimedWord> = Vec::new();
                    let mut joins_previous = false;
                    for (at, end, fragment, joins_next) in runs {
                        let starts_word = words.is_empty()
                            || !joins_previous
                            || fragment.chars().next().is_some_and(char::is_whitespace);
                        if starts_word {
                            words.push(TimedWord {
                                start: at as f64 / 1000.0,
                                end: end.max(at) as f64 / 1000.0,
                                text: fragment.trim().to_owned(),
                            });
                        } else if let Some(last) = words.last_mut() {
                            last.text.push_str(fragment.trim());
                            last.end = end.max(at) as f64 / 1000.0;
                        } else {
                            words.push(TimedWord {
                                start: at as f64 / 1000.0,
                                end: end.max(at) as f64 / 1000.0,
                                text: fragment.trim().to_owned(),
                            });
                        }
                        joins_previous = joins_next.unwrap_or_else(|| {
                            !fragment.chars().last().is_some_and(char::is_whitespace)
                        });
                    }
                    words
                })
                .unwrap_or_default();
            let text = row
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    words
                        .iter()
                        .map(|w| w.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                });
            (!text.trim().is_empty()).then_some(LyricLine {
                time: start as f64 / 1000.0,
                text: text.trim().to_owned(),
                words,
            })
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then_some(lines)
}

fn parse_enhanced_lrc(raw: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw_line in raw.lines() {
        let (time, mut body) = if let Some(prefix) = raw_line.strip_prefix('[') {
            if let Some((stamp, tail)) = prefix.split_once(']') {
                let parsed = parse_lrc(&format!("[{stamp}]x"));
                (parsed.first().map_or(0.0, |line| line.time), tail)
            } else {
                (0.0, raw_line)
            }
        } else {
            (0.0, raw_line)
        };
        let mut words: Vec<TimedWord> = Vec::new();
        let mut text = String::new();
        while let Some(open) = body.find('<') {
            let after = &body[open + 1..];
            let Some((stamp, tail)) = after.split_once('>') else {
                break;
            };
            let Some(at) = parse_lrc_stamp(stamp) else {
                break;
            };
            let next = tail.find('<').unwrap_or(tail.len());
            let syllable = tail[..next].to_owned();
            text.push_str(&syllable);
            if !syllable.trim().is_empty() {
                let starts_word = syllable.chars().next().is_some_and(char::is_whitespace);
                let word_text = syllable.trim().to_owned();
                if starts_word || words.is_empty() {
                    if let Some(word) = words.last_mut() {
                        word.end = at;
                    }
                    words.push(TimedWord {
                        start: at,
                        end: at,
                        text: word_text,
                    });
                } else if let Some(word) = words.last_mut() {
                    word.end = at;
                    word.text.push_str(&word_text);
                }
            } else if let Some(word) = words.last_mut() {
                // Enhanced LRC writers close the final word with a timed empty run.
                word.end = at;
            }
            body = &tail[next..];
        }
        if !text.trim().is_empty() {
            lines.push(LyricLine {
                time: if time > 0.0 {
                    time
                } else {
                    words.first().map_or(0.0, |word| word.start)
                },
                text: text.trim().to_owned(),
                words,
            });
        }
    }
    lines.sort_by(|a, b| a.time.total_cmp(&b.time));
    lines
}

fn parse_lrc_stamp(stamp: &str) -> Option<f64> {
    let (minutes, rest) = stamp.split_once(':')?;
    let (seconds, fraction) = rest.split_once(['.', ':']).unwrap_or((rest, ""));
    let (minutes, seconds) = (minutes.parse::<u64>().ok()?, seconds.parse::<u64>().ok()?);
    if seconds >= 60 {
        return None;
    }
    let fraction_ms = if fraction.is_empty() {
        0
    } else {
        if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        fraction.parse::<u64>().ok()? * 10_u64.pow(3 - fraction.len() as u32)
    };
    let time = minutes as f64 * 60.0 + seconds as f64 + fraction_ms as f64 / 1000.0;
    time.is_finite().then_some(time)
}

fn parse_ttml(raw: &str) -> Option<Vec<LyricLine>> {
    let mut out = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find("<p") {
        rest = &rest[start..];
        let close = rest.find('>')?;
        let attrs = &rest[2..close];
        let end = rest[close + 1..].find("</p>")? + close + 1;
        let body = &rest[close + 1..end];
        let line_start = attribute(attrs, "begin")
            .and_then(parse_clock)
            .unwrap_or(0.0);
        let words = ttml_words(body);
        let text = if words.is_empty() {
            decode_xml(strip_tags(body).trim())
        } else {
            words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_owned()
        };
        if !text.is_empty() {
            out.push(LyricLine {
                time: if line_start > 0.0 {
                    line_start
                } else {
                    words.first().map_or(0.0, |w| w.start)
                },
                text,
                words,
            });
        }
        rest = &rest[end + 4..];
    }
    out.sort_by(|a, b| a.time.total_cmp(&b.time));
    usable(&out).then_some(out)
}

fn ttml_words(body: &str) -> Vec<TimedWord> {
    // `gap`: whitespace seen between spans since the last leaf. Apple-style TTML puts the space
    // between words outside the spans (`<span>Hello</span> <span>world</span>`).
    fn leaves(raw: &str, out: &mut Vec<(String, f64, f64, bool)>, gap: &mut bool) {
        let mut rest = raw;
        while let Some(open) = rest.find("<span") {
            *gap |= strip_tags(&rest[..open]).chars().any(char::is_whitespace);
            rest = &rest[open..];
            let Some(gt) = rest.find('>') else { return };
            let attrs = &rest[5..gt];
            let inner_start = gt + 1;
            let mut depth = 1usize;
            let mut pos = inner_start;
            let mut close = None;
            let mut inner_end = None;
            while pos < rest.len() {
                let next_open = rest[pos..].find("<span").map(|n| pos + n);
                let next_close = rest[pos..].find("</span>").map(|n| pos + n);
                match (next_open, next_close) {
                    (_, Some(c)) if next_open.is_none_or(|o| c < o) => {
                        depth -= 1;
                        if depth == 0 {
                            close = Some(c);
                            inner_end = Some(c + 7);
                            break;
                        }
                        pos = c + 7;
                    }
                    (Some(o), _) => {
                        depth += 1;
                        pos = rest[o..].find('>').map_or(rest.len(), |n| o + n + 1);
                    }
                    _ => return,
                }
            }
            let (Some(close), Some(end)) = (close, inner_end) else {
                return;
            };
            let inner = &rest[inner_start..close];
            if inner.contains("<span") {
                leaves(inner, out, gap);
            } else if let Some(start) = attribute(attrs, "begin").and_then(parse_clock) {
                let end_time = attribute(attrs, "end")
                    .and_then(parse_clock)
                    .unwrap_or(start);
                let text = decode_xml(&strip_tags(inner));
                if !text.trim().is_empty() {
                    out.push((text, start, end_time.max(start), std::mem::take(gap)));
                }
            }
            rest = &rest[end..];
        }
    }

    let mut runs = Vec::new();
    leaves(body, &mut runs, &mut false);
    let mut words: Vec<TimedWord> = Vec::new();
    let mut previous_space = false;
    for (text, start, end, gap) in runs {
        let boundary = words.is_empty()
            || previous_space
            || gap
            || text.chars().next().is_some_and(char::is_whitespace);
        if boundary {
            words.push(TimedWord {
                start,
                end,
                text: text.trim().to_owned(),
            });
        } else if let Some(word) = words.last_mut() {
            word.text.push_str(text.trim());
            word.end = end;
        }
        previous_space = text.chars().last().is_some_and(char::is_whitespace);
    }
    words
}

fn parse_clock(value: &str) -> Option<f64> {
    let value = value.trim();
    if value.starts_with('-') {
        return None;
    }
    if let Some(s) = value.strip_suffix("ms") {
        return s
            .parse::<f64>()
            .ok()
            .map(|v| v / 1000.0)
            .filter(|v| v.is_finite() && *v >= 0.0);
    }
    let value = value.strip_suffix('s').unwrap_or(value);
    let parsed = if let Some((h, rest)) = value.split_once(':') {
        if let Some((m, s)) = rest.split_once(':') {
            let (h, m, s) = (
                h.parse::<f64>().ok()?,
                m.parse::<f64>().ok()?,
                s.parse::<f64>().ok()?,
            );
            ((0.0..60.0).contains(&m) && (0.0..60.0).contains(&s))
                .then_some(h * 3600.0 + m * 60.0 + s)
        } else {
            // `m:ss.fff`, as lrc.red writes every time past the first minute ("1:05.231").
            // Rejecting it gave those lines time 0 and sorted the chorus above the opening.
            let (m, s) = (h.parse::<f64>().ok()?, rest.parse::<f64>().ok()?);
            (0.0..60.0).contains(&s).then_some(m * 60.0 + s)
        }
    } else {
        value.parse().ok()
    }?;
    (parsed.is_finite() && parsed >= 0.0).then_some(parsed)
}

fn attribute<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    attrs
        .split_once(&key)?
        .1
        .split_once('"')
        .map(|(value, _)| value)
}
fn strip_tags(value: &str) -> String {
    let mut result = String::new();
    let mut inside = false;
    for c in value.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => result.push(c),
            _ => (),
        }
    }
    result
}
fn decode_xml(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}
fn usable(lines: &[LyricLine]) -> bool {
    lines.iter().any(|line| {
        let text = line.text.trim();
        !text.is_empty()
            && ![
                "instrumental",
                "♪",
                "no lyrics available",
                "lyrics unavailable",
            ]
            .iter()
            .any(|placeholder| text.eq_ignore_ascii_case(placeholder))
    })
}
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn named<'a>(value: &'a Value, name: &str) -> Vec<&'a Value> {
    fn walk<'a>(v: &'a Value, name: &str, out: &mut Vec<&'a Value>) {
        match v {
            Value::Object(map) => {
                for (key, child) in map {
                    if key == name {
                        out.push(child);
                    }
                    walk(child, name, out);
                }
            }
            Value::Array(a) => {
                for child in a {
                    walk(child, name, out);
                }
            }
            _ => (),
        }
    }
    let mut out = Vec::new();
    walk(value, name, &mut out);
    out
}
fn youtube_strings(v: &Value) -> Vec<String> {
    match v {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().flat_map(youtube_strings).collect(),
        Value::Object(m) => {
            if let Some(s) = m
                .get("text")
                .and_then(Value::as_str)
                .or_else(|| m.get("simpleText").and_then(Value::as_str))
            {
                vec![s.to_owned()]
            } else {
                m.values().flat_map(youtube_strings).collect()
            }
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc_and_rejects_empty_or_wrong_metadata_candidates() {
        assert!(valid_isrc("USRC17607839"));
        assert!(!valid_isrc("USRC1760783X"));
        let lines = parse_lrc("[00:01.20]Hello\n[00:03.50]world");
        assert_eq!(lines[0].time, 1.2);
        assert_eq!(lines[1].text, "world");
        assert!(!valid_lrclib_candidate(
            &serde_json::json!({"trackName":"Wrong","artistName":"Artist","syncedLyrics":"[00:01.00]x","duration":100}),
            &Track {
                title: "Song".into(),
                artist: "Artist".into(),
                duration: 100.0,
                ..Default::default()
            }
        ));
        assert!(!usable(&parse_lrc("[00:01.00]")));
        assert!(!usable(&parse_lrc("Instrumental")));
        assert_eq!(parse_lrc("[00:01.005]fraction")[0].time, 1.005);
        assert_eq!(parse_lrc("[00:01.5]fraction")[0].time, 1.5);
        assert_eq!(parse_lrc("[00:01]fraction")[0].time, 1.0);
        assert!(parse_lrc_stamp("999999999999999999999999:59.999").is_none());
        assert_eq!(parse_clock("-1s"), None);
        assert_eq!(parse_clock("NaNs"), None);
        assert_eq!(parse_clock("inf"), None);
        assert_eq!(parse_clock("100ms"), Some(0.1));
        assert!(kugou_song_matches(
            &serde_json::json!({"songname":"Song","singername":"Artist"}),
            &Track {
                title: "Song".into(),
                artist: "Artist".into(),
                ..Default::default()
            }
        ));
        assert!(!kugou_song_matches(
            &serde_json::json!({"songname":"Song","singername":"Wrong"}),
            &Track {
                title: "Song".into(),
                artist: "Artist".into(),
                ..Default::default()
            }
        ));
        assert!(!successful_or_miss(reqwest::StatusCode::SERVICE_UNAVAILABLE).is_ok());
        assert!(!successful_or_miss(reqwest::StatusCode::TOO_MANY_REQUESTS).is_ok());
        assert_eq!(
            successful_or_miss(reqwest::StatusCode::NOT_FOUND).unwrap(),
            false
        );
    }

    #[test]
    fn parses_betterlyrics_ttml_word_timestamps() {
        let result = parse_ttml(r#"<tt><body><div><p begin="1.0s"><span begin="1.0s" end="1.3s">Hi </span><span begin="1.3s" end="1.8s">there</span></p></div></body></tt>"#).unwrap();
        assert_eq!(result[0].text, "Hi there");
        assert_eq!(result[0].words.len(), 2);
        assert_eq!(result[0].words[1].start, 1.3);
        assert!(parse_ttml(r#"<tt><body><p begin="NaNs">bad</p></body></tt>"#).is_some());
        assert_eq!(parse_clock("-00:00:01.0"), None);
        assert_eq!(parse_clock("1:05.231"), Some(65.231));
        assert_eq!(parse_clock("1:65"), None);
        // lrc.red: plain seconds for the first minute, `m:ss.fff` after it; order must hold.
        let lines = parse_ttml(r#"<tt><body><p begin="58.783"><span begin="58.783" end="59.1">opening</span></p><p begin="1:05.231"><span begin="1:05.231" end="1:06">chorus</span></p></body></tt>"#).unwrap();
        assert_eq!(lines[0].text, "opening");
        assert_eq!(lines[1].time, 65.231);
        assert_eq!(lines[1].words[0].start, 65.231);
    }

    #[test]
    fn unwraps_json_wrapped_ttml_as_provider_does() {
        let wrapped = serde_json::json!({"data":{"lyrics":"&lt;tt&gt;&lt;body&gt;&lt;p begin=\"1s\"&gt;&lt;span begin=\"1s\" end=\"2s\"&gt;word&lt;/span&gt;&lt;/p&gt;&lt;/body&gt;&lt;/tt&gt;"}}).to_string();
        let lines = parse_provider(&wrapped).unwrap();
        assert_eq!(lines[0].words[0].text, "word");
        let enhanced =
            parse_provider("[00:01.00]<00:01.00>Hello<00:01.50> there<00:02.00>").unwrap();
        assert_eq!(enhanced[0].text, "Hello there");
        assert_eq!(enhanced[0].words[0].end, 1.5);
        assert_eq!(enhanced[0].words[1].end, 2.0);
    }

    #[test]
    fn reads_the_lyrics_tab_endpoint_and_description_shelf() {
        let next = serde_json::json!({"tabs":[
            {"tabRenderer":{"title":"Overview","endpoint":{"browseEndpoint":{"browseId":"overview"}}}},
            {"tabRenderer":{"title":"Lyrics","endpoint":{"browseEndpoint":{"browseId":"lyrics-id","params":"lyrics-params"}}}}
        ]});
        assert_eq!(
            youtube_lyrics_endpoint(&next),
            Some(("lyrics-id".into(), Some("lyrics-params".into())))
        );
        let page = serde_json::json!({"contents":{"musicDescriptionShelfRenderer":{"description":{"runs":[{"text":"First line\n"},{"text":"Second line"}]}}}});
        let lines = parse_youtube_description(&page).unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["First line", "Second line"]
        );
        assert!(parse_youtube_description(&serde_json::json!({"contents":{}})).is_none());
    }

    #[test]
    fn parses_provider_response_fixtures_and_genius_nested_html() {
        let wrapped = r#"{"data":{"lyrics":"[00:01.00]First\n[00:02.00]Second"}}"#;
        let lines = parse_provider(wrapped).unwrap();
        assert_eq!(
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["First", "Second"]
        );
        let html = r#"<div data-lyrics-container="true"><div><span>One</span><br/>Two</div><div>Three</div></div>"#;
        let lines = genius_html_lines(html);
        assert_eq!(
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["One", "Two", "Three"]
        );
        assert!(genius_html_lines("<html>no lyrics</html>").is_empty());
    }

    #[test]
    fn parses_upstream_structured_timing_and_musixmatch_payloads() {
        let lyrics_plus = serde_json::json!({"type":"Syllable","lyrics":[{
            "time":1000,"duration":1800,"text":"Hello world",
            "syllabus":[{"time":1000,"duration":400,"text":"Hel"},{"time":1400,"duration":300,"text":"lo "},{"time":1700,"duration":500,"text":"world"}]
        }]});
        let lines = parse_provider(&lyrics_plus.to_string()).unwrap();
        assert_eq!(
            lines[0]
                .words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            ["Hello", "world"]
        );
        assert_eq!(lines[0].words[0].start, 1.0);
        let paxsenix = serde_json::json!({"data":{"content":[{"timestamp":2000,"text":[
            {"timestamp":2000,"endtime":2300,"text":"Good","part":false},
            {"timestamp":2400,"endtime":2800,"text":"morning","part":false}
        ]}]}});
        let lines = parse_provider(&paxsenix.to_string()).unwrap();
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[0].words[1].start, 2.4);

        let rich = parse_mxm_richsync(
            r#"[{"ts":1.0,"te":2.2,"x":"Hello world","l":[{"c":"Hello ","o":0.0},{"c":"world","o":0.7}]}]"#,
        );
        assert_eq!(
            rich[0]
                .words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>(),
            ["Hello", "world"]
        );
        assert_eq!(rich[0].words[1].start, 1.7);
        let subtitle = parse_mxm_subtitles(r#"[{"text":"fallback","time":{"total":3.25}}]"#);
        assert_eq!(subtitle[0].time, 3.25);
        assert!(!usable(
            &parse_provider("Internal Server Error").unwrap_or_default()
        ));
    }

    #[test]
    fn paxsenix_uses_spotify_search_id_and_best_recording_match() {
        let track = Track {
            title: "Song".into(),
            artist: "Artist".into(),
            duration: 180.0,
            ..Default::default()
        };
        let search = paxsenix_search_url("spotify/search", &track.title, &track.artist).unwrap();
        assert_eq!(search.path(), "/spotify/search");
        assert_eq!(
            search.query_pairs().find(|(key, _)| key == "q").unwrap().1,
            "Song Artist"
        );

        let results = serde_json::json!({"tracks":[
            {"id":"wrong-duration","name":"Song","artistName":"Artist","duration":210000},
            {"id":"featured-match","name":"Song (feat. Guest)","artistName":"Artist","duration":181000},
            {"id":"remix","name":"Song Remix","artistName":"Artist","duration":180000}
        ]});
        let best = paxsenix_best_candidate(&results, &track).unwrap();
        assert_eq!(best.id, "featured-match");
        let lyrics = paxsenix_lyrics_url("lyrics/spotify", &best.id).unwrap();
        assert_eq!(lyrics.path(), "/lyrics/spotify");
        assert_eq!(
            lyrics.query_pairs().find(|(key, _)| key == "id").unwrap().1,
            "featured-match"
        );
        assert!(paxsenix_best_candidate(
            &serde_json::json!({"id":"bad","name":"Different","artistName":"Other"}),
            &track
        )
        .is_none());
    }

    #[test]
    fn paxsenix_generic_fallback_selects_one_matching_document() {
        let track = Track {
            title: "Song".into(),
            artist: "Artist".into(),
            duration: 180.0,
            ..Default::default()
        };
        let raw = serde_json::json!({"lyrics":[
            {"title":"Song","artist":"Artist","duration":180,"lyrics":"[00:01.00]Wanted recording"},
            {"title":"Song","artist":"Other Artist","duration":180,"lyrics":"[00:06.00]Wrong recording"}
        ]}).to_string();
        let lines = parse_paxsenix_lrcget(&raw, &track).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Wanted recording");
        assert_eq!(lines[0].time, 1.0);

        let mismatch = serde_json::json!({"lyrics":[
            {"title":"Different","artist":"Other Artist","lyrics":"[00:01.00]Not this song"}
        ]})
        .to_string();
        assert!(parse_paxsenix_lrcget(&mismatch, &track).is_none());
    }

    #[test]
    fn paxsenix_key_errors_are_actionable_and_bearer_prefix_is_normalized() {
        let error = paxsenix_key(None, true).unwrap_err().to_string();
        assert!(error.contains("Configure a PaxSenix API key in Settings"));
        assert!(paxsenix_key(None, false).unwrap().is_none());
        assert_eq!(
            paxsenix_key(Some("  Bearer secret-value  ".into()), true)
                .unwrap()
                .as_deref(),
            Some("secret-value")
        );
        assert_eq!(
            paxsenix_key(Some("bEaReR secret-value".into()), true)
                .unwrap()
                .as_deref(),
            Some("secret-value")
        );
        assert!(!error.contains("secret"));
        assert!(!error.contains("https://"));
    }

    #[test]
    fn signature_date_and_bounded_body_regressions() {
        let signature = mxm_signature("", "Jefe", "what do ya want for nothing?").unwrap();
        let hex = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, signature)
            .unwrap()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(civil_date(0), "19700101");
        assert_eq!(civil_date(10_956), "19991231");
        assert_eq!(civil_date(11_016), "20000229");
        assert_eq!(civil_date(11_017), "20000301");
        assert!(read_bounded(std::io::Cursor::new(vec![0u8; 8])).is_ok());
        assert!(
            read_bounded(std::io::Cursor::new(vec![0u8; RESPONSE_LIMIT as usize + 1])).is_err()
        );
    }

    #[test]
    fn musixmatch_refresh_invalidates_both_rotating_credentials() {
        let token = MXM_TOKEN_CACHE.get_or_init(|| std::sync::Mutex::new(None));
        let secret = MXM_SECRET_CACHE.get_or_init(|| std::sync::Mutex::new(None));
        *token.lock().unwrap() = Some("expired-token".into());
        *secret.lock().unwrap() = Some("expired-signing-secret".into());
        invalidate_mxm_auth().unwrap();
        assert!(token.lock().unwrap().is_none());
        assert!(secret.lock().unwrap().is_none());
    }

    #[test]
    fn plain_lrc_after_metadata_and_bad_recording_match_are_handled() {
        let lines = parse_lrc("[ar:artist]\n[ti:song]\nplain lyric");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "plain lyric");
        let track = Track {
            title: "Wanted".into(),
            artist: "Correct Artist".into(),
            duration: 100.0,
            ..Default::default()
        };
        let bad = serde_json::json!({"songname":"Other Song","singername":"Correct Artist","duration":100});
        assert_ne!(
            normalize(bad["songname"].as_str().unwrap()),
            normalize(&track.title)
        );
    }

    #[test]
    fn reads_embedded_playlist_sidecar_before_network_providers() {
        let dir = std::env::temp_dir().join(format!("bitchord-embedded-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let playlist = dir.join("offline.m3u8");
        std::fs::write(&playlist, "#EXTM3U").unwrap();
        std::fs::write(dir.join("lyrics.lrc"), "[00:01.00]Local line").unwrap();
        let track = Track {
            source: playlist.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let result = lookup(&track, &[], &[], None).unwrap();
        assert_eq!(result.provider, "Downloaded");
        assert_eq!(result.lines[0].text, "Local line");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ordered_provider_settings_preserve_order_and_add_new_sources() {
        let ordered = ordered(&["lrclib".into(), "lrclib".into(), "unknown".into()]);
        assert_eq!(
            ordered.iter().map(|p| p.id).collect::<Vec<_>>(),
            [
                "lrc_red",
                "bini_lyrics",
                "betterlyrics",
                "betterlyrics_portato",
                "paxsenix",
                "paxsenix_spotify",
                "paxsenix_musixmatch",
                "lyrics_plus",
                "unison",
                "youtube_captions",
                "youtube_music",
                "kugou",
                "lrclib",
                "genius",
                "musixmatch"
            ]
        );
        assert_eq!(PROVIDERS.len(), 15);
        assert!(PROVIDERS
            .iter()
            .any(|p| p.id == "musixmatch" && p.key_id.is_none()));
        assert!(!PROVIDERS
            .iter()
            .any(|p| matches!(p.id, "simpmusic" | "megalobiz")));
    }

    #[test]
    fn added_sources_insert_after_their_default_predecessor() {
        let ordered = ordered(&["youtube_music".into(), "lrclib".into()]);
        let ids = ordered.iter().map(|p| p.id).collect::<Vec<_>>();
        assert!(
            ids.iter().position(|id| *id == "youtube_captions").unwrap()
                < ids.iter().position(|id| *id == "youtube_music").unwrap()
        );
        assert!(
            ids.iter().position(|id| *id == "kugou").unwrap()
                > ids.iter().position(|id| *id == "youtube_music").unwrap()
        );
    }

    #[test]
    fn ttml_keeps_nested_leaf_timing_and_explicit_whitespace() {
        let lines = parse_ttml(r#"<tt><body><p begin="1s"><span begin="1s" end="2s"><span begin="1s" end="1.4s">Hel</span><span begin="1.4s" end="1.7s">lo </span></span><span begin="2s" end="3s">world</span></p></body></tt>"#).unwrap();
        assert_eq!(lines[0].text, "Hello world");
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[0].words[0].start, 1.0);
        assert_eq!(lines[0].words[0].end, 1.7);
        // lrc.red/Apple style: the space between words sits between the spans.
        let lines = parse_ttml(r#"<tt><body><p begin="1s"><span begin="1s" end="1.5s">Terdengar</span> <span begin="1.5s" end="2s">lirih</span> <span begin="2s" end="2.4s"><span begin="2s" end="2.2s">bisik</span><span begin="2.2s" end="2.4s">anmu</span></span></p></body></tt>"#).unwrap();
        assert_eq!(lines[0].text, "Terdengar lirih bisikanmu");
        assert_eq!(lines[0].words.len(), 3);
        assert_eq!(lines[0].words[2].start, 2.0);
    }

    #[test]
    #[ignore = "requires public Musixmatch endpoints; no account or user credentials"]
    fn live_musixmatch_lookup() {
        let lines = musixmatch(&Track {
            title: "Get Lucky".into(),
            artist: "Daft Punk".into(),
            duration: 369.0,
            ..Default::default()
        })
        .unwrap()
        .expect("Musixmatch should return lyrics for this recording");
        assert!(usable(&lines));
        println!("Musixmatch returned {} lines", lines.len());
    }

    #[test]
    #[ignore = "requires public BetterLyrics, Unison and YouTube Music endpoints"]
    fn live_keyless_lyrics_provider_smoke() {
        let track = Track {
            id: "5NV6Rdv1a3I".into(),
            title: "Get Lucky".into(),
            artist: "Daft Punk".into(),
            album: "Random Access Memories".into(),
            duration: 249.0,
            ..Default::default()
        };
        let mut successes = 0;
        for id in ["betterlyrics", "unison", "youtube_music"] {
            match lookup(&track, &[], &[], Some(id)) {
                Ok(found) => {
                    assert!(!found.lines.is_empty());
                    println!("{}: {} lyric lines", found.provider, found.lines.len());
                    successes += 1;
                }
                Err(error) => println!("{id}: {error:#}"),
            }
        }
        assert!(
            successes > 0,
            "No live provider returned lyrics; inspect availability errors above"
        );
    }
}
