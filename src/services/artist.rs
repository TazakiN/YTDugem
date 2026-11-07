//! Artist pages, artist search and the artist-name lookup that makes credits clickable.
//! Ports upstream `InnertubeParser.parseArtistPage`, `YtMusicRepository.findArtistPageId` and
//! the `main` branch's header shuffle/radio targets.

use super::{
    account_call, collect_cards, continuation, find_renderer, innertube_value, parse_renderers,
    text, thumbnails, ShelfItem,
};
use crate::model::Track;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::HashSet;

/// YouTube Music's "Artists" search filter (upstream `SearchFilter.ARTISTS`).
const ARTISTS_FILTER: &str = "EgWKAQIgAWoKEAkQChAFEAMQBA%3D%3D";
/// Continuation pages fetched for one "See all" grid, as upstream `MAX_PAGES`.
const MAX_PAGES: usize = 10;

/// A browse request: a browse id and the params that select the exact page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct BrowseTarget {
    pub browse_id: String,
    pub params: Option<String>,
}

/// A `watchEndpoint` the header's Shuffle and Mix buttons start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WatchTarget {
    pub video_id: Option<String>,
    pub playlist_id: String,
    pub params: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ArtistShelf {
    pub title: String,
    pub items: Vec<ShelfItem>,
    /// Where the shelf's "See all" leads.
    pub more: Option<BrowseTarget>,
}

#[derive(Clone, Debug, Default)]
pub struct ArtistPage {
    pub browse_id: String,
    pub name: String,
    /// Best header image; wide on an immersive header, square otherwise.
    pub thumbnail: String,
    /// The header image is a wide banner (immersive header).
    pub banner: bool,
    pub description: String,
    /// "5.43M subscribers"
    pub subscribers: Option<String>,
    /// "206M monthly audience"
    pub monthly: Option<String>,
    pub channel_id: Option<String>,
    /// The account's subscription; None when signed out or the header has no button.
    pub subscribed: Option<bool>,
    pub songs: Vec<Track>,
    pub songs_more: Option<BrowseTarget>,
    pub shelves: Vec<ArtistShelf>,
    pub shuffle: Option<WatchTarget>,
    pub radio: Option<WatchTarget>,
}

impl ArtistPage {
    /// The newest release among the album and single shelves, by the year in its subtitle.
    /// YouTube gives only a year, so among releases of the same year the first listed wins.
    pub fn latest_release(&self) -> Option<&ShelfItem> {
        let mut best: Option<(u32, &ShelfItem)> = None;
        for item in self.shelves.iter().flat_map(|s| &s.items) {
            if !item
                .browse_id
                .as_deref()
                .is_some_and(|id| id.starts_with("MPRE"))
            {
                continue;
            }
            let Some(year) = release_year(&item.subtitle) else {
                continue;
            };
            if best.is_none_or(|(y, _)| year > y) {
                best = Some((year, item));
            }
        }
        best.map(|(_, item)| item)
    }
}

fn release_year(subtitle: &str) -> Option<u32> {
    subtitle
        .split(|c: char| !c.is_ascii_digit())
        .filter(|w| w.len() == 4)
        .filter_map(|w| w.parse().ok())
        .filter(|y| (1900..=2100).contains(y))
        .max()
}

pub fn artist_page(browse_id: &str) -> Result<ArtistPage> {
    if !browse_id.starts_with("UC") {
        bail!("Not an artist page: {browse_id}");
    }
    let v = innertube_value("browse", json!({ "browseId": browse_id }))?;
    let mut page = parse_artist_page(&v);
    page.browse_id = browse_id.to_owned();
    if page.name.is_empty() && page.songs.is_empty() && page.shelves.is_empty() {
        bail!("YouTube Music returned an empty artist page");
    }
    Ok(page)
}

