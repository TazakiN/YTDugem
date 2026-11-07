//! Lyrics translation, after upstream `LyricsTranslation`: Google's lightweight web endpoint
//! (`sl=auto`, no key), lines batched up to 3,500 characters with private-use markers so every
//! answer maps back to its line. Timing is kept; a partial or malformed answer is discarded.

use super::LyricLine;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::time::Duration;

const ENDPOINT: &str = "https://translate.googleapis.com/translate_a/single";
const MAX_BATCH_CHARS: usize = 3_500;

/// Destination choices offered in Settings: (tag, name). Blank means the system language.
pub const LANGUAGES: [(&str, &str); 20] = [
    ("id", "Indonesian"),
    ("en", "English"),
    ("ms", "Malay"),
    ("jv", "Javanese"),
    ("su", "Sundanese"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh-CN", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"),
    ("th", "Thai"),
    ("vi", "Vietnamese"),
    ("tl", "Filipino"),
    ("hi", "Hindi"),
    ("ar", "Arabic"),
    ("es", "Spanish"),
    ("pt", "Portuguese"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("ru", "Russian"),
];

#[derive(Clone, Debug, PartialEq)]
pub enum Translation {
    /// The lyrics with each line's text translated; timing is the original's.
    Translated {
        lines: Vec<LyricLine>,
        source: String,
    },
    /// Already in the destination language: nothing to show instead.
    SameLanguage(String),
}

#[allow(dead_code)] // used by the player sheet (in progress)
#[derive(Clone, Debug, PartialEq)]
pub enum Romanization {
    /// One entry per input line, same length and timing; text is the romanized pronunciation.
    Romanized {
        lines: Vec<LyricLine>,
        source: String,
    },
    /// Every line is already in Latin script: nothing to show.
    AlreadyLatin,
}

fn http_client(agent: &str) -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(agent)
        .build()?)
}

/// One request for a batch's payload; `dt` is `t` (translate) or `rm` (romanize).
fn post_batch(
    client: &reqwest::blocking::Client,
    tl: &str,
    dt: &str,
    payload: &str,
) -> Result<String> {
    Ok(client
        .post(ENDPOINT)
        .header("Accept", "application/json")
        .form(&[
            ("client", "dict-chrome-ex"),
            ("sl", "auto"),
            ("tl", tl),
            ("dt", dt),
            ("q", payload),
        ])
        .send()?
        .error_for_status()?
        .text()?)
}

pub fn translate(lines: &[LyricLine], target: &str) -> Result<Translation> {
    let target = target.trim();
    if target.is_empty() {
        bail!("No translation language selected");
    }
    let slots: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].text.trim().is_empty())
        .collect();
    if slots.is_empty() {
        bail!("These lyrics have no text to translate");
    }
    let client = http_client("YTDugem/0.1 (lyrics translation)")?;
    let mut translated = Vec::with_capacity(slots.len());
    let mut sources: Vec<(String, usize)> = vec![];
    for batch in batches(&slots, lines) {
        let payload = payload(&batch, lines);
        let body = post_batch(&client, target, "t", &payload)?;
        let (parts, source) = parse_answer(&body, batch.len())?;
        translated.extend(parts);
        add_weight(&mut sources, source, payload.len());
    }
    let source = dominant(sources);
    if same_language(&source, target) {
        return Ok(Translation::SameLanguage(source));
    }
    let mut out = lines.to_vec();
    for (&i, text) in slots.iter().zip(translated) {
        out[i].text = text;
        // Word timings belong to the original words; the translation is highlighted by line.
        out[i].words.clear();
    }
    Ok(Translation::Translated { lines: out, source })
}

fn batches(slots: &[usize], lines: &[LyricLine]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = vec![];
    let mut length = 0;
    for &i in slots {
        // The separator is a newline, U+E000, 4 digits, U+E001, a newline: 12 bytes of UTF-8.
        let added = lines[i].text.len() + 12;
        match out.last_mut() {
            Some(batch) if length + added <= MAX_BATCH_CHARS => {
                batch.push(i);
                length += added;
            }
            _ => {
                out.push(vec![i]);
                length = added;
            }
        }
    }
    out
}

