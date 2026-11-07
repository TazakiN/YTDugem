use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
    pub thumbnail: String,
    pub source: String,
    /// The credited artists that YouTube links to a page, in credit order. Empty for local
    /// files and for tracks saved before artist links existed; the UI then looks names up.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artists: Vec<ArtistRef>,
}

/// One credited artist: the name as written in the credit line and its artist page id.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ArtistRef {
    pub name: String,
    pub browse_id: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimedWord {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

impl Track {
    pub fn key(&self) -> &str {
        if self.id.is_empty() {
            &self.source
        } else {
            &self.id
        }
    }
}

pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