fn browse_target(endpoint: &Value) -> Option<BrowseTarget> {
    let browse_id = endpoint["browseId"].as_str().filter(|id| !id.is_empty())?;
    Some(BrowseTarget {
        browse_id: browse_id.to_owned(),
        params: endpoint["params"].as_str().map(str::to_owned),
    })
}

fn watch_target(button: &Value) -> Option<WatchTarget> {
    let w = button.pointer("/buttonRenderer/navigationEndpoint/watchEndpoint")?;
    Some(WatchTarget {
        video_id: w["videoId"].as_str().map(str::to_owned),
        playlist_id: w["playlistId"]
            .as_str()
            .filter(|p| !p.is_empty())?
            .to_owned(),
        params: w["params"].as_str().map(percent_decode),
    })
}

/// Undoes the `%3D`-style escapes YouTube leaves in endpoint params before they are resent.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The "See all" target of a carousel, in upstream's order of preference.
fn carousel_more(shelf: &Value) -> Option<BrowseTarget> {
    let header = &shelf["header"]["musicCarouselShelfBasicHeaderRenderer"];
    let from_runs = header["title"]["runs"].as_array().and_then(|runs| {
        runs.iter()
            .find_map(|r| r.pointer("/navigationEndpoint/browseEndpoint"))
    });
    let from_icons = header["endIcons"].as_array().and_then(|icons| {
        icons.iter().find_map(|i| {
            let b = &i["musicNavigationButtonRenderer"];
            b.pointer("/clickCommand/browseEndpoint")
                .or_else(|| b.pointer("/navigationEndpoint/browseEndpoint"))
        })
    });
    from_runs
        .or_else(|| header.pointer("/moreContentButton/buttonRenderer/navigationEndpoint/browseEndpoint"))
        .or_else(|| {
            header.pointer(
                "/moreContentButton/musicMoreContentButtonRenderer/navigationEndpoint/browseEndpoint",
            )
        })
        .or(from_icons)
        .or_else(|| header.pointer("/navigationEndpoint/browseEndpoint"))
        .and_then(browse_target)
}

