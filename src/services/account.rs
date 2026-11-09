//! Signed-in song ratings: like, dislike or clear a song, and the account's liked songs, so song
//! rows can show their rating (upstream desktop `DesktopSongMenu` like/dislike rows).

use super::{
    account_call, continuation, find_renderer, parse_collection, valid_video_id, LikeStatus,
    MAX_PAGES,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::HashSet;

/// The InnerTube endpoint and body that set a song's rating.
fn rating_request(video_id: &str, status: LikeStatus) -> Result<(&'static str, Value)> {
    if !valid_video_id(video_id) {
        bail!("Not a YouTube video ID: {video_id}");
    }
    let endpoint = match status {
        LikeStatus::Like => "like/like",
        LikeStatus::Dislike => "like/dislike",
        LikeStatus::Indifferent => "like/removelike",
    };
    Ok((endpoint, json!({"target": {"videoId": video_id}})))
}

/// Like, dislike or clear the rating of a song on the signed-in account.
pub fn rate_song(video_id: &str, status: LikeStatus) -> Result<()> {
    let (endpoint, body) = rating_request(video_id, status)?;
    account_call(endpoint, body).map(|_| ())
}

/// The video ids on one page of the Liked Music playlist, and the next page's token.
fn liked_page(v: &Value) -> (Vec<String>, Option<String>) {
    let ids = parse_collection(v)
        .tracks
        .into_iter()
        .map(|t| t.id)
        .filter(|id| valid_video_id(id))
        .collect();
    (ids, next_page(v))
}

/// Port of upstream `InnertubeParser.continuationToken`: the modern `continuationItemRenderer`
/// command first, then the older `nextContinuationData`; the shared lookup as a last resort.
fn next_page(v: &Value) -> Option<String> {
    find_renderer(v, "continuationItemRenderer")
        .and_then(|r| r.pointer("/continuationEndpoint/continuationCommand/token"))
        .or_else(|| find_renderer(v, "nextContinuationData").map(|d| &d["continuation"]))
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .or_else(|| continuation(v))
}

/// Songs the signed-in account liked (the `LM` playlist), up to `MAX_PAGES` pages. A failed
/// later page keeps what was already collected, like `library_playlists`.
pub fn liked_song_ids() -> Result<HashSet<String>> {
    let first = account_call("browse", json!({"browseId": "VLLM"}))?;
    let (ids, mut token) = liked_page(&first);
    let mut liked: HashSet<String> = ids.into_iter().collect();
    let mut page = 1;
    while let Some(t) = token.take() {
        if page >= MAX_PAGES {
            break;
        }
        page += 1;
        match account_call("browse", json!({"continuation": t})) {
            Ok(v) => {
                let (ids, next) = liked_page(&v);
                liked.extend(ids);
                token = next;
            }
            Err(_) => break,
        }
    }
    Ok(liked)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like a signed-in `browse` of `VLLM` and its continuation (made-up ids, no account).
    const LIKED_FIRST: &str = include_str!("fixtures/liked-music.json");
    const LIKED_MORE: &str = include_str!("fixtures/liked-music-continuation.json");
    const LIKED_LEGACY: &str = include_str!("fixtures/liked-music-legacy.json");

    #[test]
    fn rating_requests_pick_the_endpoint_and_target_the_video() {
        for (status, endpoint) in [
            (LikeStatus::Like, "like/like"),
            (LikeStatus::Dislike, "like/dislike"),
            (LikeStatus::Indifferent, "like/removelike"),
        ] {
            let (e, body) = rating_request("abcdefghijk", status).unwrap();
            assert_eq!(e, endpoint);
            assert_eq!(body, json!({"target": {"videoId": "abcdefghijk"}}));
        }
        assert!(rating_request("", LikeStatus::Like).is_err());
        assert!(rating_request("not a video id", LikeStatus::Like).is_err());
    }

    #[test]
    fn liked_music_pages_yield_ids_and_the_next_token() {
        let first: Value = serde_json::from_str(LIKED_FIRST).unwrap();
        let (ids, token) = liked_page(&first);
        assert_eq!(ids, ["aaaaaaaaaaa", "bbbbbbbbbbb"]);
        assert_eq!(token.as_deref(), Some("NEXT_PAGE_TOKEN"));

        let more: Value = serde_json::from_str(LIKED_MORE).unwrap();
        let (ids, token) = liked_page(&more);
        assert_eq!(ids, ["ccccccccccc"]);
        assert_eq!(token, None);

        let legacy: Value = serde_json::from_str(LIKED_LEGACY).unwrap();
        let (ids, token) = liked_page(&legacy);
        assert_eq!(ids, ["ddddddddddd"]);
        assert_eq!(token.as_deref(), Some("LEGACY_TOKEN"));

        assert_eq!(liked_page(&json!({})), (vec![], None));
    }
}