fn payload(batch: &[usize], lines: &[LyricLine]) -> String {
    let mut out = String::new();
    for (n, &i) in batch.iter().enumerate() {
        if n > 0 {
            out.push_str(&format!("\n\u{E000}{n:04}\u{E001}\n"));
        }
        out.push_str(lines[i].text.trim());
    }
    out
}

#[allow(dead_code)] // used by the player sheet (in progress)
pub fn romanize(lines: &[LyricLine]) -> Result<Romanization> {
    let slots: Vec<usize> = (0..lines.len())
        .filter(|&i| !lines[i].text.trim().is_empty())
        .collect();
    if slots.is_empty() {
        bail!("These lyrics have no text to romanize");
    }
    if !slots.iter().any(|&i| has_non_latin_letters(&lines[i].text)) {
        return Ok(Romanization::AlreadyLatin);
    }
    let client = http_client("YTDugem/0.1 (lyrics translation)")?;
    let mut romanized = Vec::with_capacity(slots.len());
    let mut sources: Vec<(String, usize)> = vec![];
    for batch in batches(&slots, lines) {
        let payload = payload(&batch, lines);
        let body = post_batch(&client, "en", "rm", &payload)?;
        let (parts, source) = parse_romanization(&body, batch.len())?;
        romanized.extend(parts);
        add_weight(&mut sources, source, payload.len());
    }
    let source = dominant(sources);
    if slots
        .iter()
        .zip(&romanized)
        .all(|(&i, text)| lines[i].text.trim() == text)
    {
        bail!("Romanization is not available for these lyrics");
    }
    let mut out = lines.to_vec();
    for (&i, text) in slots.iter().zip(romanized) {
        out[i].text = text;
        out[i].words.clear();
    }
    Ok(Romanization::Romanized { lines: out, source })
}

/// True for a letter outside the Latin script (U+0000..U+024F and the Latin Extended Additional block).
fn has_non_latin_letters(text: &str) -> bool {
    text.chars().any(|c| {
        let code = c as u32;
        c.is_alphabetic() && code > 0x024F && !(0x1E00..=0x1EFF).contains(&code)
    })
}

fn add_weight(sources: &mut Vec<(String, usize)>, source: String, weight: usize) {
    match sources.iter_mut().find(|(s, _)| *s == source) {
        Some((_, total)) => *total += weight,
        None => sources.push((source, weight)),
    }
}

/// The source language that carried the most text across all batches.
fn dominant(sources: Vec<(String, usize)>) -> String {
    sources
        .into_iter()
        .max_by_key(|(_, weight)| *weight)
        .map(|(s, _)| s)
        .unwrap_or_default()
}

/// The romanized answers split back at the markers; the source language must be known.
fn parse_romanization(body: &str, expected: usize) -> Result<(Vec<String>, String)> {
    let (parts, source) = split_answer(body, expected, 3)?;
    if source.is_empty() {
        bail!("The romanization came back without a source language");
    }
    Ok((parts, source))
}

fn parse_answer(body: &str, expected: usize) -> Result<(Vec<String>, String)> {
    split_answer(body, expected, 0)
}

/// The joined text of each answer segment (`segment` is the index: 0 for translation, 3 for
/// romanization) split back at the markers, and the detected source language.
fn split_answer(body: &str, expected: usize, segment: usize) -> Result<(Vec<String>, String)> {
    let root: Value = serde_json::from_str(body)?;
    let joined: String = root[0]
        .as_array()
        .ok_or_else(|| anyhow!("Unexpected translation response"))?
        .iter()
        .filter_map(|seg| seg.get(segment)?.as_str())
        .collect();
    let source = root[2].as_str().unwrap_or_default().to_owned();
    let mut parts = vec![];
    for (n, piece) in joined.split('\u{E000}').enumerate() {
        let text = if n == 0 {
            piece
        } else {
            piece.split_once('\u{E001}').map_or(piece, |(_, rest)| rest)
        };
        parts.push(text.trim().to_owned());
    }
    if parts.len() != expected || parts.iter().any(String::is_empty) {
        bail!("The translation came back incomplete");
    }
    Ok((parts, source))
}

/// `zh-CN` and `zh-TW` are one language here; the detected source is a base tag.
fn same_language(a: &str, b: &str) -> bool {
    let base = |s: &str| {
        let s = s
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match s.as_str() {
            "in" => "id".to_owned(),
            "iw" => "he".to_owned(),
            _ => s,
        }
    };
    !a.is_empty() && base(a) == base(b)
}