fn parse_artist_page(v: &Value) -> ArtistPage {
    let immersive = v.pointer("/header/musicImmersiveHeaderRenderer");
    let header = immersive
        .or_else(|| v.pointer("/header/musicVisualHeaderRenderer"))
        .unwrap_or(&Value::Null);
    let thumbnail = [
        thumbnails(header),
        header
            .pointer("/foregroundThumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
            .and_then(Value::as_array)
            .and_then(|a| a.last())
            .and_then(|t| t["url"].as_str())
            .unwrap_or("")
            .to_owned(),
    ]
    .into_iter()
    .find(|t| !t.is_empty())
    .unwrap_or_default();
    let subscribe = header
        .pointer("/subscriptionButton/subscribeButtonRenderer")
        .or_else(|| header.pointer("/subscriptionButton2/subscribeButtonRenderer"));
    let non_empty = |s: String| (!s.trim().is_empty()).then(|| s.trim().to_owned());
    let subscribers = subscribe.and_then(|s| {
        non_empty(text(&s["longSubscriberCountText"])).or_else(|| {
            non_empty(text(&s["subscriberCountText"])).map(|n| format!("{n} subscribers"))
        })
    });
    let channel_id = subscribe
        .and_then(|s| s["channelId"].as_str())
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    // Signed out, YouTube still sends `subscribed: false`; only a session can tell.
    let subscribed = subscribe
        .filter(|_| crate::auth::snapshot().is_some())
        .and_then(|s| s["subscribed"].as_bool());
    let description = non_empty(text(&header["description"]))
        .or_else(|| {
            find_renderer(v, "musicDescriptionShelfRenderer")
                .and_then(|d| non_empty(text(&d["description"])))
        })
        .unwrap_or_default();

    let mut page = ArtistPage {
        name: text(&header["title"]).trim().to_owned(),
        thumbnail,
        banner: immersive.is_some(),
        description,
        subscribers,
        monthly: non_empty(text(&header["monthlyListenerCount"])),
        channel_id,
        subscribed,
        shuffle: watch_target(&header["playButton"]),
        radio: watch_target(&header["startRadioButton"]),
        ..Default::default()
    };
    let sections = v
        .pointer("/contents/singleColumnBrowseResultsRenderer/tabs/0/tabRenderer/content/sectionListRenderer/contents")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    for section in sections {
        if let Some(shelf) = section.get("musicShelfRenderer") {
            if page.songs.is_empty() {
                parse_renderers(&shelf["contents"], &mut page.songs);
                let mut seen = HashSet::new();
                page.songs.retain(|t| seen.insert(t.id.clone()));
                page.songs_more = shelf["title"]["runs"]
                    .as_array()
                    .and_then(|r| {
                        r.iter()
                            .find_map(|r| r.pointer("/navigationEndpoint/browseEndpoint"))
                    })
                    .or_else(|| shelf.pointer("/bottomEndpoint/browseEndpoint"))
                    .and_then(browse_target);
                for song in &mut page.songs {
                    if song.artist.is_empty() {
                        song.artist = page.name.clone();
                    }
                }
            }
        } else if let Some(shelf) = section.get("musicCarouselShelfRenderer") {
            let title = text(&shelf["header"]["musicCarouselShelfBasicHeaderRenderer"]["title"]);
            let mut items = vec![];
            collect_cards(&shelf["contents"], &mut items);
            items.retain(|i| i.video_id.is_some() || i.browse_id.is_some());
            if !items.is_empty() {
                page.shelves.push(ArtistShelf {
                    title: title.trim().to_owned(),
                    items,
                    more: carousel_more(shelf),
                });
            }
        }
    }
    page
}

/// Every card behind a shelf's "See all" (discography grids, video and playlist lists),
/// following continuations up to [`MAX_PAGES`].
pub fn shelf_items(target: &BrowseTarget) -> Result<Vec<ShelfItem>> {
    let mut body = json!({ "browseId": target.browse_id });
    if let Some(params) = &target.params {
        body["params"] = json!(percent_decode(params));
    }
    let mut v = innertube_value("browse", body)?;
    let mut items = vec![];
    for _ in 0..MAX_PAGES {
        collect_cards(&v, &mut items);
        let Some(token) = continuation(&v) else { break };
        v = innertube_value("browse", json!({ "continuation": token }))?;
    }
    let mut seen = HashSet::new();
    items.retain(|i| {
        let key = i.video_id.clone().or_else(|| i.browse_id.clone());
        key.is_some_and(|k| seen.insert(k))
    });
    Ok(items)
}

/// The queue a header's Shuffle or Mix button plays.
pub fn watch(target: &WatchTarget) -> Result<Vec<Track>> {
    let mut body = json!({ "playlistId": target.playlist_id, "isAudioOnly": true });
    if let Some(id) = &target.video_id {
        body["videoId"] = json!(id);
    }
    if let Some(params) = &target.params {
        body["params"] = json!(params);
    }
    let v = innertube_value("next", body)?;
    let mut tracks = vec![];
    super::collect_panel(&v, &mut tracks);
    let mut seen = HashSet::new();
    tracks.retain(|t| seen.insert(t.id.clone()));
    if tracks.is_empty() {
        bail!("YouTube Music returned no songs for this artist");
    }
    Ok(tracks)
}

/// Subscribe to or unsubscribe from an artist's channel on the signed-in account.
pub fn set_subscribed(channel_id: &str, subscribed: bool) -> Result<()> {
    if !channel_id.starts_with("UC") {
        bail!("Not a channel id: {channel_id}");
    }
    let endpoint = if subscribed {
        "subscription/subscribe"
    } else {
        "subscription/unsubscribe"
    };
    account_call(endpoint, json!({ "channelIds": [channel_id] })).map(|_| ())
}

/// Artists matching a search, best match first.
pub fn search_artists(query: &str) -> Result<Vec<ShelfItem>> {
    let query = query.trim();
    if query.is_empty() || query.len() > 500 {
        bail!("Enter a search between 1 and 500 characters");
    }
    let v = innertube_value(
        "search",
        json!({ "query": query, "params": ARTISTS_FILTER }),
    )?;
    Ok(parse_artist_results(&v))
}

fn parse_artist_results(v: &Value) -> Vec<ShelfItem> {
    let mut items = vec![];
    collect_cards(v, &mut items);
    let mut seen = HashSet::new();
    items.retain(|i| i.is_artist() && seen.insert(i.browse_id.clone()));
    items
}

/// The artist page of a credited name that carries no link (local imports, tracks saved before
/// links were kept): only an artist whose name matches exactly, ignoring case, spacing and
/// punctuation, so a lookup never opens a different artist.
pub fn find_artist_id(name: &str) -> Result<Option<String>> {
    let wanted = normalized(name);
    if wanted.is_empty() {
        return Ok(None);
    }
    Ok(search_artists(name)?
        .into_iter()
        .find(|i| normalized(&i.title) == wanted)
        .and_then(|i| i.browse_id))
}

fn normalized(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::credits;

    /// Trimmed from upstream BitChord `web/fixtures/innertube/browse-artist.json` (GPL-3.0).
    const ARTIST: &str = include_str!("fixtures/browse-artist.json");

    #[test]
    fn parses_artist_page_header_songs_and_shelves() {
        let v: Value = serde_json::from_str(ARTIST).unwrap();
        let page = parse_artist_page(&v);
        assert_eq!(page.name, "Radiohead");
        assert!(page.banner);
        assert!(page.thumbnail.starts_with("https://"));
        assert!(page
            .description
            .starts_with("Radiohead are an English rock band"));
        assert_eq!(page.subscribers.as_deref(), Some("5.43M subscribers"));
        assert_eq!(page.monthly.as_deref(), Some("206M monthly audience"));
        assert_eq!(page.channel_id.as_deref(), Some("UCq19-LqvG35A-30oyAiPiqA"));

        assert_eq!(page.songs[0].title, "Creep");
        assert_eq!(page.songs[0].artist, "Radiohead");
        assert_eq!(
            page.songs[0].artists[0].browse_id,
            "UCr_iyUANcn9OX_yy9piYoLw"
        );
        assert!(page
            .songs_more
            .as_ref()
            .unwrap()
            .browse_id
            .starts_with("VL"));

        let shuffle = page.shuffle.unwrap();
        assert_eq!(shuffle.playlist_id, "RDAOk8jEIzOyB2trfXZrSEVz_Q");
        assert_eq!(shuffle.params.as_deref(), Some("wAEB8gECGAE="));
        assert!(page.radio.unwrap().playlist_id.starts_with("RDEM"));

        let titles: Vec<_> = page.shelves.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(&titles[..3], ["Albums", "Singles & EPs", "Videos"]);
        assert!(titles.contains(&"Fans might also like"));
        let albums = &page.shelves[0];
        assert_eq!(
            albums.more.as_ref().unwrap().browse_id,
            "MPADUCr_iyUANcn9OX_yy9piYoLw"
        );
        assert_eq!(
            albums.items[0].page_type.as_deref(),
            Some("MUSIC_PAGE_TYPE_ALBUM")
        );
        let videos = &page.shelves[2];
        assert!(videos.items[0].wide && videos.items[0].video_id.is_some());
        assert_eq!(videos.items[0].artists[0].name, "Radiohead");
        let similar = page
            .shelves
            .iter()
            .find(|s| s.title == "Fans might also like")
            .unwrap();
        assert!(similar.items.iter().all(ShelfItem::is_artist));
        assert!(similar.more.is_none());
    }

    #[test]
    fn latest_release_takes_the_newest_year_and_the_first_on_ties() {
        let item = |id: &str, subtitle: &str| ShelfItem {
            browse_id: Some(id.into()),
            subtitle: subtitle.into(),
            ..Default::default()
        };
        let page = ArtistPage {
            shelves: vec![
                ArtistShelf {
                    items: vec![item("MPREa", "2021"), item("MPREb", "Album • 2023")],
                    ..Default::default()
                },
                ArtistShelf {
                    items: vec![
                        item("MPREc", "Single • 2023"),
                        item("VLplaylist", "Playlist • 2030"),
                    ],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            page.latest_release().unwrap().browse_id.as_deref(),
            Some("MPREb")
        );
        assert!(ArtistPage::default().latest_release().is_none());
    }

    #[test]
    fn credits_keep_the_artist_group_and_every_linked_artist() {
        let artist = |name: &str, id: &str| {
            json!({"text": name, "navigationEndpoint": {"browseEndpoint": {"browseId": id,
                "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig":
                {"pageType": "MUSIC_PAGE_TYPE_ARTIST"}}}}})
        };
        let album = json!({"text": "Album", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_x",
            "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig":
            {"pageType": "MUSIC_PAGE_TYPE_ALBUM"}}}}});
        let v = json!({"runs": [
            {"text": "Song"}, {"text": " • "},
            artist("A", "UCa"), {"text": ", "}, {"text": "Unlinked"}, {"text": " & "}, artist("B", "UCb"),
            {"text": " • "}, album, {"text": " • "}, {"text": "3:16"}
        ]});
        let (line, artists) = credits(&v);
        assert_eq!(line.as_deref(), Some("A, Unlinked & B"));
        let ids: Vec<_> = artists.iter().map(|a| a.browse_id.as_str()).collect();
        assert_eq!(ids, ["UCa", "UCb"]);
        assert_eq!(
            credits(&json!({"runs": [{"text": "2021"}]})),
            (None, vec![])
        );
    }

    #[test]
    fn artist_search_keeps_only_artist_rows() {
        let row = |title: &str, id: &str, page: &str| {
            json!({"musicResponsiveListItemRenderer": {
            "navigationEndpoint": {"browseEndpoint": {"browseId": id,
                "browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": page}}}},
            "flexColumns": [
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": title}]}}},
                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Artist"}, {"text": " • "}, {"text": "5M subscribers"}]}}}
            ]}})
        };
        let v = json!({"contents": [
            row("Radiohead", "UCr", "MUSIC_PAGE_TYPE_ARTIST"),
            row("OK Computer", "MPREb_x", "MUSIC_PAGE_TYPE_ALBUM"),
            row("Radiohead", "UCr", "MUSIC_PAGE_TYPE_ARTIST"),
        ]});
        let found = parse_artist_results(&v);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Radiohead");
        assert_eq!(found[0].subtitle, "Artist • 5M subscribers");
        assert_eq!(
            normalized("  The  Smashing-Pumpkins! "),
            normalized("the smashing pumpkins")
        );
    }

    #[test]
    fn decodes_endpoint_params() {
        assert_eq!(percent_decode("wAEB8gECGAE%3D"), "wAEB8gECGAE=");
        assert_eq!(percent_decode("a%2"), "a%2");
    }

    #[test]
    #[ignore = "needs network access"]
    fn live_artist_page_and_search() {
        let found = search_artists("Radiohead").unwrap();
        let id = found[0].browse_id.clone().unwrap();
        let page = artist_page(&id).unwrap();
        assert!(!page.name.is_empty());
        assert!(!page.songs.is_empty());
        for target in [&page.shuffle, &page.radio].into_iter().flatten() {
            assert!(!watch(target).unwrap().is_empty());
        }
        let albums = page.shelves.iter().find_map(|s| s.more.clone()).unwrap();
        assert!(!shelf_items(&albums).unwrap().is_empty());
        assert_eq!(find_artist_id("radiohead").unwrap(), Some(id));
    }
}
