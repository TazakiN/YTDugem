//! Playlist files: export a list of tracks to extended M3U (M3U8) or CSV, and read an M3U/M3U8
//! file back into tracks. A YTDugem addition; upstream BitChord desktop has no playlist files.
use crate::model::Track;
use anyhow::{bail, Context, Result};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

/// A playlist file larger than this is not a playlist anyone wrote by hand or by app.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// More entries than this are cut off (counted as skipped).
const MAX_ENTRIES: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    M3u8,
    Csv,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::M3u8 => "m3u8",
            Format::Csv => "csv",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::M3u8 => "M3U8",
            Format::Csv => "CSV",
        }
    }
}

/// The YouTube Music watch URL of an online track, or the file path of a local one.
fn location(track: &Track) -> String {
    if !track.id.is_empty() && track.source.starts_with("https://") {
        match youtube_id(&track.source) {
            Some(id) => format!("https://music.youtube.com/watch?v={id}"),
            None => track.source.clone(),
        }
    } else {
        track.source.clone()
    }
}

/// One line of text: line breaks in titles would start a new M3U entry or CSV row.
fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Extended M3U, UTF-8: `#EXTINF:<seconds>,<artist> - <title>` above each location.
pub fn write_m3u(tracks: &[Track]) -> String {
    let mut out = String::from("#EXTM3U\n");
    for t in tracks {
        let location = one_line(&location(t));
        if location.is_empty() {
            continue;
        }
        let seconds = if t.duration.is_finite() && t.duration > 0.0 {
            t.duration.round() as i64
        } else {
            -1
        };
        let title = one_line(&t.title);
        let artist = one_line(&t.artist);
        let name = if artist.is_empty() {
            title
        } else {
            format!("{artist} - {title}")
        };
        out.push_str(&format!("#EXTINF:{seconds},{name}\n{location}\n"));
    }
    out
}

/// A CSV field, quoted when it holds a comma, quote or line break (RFC 4180). A leading `=`,
/// `+`, `@` or tab gets a `'` so spreadsheet apps do not run a song title as a formula.
fn csv_field(s: &str) -> String {
    let mut s = s.to_owned();
    if s.starts_with(['=', '+', '@', '\t', '\r']) {
        s.insert(0, '\'');
    }
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s
    }
}

/// `title,artist,album,duration,url` with the duration in whole seconds (blank when unknown).
pub fn write_csv(tracks: &[Track]) -> String {
    let mut out = String::from("title,artist,album,duration,url\r\n");
    for t in tracks {
        let duration = if t.duration.is_finite() && t.duration > 0.0 {
            format!("{}", t.duration.round() as i64)
        } else {
            String::new()
        };
        let row = [
            csv_field(&t.title),
            csv_field(&t.artist),
            csv_field(&t.album),
            duration,
            csv_field(&location(t)),
        ];
        out.push_str(&row.join(","));
        out.push_str("\r\n");
    }
    out
}

/// The format a path's extension names; `.m3u` is written as UTF-8 extended M3U too.
pub fn format_of(path: &Path) -> Option<Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "m3u8" | "m3u" => Some(Format::M3u8),
        "csv" => Some(Format::Csv),
        _ => None,
    }
}

/// Write `tracks` to a new file at `path`. Never replaces an existing file.
pub fn export(path: &Path, tracks: &[Track]) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        bail!("Enter a file path to export to");
    }
    let Some(format) = format_of(path) else {
        bail!("The file name must end in .m3u8, .m3u or .csv");
    };
    let text = match format {
        Format::M3u8 => write_m3u(tracks),
        Format::Csv => write_csv(tracks),
    };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                anyhow::anyhow!("{} already exists; choose another name", path.display())
            } else {
                anyhow::Error::new(e).context(format!("Creating {}", path.display()))
            }
        })?;
    // The file is new and ours: remove it rather than leave half a playlist behind.
    if let Err(e) = file
        .write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(anyhow::Error::new(e).context(format!("Writing {}", path.display())));
    }
    Ok(path.to_path_buf())
}

/// An M3U file read into tracks, with how many entries could not be used.
#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub tracks: Vec<Track>,
    pub skipped: usize,
}

fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The video ID of a YouTube or YouTube Music URL (`watch?v=`, `youtu.be/`), checked for shape.
pub fn youtube_id(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.to_ascii_lowercase();
    let id = match host.as_str() {
        "youtu.be" => path.split(['?', '#', '/']).next()?.to_owned(),
        "music.youtube.com" | "www.youtube.com" | "youtube.com" | "m.youtube.com" => {
            let query = path.strip_prefix("watch?")?;
            let query = query.split('#').next()?;
            query
                .split('&')
                .find_map(|pair| pair.strip_prefix("v="))?
                .to_owned()
        }
        _ => return None,
    };
    valid_video_id(&id).then_some(id)
}