/// The Windows display language as a translation tag (`id-ID` gives `id`), or English.
pub fn system_language() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
        let mut buffer = [0u16; 85];
        // SAFETY: the buffer is LOCALE_NAME_MAX_LENGTH (85) wide characters, as required.
        let n = unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), buffer.len() as i32) };
        if n > 1 {
            let name = String::from_utf16_lossy(&buffer[..n as usize - 1]);
            if let Some(tag) = name.split('-').next().filter(|t| !t.is_empty()) {
                return tag.to_ascii_lowercase();
            }
        }
    }
    "en".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(time: f64, text: &str) -> LyricLine {
        LyricLine {
            time,
            text: text.into(),
            words: vec![],
        }
    }

    #[test]
    fn markers_map_answers_back_to_lines() {
        let lines = [
            line(1.0, "Aku milikmu"),
            line(2.0, " "),
            line(3.0, "Malam ini"),
        ];
        let slots = [0, 2];
        assert_eq!(
            payload(&slots, &lines),
            "Aku milikmu\n\u{E000}0001\u{E001}\nMalam ini"
        );
        let body = r#"[[["I am yours\n","Aku milikmu\n"],[" 0001\nTonight","x"]],null,"id"]"#;
        let (parts, source) = parse_answer(body, 2).unwrap();
        assert_eq!(parts, ["I am yours", "Tonight"]);
        assert_eq!(source, "id");
        assert!(
            parse_answer(body, 3).is_err(),
            "a dropped marker must not shift lines"
        );
        assert!(same_language("in", "id-ID") && !same_language("ja", "en"));
    }

    #[test]
    fn latin_detection_ignores_latin_letters() {
        assert!(!has_non_latin_letters("Aku milikmu"));
        assert!(!has_non_latin_letters("Café"));
        assert!(!has_non_latin_letters("Ẽ Ạ 123 !?"));
        assert!(has_non_latin_letters("靡く"));
        assert!(has_non_latin_letters("사랑"));
        assert!(has_non_latin_letters("Hello мир"));
    }

    #[test]
    fn romanization_reads_the_romanized_segment() {
        let (open, close) = ('\u{E000}', '\u{E001}');
        let body = format!(
            r#"[[["ignored","x","y","nabiku\n"],[null,null,null,"{open} 0001{close}\ntoppu"]],null,"ja"]"#
        );
        let (parts, source) = parse_romanization(&body, 2).unwrap();
        assert_eq!(parts, ["nabiku", "toppu"]);
        assert_eq!(source, "ja");
        let no_source = r#"[[[null,null,null,"nabiku"]],null,""]"#;
        assert!(parse_romanization(no_source, 1).is_err());
    }

    #[test]
    #[ignore = "requires internet access to Google's translation endpoint; no account"]
    fn live_romanization() {
        let lines = [line(1.0, "靡く"), line(5.0, "私は道の上")];
        let result = romanize(&lines);
        println!("{result:?}");
        assert!(matches!(result, Ok(Romanization::Romanized { .. })));
    }

    #[test]
    fn long_lyrics_are_batched() {
        let lines: Vec<_> = (0..200)
            .map(|i| line(i as f64, &"word ".repeat(10)))
            .collect();
        let slots: Vec<_> = (0..200).collect();
        let b = batches(&slots, &lines);
        assert!(b.len() > 1);
        assert_eq!(b.iter().map(Vec::len).sum::<usize>(), 200);
        assert!(b
            .iter()
            .all(|batch| payload(batch, &lines).len() <= MAX_BATCH_CHARS + 16));
    }

    #[test]
    #[ignore = "requires internet access to Google's translation endpoint; no account"]
    fn live_translation() {
        let lines = [
            line(1.0, "Aku milikmu malam ini"),
            line(5.0, "Terucap kata cinta"),
        ];
        let result = translate(&lines, "en").unwrap();
        println!("{result:?}");
        assert!(matches!(result, Translation::Translated { ref source, .. } if source == "id"));
        assert!(matches!(
            translate(&lines, "id").unwrap(),
            Translation::SameLanguage(_)
        ));
    }
}
