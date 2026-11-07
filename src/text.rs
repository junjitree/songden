//! Comparing song and album names written by different people: Spotify, file
//! tags, MusicBrainz and torrent uploaders all spell the same thing differently.

use std::{collections::BTreeSet, sync::LazyLock};

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// Words every soundtrack title carries in some form; they say nothing about which one.
const BOILERPLATE: &[&str] = &[
    "original",
    "motion",
    "picture",
    "soundtrack",
    "deluxe",
    "edition",
    "the",
    "from",
    "music",
    "ost",
    "film",
    "songs",
    "version",
    "english",
    "a",
    "an",
    "of",
    "and",
];

static DASH_CREDIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+-\s+(from|feat|ft|with)\b.*$").expect("valid regex"));
static PAREN_CREDIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[(\[](feat|ft|with|from)\b[^)\]]*[)\]]").expect("valid regex"));
static NON_ALNUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^a-z0-9]+").expect("valid regex"));

/// Lowercase ASCII with accents folded and apostrophes dropped ("Tío" -> "tio",
/// "Don’t" -> "dont").
fn fold(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '`' | '\u{b4}'))
        .collect::<String>()
        .nfkd()
        .filter(char::is_ascii)
        .collect::<String>()
        .to_lowercase()
}

/// A song title reduced to its words, with credit tails dropped.
///
/// Only tails that credit someone else go: `- From "Encanto"`, `(feat. X)`,
/// `(with Y)`. Version tails stay, so "How Far I'll Go - Reprise" and
/// "How Far I'll Go (Reprise)" agree, and neither equals the plain song.
pub fn norm(title: &str) -> String {
    let s = fold(title);
    let s = DASH_CREDIT.replace(&s, "");
    let s = PAREN_CREDIT.replace_all(&s, "");
    NON_ALNUM.replace_all(&s, " ").trim().to_string()
}

/// All the words of a name, folded.
pub fn words(s: &str) -> Vec<String> {
    NON_ALNUM
        .split(&fold(s))
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// The words that identify an album: soundtrack boilerplate and years removed.
/// The year is compared on its own; catalogue titles such as "Encanto (2021
/// film soundtrack)" carry it in the name and torrent names often don't.
pub fn core(s: &str) -> BTreeSet<String> {
    words(s)
        .into_iter()
        .filter(|w| !BOILERPLATE.contains(&w.as_str()))
        .filter(|w| !(w.len() == 4 && w.chars().all(|c| c.is_ascii_digit())))
        .collect()
}

/// Share of words two album names have in common (Jaccard index of `core`).
pub fn overlap(a: &str, b: &str) -> f64 {
    let (a, b) = (core(a), core(b));
    let union = a.union(&b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(&b).count() as f64 / union as f64
}

/// Decodes the HTML character references Spotify's pages use in attribute
/// values: numeric (`&#x27;`, `&#39;`) and the five XML named ones. Anything
/// else is left as written.
pub fn unescape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .map(|hex| u32::from_str_radix(hex, 16))
                .or_else(|| entity.strip_prefix('#').map(str::parse::<u32>))
                .and_then(|n| n.ok())
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_stay_distinct() {
        assert_eq!(
            norm("How Far I'll Go - Reprise"),
            norm("How Far I'll Go (Reprise)")
        );
        assert_ne!(norm("How Far I'll Go - Reprise"), norm("How Far I'll Go"));
        assert_ne!(
            norm("How Far I'll Go - Alessia Cara Version"),
            norm("How Far I'll Go")
        );
    }

    #[test]
    fn credits_and_spelling_are_ignored() {
        assert_eq!(
            norm("Surface Pressure - From \"Encanto\""),
            norm("Surface Pressure")
        );
        assert_eq!(norm("Song (feat. Someone)"), norm("Song"));
        assert_eq!(
            norm("We Don't Talk About Bruno"),
            norm("We Don\u{2019}t Talk About Bruno")
        );
        assert_eq!(norm("Tío Bruno"), norm("Tio Bruno"));
    }

    #[test]
    fn album_names_compare_without_boilerplate_or_years() {
        assert_eq!(
            overlap(
                "Encanto (Original Motion Picture Soundtrack)",
                "Encanto (2021 film soundtrack, English version)"
            ),
            1.0
        );
        // a sequel shares the name but is a different album
        assert!(
            overlap(
                "Moana (Original Motion Picture Soundtrack/Deluxe Edition)",
                "Moana 2 (Original Score)"
            ) < 0.6
        );
        assert_eq!(
            overlap(
                "Moana (Original Motion Picture Soundtrack/Deluxe Edition)",
                "Moana: Original Motion Picture Soundtrack"
            ),
            1.0
        );
    }

    #[test]
    fn html_references_decode() {
        assert_eq!(
            unescape_html("Auli&#x27;i Cravalho &amp; Disney"),
            "Auli'i Cravalho & Disney"
        );
        assert_eq!(
            unescape_html("&#39;quoted&#39; &quot;x&quot;"),
            "'quoted' \"x\""
        );
        assert_eq!(unescape_html("AT&T &bogus; 5 & 6"), "AT&T &bogus; 5 & 6");
    }
}
