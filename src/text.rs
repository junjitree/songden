//! Comparing song titles written by different people: Spotify and YouTube
//! uploaders spell the same song differently.

use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

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