/// `#EXTINF:<seconds>,<artist> - <title>` → (seconds, artist, title).
fn extinf(info: &str) -> (f64, String, String) {
    let (seconds, name) = info.split_once(',').unwrap_or((info, ""));
    // Attributes (`tvg-id="…"`) may follow the duration before the comma.
    let seconds = seconds
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(0.0);
    let name = name.trim();
    match name.split_once(" - ") {
        Some((artist, title)) if !title.trim().is_empty() => {
            (seconds, artist.trim().to_owned(), title.trim().to_owned())
        }
        _ => (seconds, String::new(), name.to_owned()),
    }
}

/// Read M3U/M3U8 text. Relative paths resolve against `base` (the playlist's folder); a local
/// entry is kept only when `usable` accepts its path. Web entries must be YouTube video URLs.
/// Unusable, duplicate and excess entries are counted in `skipped`.
pub fn parse_m3u(text: &str, base: Option<&Path>, usable: impl Fn(&Path) -> bool) -> Parsed {
    let mut parsed = Parsed::default();
    let mut info: Option<(f64, String, String)> = None;
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            info = Some(extinf(rest));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let (duration, artist, title) = info.take().unwrap_or_default();
        if parsed.tracks.len() >= MAX_ENTRIES {
            parsed.skipped += 1;
            continue;
        }
        let track = if line.contains("://") {
            youtube_id(line).map(|id| Track {
                title: if title.is_empty() { id.clone() } else { title },
                artist,
                duration,
                thumbnail: format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"),
                source: format!("https://music.youtube.com/watch?v={id}"),
                id,
                ..Default::default()
            })
        } else {
            let path = PathBuf::from(line);
            let path = match base {
                Some(base) if path.is_relative() => base.join(path),
                _ => path,
            };
            usable(&path).then(|| {
                let source = path.to_string_lossy().into_owned();
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Unknown title")
                    .to_owned();
                Track {
                    id: source.clone(),
                    title: if title.is_empty() { stem } else { title },
                    artist,
                    duration,
                    source,
                    ..Default::default()
                }
            })
        };
        match track {
            Some(t) if !parsed.tracks.iter().any(|o| o.key() == t.key()) => parsed.tracks.push(t),
            _ => parsed.skipped += 1,
        }
    }
    parsed
}

