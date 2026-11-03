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
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("YTDugem/0.1 (lyrics translation)")
        .build()?;
    let mut translated = Vec::with_capacity(slots.len());
    let mut sources: Vec<(String, usize)> = vec![];
    for batch in batches(&slots, lines) {
        let payload = payload(&batch, lines);
        let body = client
            .post(ENDPOINT)
            .header("Accept", "application/json")
            .form(&[
                ("client", "dict-chrome-ex"),
                ("sl", "auto"),
                ("tl", target),
                ("dt", "t"),
                ("q", payload.as_str()),
            ])
            .send()?
            .error_for_status()?
            .text()?;
        let (parts, source) = parse_answer(&body, batch.len())?;
        translated.extend(parts);
        match sources.iter_mut().find(|(s, _)| *s == source) {
            Some((_, weight)) => *weight += payload.len(),
            None => sources.push((source, payload.len())),
        }
    }
    let source = sources
        .into_iter()
        .max_by_key(|(_, weight)| *weight)
        .map(|(s, _)| s)
        .unwrap_or_default();
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

/// The joined translation split back at the markers, and the detected source language.
fn parse_answer(body: &str, expected: usize) -> Result<(Vec<String>, String)> {
    let root: Value = serde_json::from_str(body)?;
    let joined: String = root[0]
        .as_array()
        .ok_or_else(|| anyhow!("Unexpected translation response"))?
        .iter()
        .filter_map(|segment| segment[0].as_str())
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
