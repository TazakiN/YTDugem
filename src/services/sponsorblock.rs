//! SponsorBlock `music_offtopic` segments: the non-music parts (intros, outros, talking) of
//! music videos, skipped during playback. Only the first four hex digits of the video id's
//! SHA-256 are sent; the matching video is picked out of the response locally.

use anyhow::{bail, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::time::Duration;

const API: &str = "https://sponsor.ajay.app/api/skipSegments";
/// Longest position step between two polls that still counts as playing forward, not a seek.
const MAX_STEP: f64 = 3.0;
/// Shorter segments, or what is left of one, are not worth a seek.
const MIN_SKIP: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
}

/// The privacy-preserving lookup key: the first four hex digits of SHA-256(video id).
pub fn hash_prefix(video_id: &str) -> String {
    let digest = Sha256::digest(video_id.as_bytes());
    format!("{:02x}{:02x}", digest[0], digest[1])
}

/// The `music_offtopic` segments of `video_id`; an empty list when it has none.
pub fn fetch(video_id: &str) -> Result<Vec<Segment>> {
    if !super::valid_video_id(video_id) {
        bail!("Not a YouTube video ID: {video_id}");
    }
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("YTDugem")
        .build()?
        .get(format!("{API}/{}", hash_prefix(video_id)))
        .query(&[("categories", r#"["music_offtopic"]"#)])
        .send()?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(vec![]);
    }
    let body: Value = response.error_for_status()?.json()?;
    Ok(parse(&body, video_id))
}

/// The usable skip segments of `video_id` in a hashed-endpoint response, in start order.
pub fn parse(body: &Value, video_id: &str) -> Vec<Segment> {
    let mut out: Vec<Segment> = body
        .as_array()
        .into_iter()
        .flatten()
        .filter(|v| v["videoID"].as_str() == Some(video_id))
        .filter_map(|v| v["segments"].as_array())
        .flatten()
        .filter(|s| s["category"].as_str() == Some("music_offtopic"))
        .filter(|s| s["actionType"].as_str().is_none_or(|a| a == "skip"))
        .filter_map(|s| {
            let range = s["segment"].as_array()?;
            let (start, end) = (range.first()?.as_f64()?, range.get(1)?.as_f64()?);
            (start.is_finite() && end.is_finite() && start >= 0.0 && end - start >= MIN_SKIP)
                .then_some(Segment { start, end })
        })
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// The segment to skip now: playback moved forward naturally from `previous` to `position`
/// across a segment's start, and enough of it is left. A seek back into a segment is left
/// alone, and so is a segment already `skipped`.
pub fn segment_to_skip(
    previous: f64,
    position: f64,
    segments: &[Segment],
    skipped: &[bool],
) -> Option<usize> {
    let step = position - previous;
    if !(0.0..=MAX_STEP).contains(&step) {
        return None;
    }
    segments.iter().enumerate().position(|(i, s)| {
        !skipped.get(i).copied().unwrap_or(false)
            && previous <= s.start
            && s.start <= position
            && position < s.end - MIN_SKIP
    })
}

/// The segments of the playing track and the playback state needed to skip each once.
#[derive(Default)]
pub struct Skipper {
    key: String,
    segments: Vec<Segment>,
    skipped: Vec<bool>,
    last: f64,
}

impl Skipper {
    /// Forget everything, e.g. for a track that is not looked up.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    /// Wait for the segments of track `key`; nothing is skipped until they arrive.
    pub fn expect(&mut self, key: &str) {
        *self = Self {
            key: key.to_owned(),
            ..Self::default()
        };
    }
    pub fn is_for(&self, key: &str) -> bool {
        !self.key.is_empty() && self.key == key
    }
    /// The same track starts again (repeat one): every segment may be skipped again.
    pub fn rearm(&mut self) {
        self.skipped = vec![false; self.segments.len()];
        self.last = 0.0;
    }
    /// Segments arrived for `key`; ignored when another track is playing by now.
    pub fn set(&mut self, key: &str, segments: Vec<Segment>) {
        if self.is_for(key) {
            self.skipped = vec![false; segments.len()];
            self.segments = segments;
        }
    }
    /// The position to seek to when `key` just played into a segment, once per segment.
    pub fn check(&mut self, key: &str, position: f64) -> Option<f64> {
        if !self.is_for(key) {
            return None;
        }
        let previous = std::mem::replace(&mut self.last, position);
        let i = segment_to_skip(previous, position, &self.segments, &self.skipped)?;
        self.skipped[i] = true;
        self.last = self.segments[i].end;
        Some(self.segments[i].end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
        {"videoID": "dQw4w9WgXcQ", "segments": [
            {"category": "music_offtopic", "actionType": "skip", "segment": [180.5, 212.0],
             "UUID": "b", "videoDuration": 212.1},
            {"category": "music_offtopic", "actionType": "skip", "segment": [0, 14.25],
             "UUID": "a", "videoDuration": 212.1},
            {"category": "music_offtopic", "actionType": "skip", "segment": [50, 50.4]},
            {"category": "sponsor", "actionType": "skip", "segment": [60, 70]},
            {"category": "music_offtopic", "actionType": "mute", "segment": [80, 90]},
            {"category": "music_offtopic", "actionType": "skip", "segment": ["x", 90]}
        ]},
        {"videoID": "otherVideo1", "segments": [
            {"category": "music_offtopic", "actionType": "skip", "segment": [1, 30]}
        ]}
    ]"#;

    #[test]
    fn hash_prefix_is_first_four_hex_digits_of_sha256() {
        // sha256("dQw4w9WgXcQ") = 5f6b0b4e...
        assert_eq!(hash_prefix("dQw4w9WgXcQ"), "5f6b");
    }

    #[test]
    fn parses_only_the_requested_video_and_usable_skip_segments() {
        let body: Value = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(
            parse(&body, "dQw4w9WgXcQ"),
            vec![
                Segment {
                    start: 0.0,
                    end: 14.25
                },
                Segment {
                    start: 180.5,
                    end: 212.0
                },
            ]
        );
        assert_eq!(parse(&body, "otherVideo1").len(), 1);
        assert!(parse(&body, "notInResult").is_empty());
        assert!(parse(&serde_json::json!({"error": 1}), "dQw4w9WgXcQ").is_empty());
    }

    fn segments() -> Vec<Segment> {
        vec![
            Segment {
                start: 0.0,
                end: 14.0,
            },
            Segment {
                start: 100.0,
                end: 120.0,
            },
        ]
    }

    #[test]
    fn skips_when_playback_crosses_a_segment_start() {
        let s = segments();
        assert_eq!(segment_to_skip(0.0, 0.1, &s, &[false, false]), Some(0));
        assert_eq!(segment_to_skip(99.9, 100.1, &s, &[false, false]), Some(1));
        assert_eq!(segment_to_skip(50.0, 50.1, &s, &[false, false]), None);
    }

    #[test]
    fn leaves_seeks_skipped_segments_and_short_remainders_alone() {
        let s = segments();
        // A seek (big jump or backwards) into the segment.
        assert_eq!(segment_to_skip(20.0, 105.0, &s, &[false, false]), None);
        assert_eq!(segment_to_skip(130.0, 105.0, &s, &[false, false]), None);
        // Playing on inside the segment after seeking into it.
        assert_eq!(segment_to_skip(105.0, 105.1, &s, &[false, false]), None);
        // Already skipped once.
        assert_eq!(segment_to_skip(99.9, 100.1, &s, &[false, true]), None);
        // Less than a second of it left.
        let short = [Segment {
            start: 10.0,
            end: 11.5,
        }];
        assert_eq!(segment_to_skip(9.9, 10.6, &short, &[]), None);
    }

    #[test]
    fn skipper_skips_each_segment_once_for_its_track() {
        let mut k = Skipper::default();
        k.expect("a");
        assert_eq!(k.check("a", 0.1), None, "no segments yet");
        k.set("b", segments());
        assert_eq!(k.check("a", 0.2), None, "segments of another track");
        k.set("a", segments());
        k.rearm();
        assert_eq!(k.check("a", 0.1), Some(14.0));
        assert_eq!(k.check("a", 14.1), None);
        // The user seeks back to the start and plays into the intro again: once only.
        assert_eq!(k.check("a", 0.0), None);
        assert_eq!(k.check("a", 0.1), None);
        assert_eq!(k.check("b", 100.1), None);
        k.rearm();
        assert_eq!(k.check("a", 0.1), Some(14.0));
    }

    #[test]
    #[ignore = "contacts sponsor.ajay.app"]
    fn live_sponsorblock() {
        // "Despacito": a locked non-music intro and outro.
        let segments = fetch("kJQP7kiw5Fk").unwrap();
        println!("{segments:?}");
        assert!(!segments.is_empty());
        assert!(fetch("not a video").is_err());
    }
}