/// Read an `.m3u`/`.m3u8` file into a playlist name (the file name) and its tracks.
pub fn import(path: &Path) -> Result<(String, Parsed)> {
    if path.as_os_str().is_empty() {
        bail!("Enter the path of an .m3u or .m3u8 file");
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if !matches!(ext.as_deref(), Some("m3u" | "m3u8")) {
        bail!("Only .m3u and .m3u8 playlists can be imported");
    }
    let size = fs::metadata(path)
        .with_context(|| format!("Opening {}", path.display()))?
        .len();
    if size > MAX_FILE_BYTES {
        bail!("{} is too large to be a playlist", path.display());
    }
    let bytes = fs::read(path).with_context(|| format!("Reading {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Imported playlist")
        .to_owned();
    let parsed = parse_m3u(&text, path.parent(), |p| {
        p.is_file() && crate::services::is_audio(p)
    });
    Ok((name, parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn online(id: &str, title: &str, artist: &str, duration: f64) -> Track {
        Track {
            id: id.into(),
            title: title.into(),
            artist: artist.into(),
            album: "Album".into(),
            duration,
            source: format!("https://music.youtube.com/watch?v={id}"),
            ..Default::default()
        }
    }

    fn local(path: &str, title: &str) -> Track {
        Track {
            id: path.into(),
            title: title.into(),
            source: path.into(),
            ..Default::default()
        }
    }

    #[test]
    fn m3u_has_extinf_and_locations() {
        let text = write_m3u(&[
            online("dQw4w9WgXcQ", "Never\nGonna", "Rick", 212.6),
            local(r"C:\Music\a.mp3", "A"),
        ]);
        assert_eq!(
            text,
            "#EXTM3U\n#EXTINF:213,Rick - Never Gonna\nhttps://music.youtube.com/watch?v=dQw4w9WgXcQ\n#EXTINF:-1,A\nC:\\Music\\a.mp3\n"
        );
    }

    #[test]
    fn csv_escapes_commas_quotes_newlines_and_formulas() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_field("=SUM(A1)"), "'=SUM(A1)");
        assert_eq!(csv_field("-ish"), "-ish");
        let text = write_csv(&[online("dQw4w9WgXcQ", "Hi, \"there\"", "Rick", 0.0)]);
        assert_eq!(
            text,
            "title,artist,album,duration,url\r\n\"Hi, \"\"there\"\"\",Rick,Album,,https://music.youtube.com/watch?v=dQw4w9WgXcQ\r\n"
        );
    }

    #[test]
    fn youtube_ids_come_from_known_hosts_only() {
        let id = Some("dQw4w9WgXcQ".to_owned());
        assert_eq!(
            youtube_id("https://music.youtube.com/watch?v=dQw4w9WgXcQ"),
            id
        );
        assert_eq!(
            youtube_id("https://www.youtube.com/watch?list=x&v=dQw4w9WgXcQ"),
            id
        );
        assert_eq!(youtube_id("http://youtube.com/watch?v=dQw4w9WgXcQ#t"), id);
        assert_eq!(youtube_id("https://youtu.be/dQw4w9WgXcQ?t=3"), id);
        assert_eq!(youtube_id("https://evil.com/watch?v=dQw4w9WgXcQ"), None);
        assert_eq!(
            youtube_id("https://youtube.com.evil.com/watch?v=dQw4w9WgXcQ"),
            None
        );
        assert_eq!(youtube_id("https://youtu.be/short"), None);
        assert_eq!(youtube_id("https://youtu.be/dQw4w9WgX;Q"), None);
        assert_eq!(youtube_id("ftp://youtu.be/dQw4w9WgXcQ"), None);
    }

    #[test]
    fn parses_m3u_entries_and_counts_skips() {
        let text = "\u{feff}#EXTM3U\r\n\
            #EXTINF:213,Rick - Never Gonna\r\n\
            https://music.youtube.com/watch?v=dQw4w9WgXcQ\r\n\
            \r\n\
            #EXTINF:-1 tvg-id=\"x\",Song\r\n\
            sub/song.mp3\r\n\
            missing.mp3\r\n\
            https://example.com/stream.mp3\r\n\
            https://youtu.be/dQw4w9WgXcQ\r\n\
            #EXTVLCOPT:foo\r\n";
        let base = Path::new("/music");
        let parsed = parse_m3u(text, Some(base), |p| p == base.join("sub/song.mp3"));
        assert_eq!(parsed.skipped, 3, "missing file, foreign URL, duplicate");
        assert_eq!(parsed.tracks.len(), 2);
        let yt = &parsed.tracks[0];
        assert_eq!(yt.id, "dQw4w9WgXcQ");
        assert_eq!(yt.title, "Never Gonna");
        assert_eq!(yt.artist, "Rick");
        assert_eq!(yt.duration, 213.0);
        assert_eq!(yt.source, "https://music.youtube.com/watch?v=dQw4w9WgXcQ");
        let file = &parsed.tracks[1];
        assert_eq!(Path::new(&file.source), base.join("sub/song.mp3"));
        assert_eq!(file.title, "Song");
        assert_eq!(file.artist, "");
        assert_eq!(file.duration, 0.0);
    }

    #[test]
    fn written_m3u_reads_back() {
        let tracks = vec![online("dQw4w9WgXcQ", "Never Gonna", "Rick", 213.0)];
        let parsed = parse_m3u(&write_m3u(&tracks), None, |_| false);
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.tracks[0].title, "Never Gonna");
        assert_eq!(parsed.tracks[0].artist, "Rick");
        assert_eq!(parsed.tracks[0].key(), "dQw4w9WgXcQ");
    }

    #[test]
    fn export_never_overwrites_and_checks_the_extension() {
        let dir = std::env::temp_dir().join(format!("ytdugem-io-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("list.m3u8");
        let _ = fs::remove_file(&path);
        let tracks = [online("dQw4w9WgXcQ", "T", "A", 1.0)];
        export(&path, &tracks).unwrap();
        let before = fs::read_to_string(&path).unwrap();
        assert!(export(&path, &[]).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        assert!(export(&dir.join("list.txt"), &tracks).is_err());
        assert!(export(Path::new(""), &tracks).is_err());
        assert!(import(&dir.join("list.txt")).is_err());
        let (name, parsed) = import(&path).unwrap();
        assert_eq!(name, "list");
        assert_eq!(parsed.tracks.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
