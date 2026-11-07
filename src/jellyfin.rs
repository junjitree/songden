//! Finds Spotify songs in the Jellyfin library and streams them.

use std::{collections::BTreeSet, time::Duration};

use serde::Deserialize;

use crate::{Result, text::norm};

const TIMEOUT: Duration = Duration::from_secs(20);
/// Candidates fetched per song; a common word ("love") can have many hits.
const SEARCH_LIMIT: u32 = 200;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Item {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub album_artist: Option<String>,
    #[serde(default)]
    pub album_id: Option<String>,
    #[serde(default)]
    pub run_time_ticks: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Items {
    items: Vec<Item>,
}

/// The library item that is this Spotify song, if any.
///
/// Titles must agree once normalised (so an instrumental or a reprise is never
/// the plain song: its version word is part of the title), and among equals the item sharing the most
/// artists with Spotify wins.
pub fn pick<'a>(title: &str, artists: &str, items: &'a [Item]) -> Option<&'a Item> {
    let want = norm(title);
    if want.is_empty() {
        return None;
    }
    let spotify_artists: BTreeSet<String> = artists
        .split(',')
        .map(norm)
        .filter(|a| !a.is_empty())
        .collect();
    let shared = |item: &Item| {
        item.artists
            .iter()
            .chain(item.album_artist.iter())
            .map(|a| norm(a))
            .filter(|a| spotify_artists.contains(a))
            .count()
    };
    items
        .iter()
        .filter(|i| norm(&i.name) == want)
        .max_by_key(|i| shared(i))
}

/// The word to search Jellyfin by: the title's longest word as written.
///
/// Jellyfin's search is literal about punctuation, and `pick` does the exact
/// comparison anyway, so one distinctive word is enough. Apostrophes split
/// words here ("I'll" is "I" and "ll"): folding them away first would search for
/// "ill", which Jellyfin does not connect to "I'll".
pub fn search_term(title: &str) -> Option<String> {
    title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .rev() // max_by_key keeps the last of equals; this makes it the first
        .max_by_key(|w| w.chars().count())
        .map(str::to_string)
}

#[derive(Clone)]
pub struct Jellyfin {
    client: reqwest::Client,
    url: String,
    token: String,
}

impl Jellyfin {
    pub fn new(url: String, token: String) -> Result<Self> {
        let client = reqwest::Client::builder().timeout(TIMEOUT).build()?;
        Ok(Self { client, url, token })
    }

    fn auth(&self) -> String {
        format!("MediaBrowser Token=\"{}\"", self.token)
    }

    /// The library item for a Spotify song, or None if it isn't downloaded yet.
    pub async fn find(&self, title: &str, artists: &str) -> Result<Option<Item>> {
        let Some(term) = search_term(title) else {
            return Ok(None);
        };
        let items: Items = self
            .client
            .get(format!("{}/Items", self.url))
            .header("Authorization", self.auth())
            .query(&[
                ("Recursive", "true"),
                ("IncludeItemTypes", "Audio"),
                ("searchTerm", &term),
                ("Fields", "Artists,AlbumArtist,AlbumId,RunTimeTicks"),
                ("Limit", &SEARCH_LIMIT.to_string()),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(pick(title, artists, &items.items).cloned())
    }

    /// The original audio file, honouring a Range header.
    pub async fn audio(&self, id: &str, range: Option<&str>) -> Result<reqwest::Response> {
        let mut req = self
            .client
            .get(format!("{}/Audio/{id}/stream", self.url))
            .header("Authorization", self.auth())
            .query(&[("static", "true")]);
        if let Some(range) = range {
            req = req.header("Range", range);
        }
        Ok(req.send().await?)
    }

    pub async fn image(&self, id: &str) -> Result<reqwest::Response> {
        Ok(self
            .client
            .get(format!("{}/Items/{id}/Images/Primary", self.url))
            .header("Authorization", self.auth())
            .query(&[("maxWidth", "800"), ("quality", "90")])
            .send()
            .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, name: &str, artists: &[&str]) -> Item {
        Item {
            id: id.into(),
            name: name.into(),
            artists: artists.iter().map(|a| a.to_string()).collect(),
            album_artist: None,
            album_id: None,
            run_time_ticks: None,
        }
    }

    #[test]
    fn picks_the_vocal_not_the_instrumental() {
        let items = [
            item("i", "We Don't Talk About Bruno (Instrumental)", &[]),
            item(
                "v",
                "We Don\u{2019}t Talk About Bruno",
                &["Carolina Gaitán - La Gaita"],
            ),
        ];
        assert_eq!(
            pick(
                "We Don't Talk About Bruno",
                "Carolina Gaitán - La Gaita",
                &items
            )
            .unwrap()
            .id,
            "v"
        );
        assert_eq!(
            pick("We Don't Talk About Bruno - Instrumental", "", &items)
                .unwrap()
                .id,
            "i"
        );
    }

    #[test]
    fn search_terms_are_real_words() {
        // "I'll" folded would be "ill", which Jellyfin's search can't find
        assert_eq!(search_term("How Far I'll Go").as_deref(), Some("How"));
        assert_eq!(search_term("You're Welcome").as_deref(), Some("Welcome"));
        assert_eq!(search_term("Tío Bruno").as_deref(), Some("Bruno"));
        assert_eq!(search_term(" - ").as_deref(), None);
    }

    #[test]
    fn an_instrumental_never_stands_in_for_the_song() {
        let only_instrumental = [item(
            "i",
            "We Don't Talk About Bruno (Instrumental)",
            &["Carolina Gaitán - La Gaita"],
        )];
        assert!(
            pick(
                "We Don't Talk About Bruno",
                "Carolina Gaitán - La Gaita",
                &only_instrumental
            )
            .is_none()
        );
    }

    #[test]
    fn a_reprise_is_not_the_song() {
        let items = [item("song", "How Far I'll Go", &[])];
        assert!(pick("How Far I'll Go - Reprise", "Auli'i Cravalho", &items).is_none());
    }

    #[test]
    fn artists_break_ties() {
        let items = [
            item("cover", "How Far I'll Go", &["Someone Else"]),
            item("film", "How Far I'll Go", &["Auli'i Cravalho"]),
        ];
        assert_eq!(
            pick("How Far I'll Go", "Auli'i Cravalho, Disney", &items)
                .unwrap()
                .id,
            "film"
        );
    }
}
