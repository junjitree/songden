//! Reads Spotify's public pages: no login, no developer app.
//!
//! A playlist comes from its embed page, which lists at most the first hundred
//! songs (Spotify's cap). A song's album comes from the song's own page, whose
//! description reads "Artists · Album · Song · Year".

use std::{sync::LazyLock, time::Duration};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result, text::unescape_html};

pub const USER_AGENT: &str = "Mozilla/5.0";
const TIMEOUT: Duration = Duration::from_secs(20);
/// Spotify's own image hosts; nothing else is fetched for playlist covers.
const IMAGE_HOSTS: &[&str] = &[
    "i.scdn.co",
    "mosaic.scdn.co",
    "image-cdn-ak.spotifycdn.com",
    "image-cdn-fa.spotifycdn.com",
];

static NEXT_DATA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<script id="__NEXT_DATA__"[^>]*>(.*?)</script>"#).expect("valid regex")
});
static OG_DESCRIPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<meta property="og:description" content="([^"]*)""#).expect("valid regex")
});
static PLAYLIST_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"open\.spotify\.com/(?:intl-[a-z]+/)?(?:embed/)?playlist/([A-Za-z0-9]{22})")
        .expect("valid regex")
});

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Track {
    pub uri: String,
    pub title: String,
    pub artists: String,
    pub explicit: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub cover: Option<String>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TrackAlbum {
    pub album: String,
    pub year: Option<String>,
    pub artists: String,
}

pub fn is_id(s: &str) -> bool {
    s.len() == 22 && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The playlist id in a share link (or a bare id).
pub fn playlist_id(link: &str) -> Option<String> {
    let link = link.trim();
    if is_id(link) {
        return Some(link.to_string());
    }
    PLAYLIST_LINK.captures(link).map(|c| c[1].to_string())
}

pub fn cover_url_ok(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .is_some_and(|host| IMAGE_HOSTS.contains(&host))
}

/// The playlist on an embed page.
pub fn parse_embed(html: &str) -> Result<Playlist> {
    let data = NEXT_DATA
        .captures(html)
        .ok_or_else(|| Error::Upstream("no data on the embed page (private or deleted?)".into()))?;
    let root: Value = serde_json::from_str(&data[1]).map_err(|e| Error::Upstream(e.to_string()))?;
    let entity = &root["props"]["pageProps"]["state"]["data"]["entity"];
    let list = entity["trackList"]
        .as_array()
        .ok_or_else(|| Error::Upstream("playlist not found".into()))?;
    let cover = entity["coverArt"]["sources"]
        .as_array()
        .and_then(|sources| {
            sources
                .iter()
                .max_by_key(|s| s["width"].as_u64().unwrap_or(0))
                .and_then(|s| s["url"].as_str())
                .map(String::from)
        });
    let tracks = list
        .iter()
        .filter(|t| t["entityType"].as_str().unwrap_or("track") == "track")
        .filter_map(|t| {
            Some(Track {
                uri: t["uri"].as_str()?.to_string(),
                title: t["title"].as_str()?.to_string(),
                artists: t["subtitle"]
                    .as_str()
                    .unwrap_or_default()
                    .replace('\u{a0}', " "),
                explicit: t["isExplicit"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    Ok(Playlist {
        name: entity["name"].as_str().unwrap_or("Playlist").to_string(),
        cover,
        tracks,
    })
}

/// The album named on a song's own page.
pub fn parse_track_page(html: &str) -> Result<TrackAlbum> {
    let description = OG_DESCRIPTION
        .captures(html)
        .map(|c| unescape_html(&c[1]))
        .ok_or_else(|| Error::Upstream("no description on the song page".into()))?;
    let parts: Vec<&str> = description.split(" · ").map(str::trim).collect();
    let [artists, album, .., last] = parts.as_slice() else {
        return Err(Error::Upstream(format!(
            "unexpected song description {description:?}"
        )));
    };
    let year =
        (last.len() == 4 && last.chars().all(|c| c.is_ascii_digit())).then(|| last.to_string());
    Ok(TrackAlbum {
        album: album.to_string(),
        year,
        artists: artists.to_string(),
    })
}

#[derive(Clone)]
pub struct Spotify {
    client: reqwest::Client,
}

impl Spotify {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(TIMEOUT)
            .build()?;
        Ok(Self { client })
    }

    async fn page(&self, url: &str) -> Result<String> {
        Ok(self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }

    pub async fn playlist(&self, id: &str) -> Result<Playlist> {
        parse_embed(
            &self
                .page(&format!("https://open.spotify.com/embed/playlist/{id}"))
                .await?,
        )
    }

    pub async fn track_album(&self, uri: &str) -> Result<TrackAlbum> {
        let id = uri.rsplit(':').next().filter(|id| is_id(id));
        let id = id.ok_or_else(|| Error::Internal(format!("bad track uri {uri:?}")))?;
        parse_track_page(
            &self
                .page(&format!("https://open.spotify.com/track/{id}"))
                .await?,
        )
    }

    /// A playlist cover image, only from Spotify's own hosts.
    pub async fn image(&self, url: &str) -> Result<reqwest::Response> {
        if !cover_url_ok(url) {
            return Err(Error::NotFound);
        }
        Ok(self.client.get(url).send().await?.error_for_status()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMBED: &str = r#"<html><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"data":{"entity":{
        "name":"Mia - Moana",
        "coverArt":{"sources":[{"url":"https://image-cdn-ak.spotifycdn.com/small","width":60},{"url":"https://image-cdn-ak.spotifycdn.com/big","width":640}]},
        "trackList":[
          {"uri":"spotify:track:626IyVO7cgaCTFbjDE3KPB","title":"Tulou Tagaloa","subtitle":"Olivia Foa'i","isExplicit":false,"entityType":"track"},
          {"uri":"spotify:episode:abc","title":"A podcast","subtitle":"Someone","entityType":"episode"},
          {"uri":"spotify:track:4hDok0OAFX4tc0emTjMsuC","title":"Where You Are","subtitle":"Christopher Jackson, Disney","isExplicit":true,"entityType":"track"}
        ]}}}}}}</script></html>"#;

    #[test]
    fn reads_an_embed_page() {
        let p = parse_embed(EMBED).unwrap();
        assert_eq!(p.name, "Mia - Moana");
        assert_eq!(
            p.cover.as_deref(),
            Some("https://image-cdn-ak.spotifycdn.com/big")
        );
        // the podcast episode is not a song
        assert_eq!(p.tracks.len(), 2);
        assert_eq!(p.tracks[1].artists, "Christopher Jackson, Disney");
        assert!(p.tracks[1].explicit);
    }

    #[test]
    fn a_page_without_data_is_an_error() {
        assert!(parse_embed("<html>gone</html>").is_err());
    }

    #[test]
    fn reads_a_song_page() {
        let html = r#"<meta property="og:description" content="Auli&#x27;i Cravalho, Disney · Moana (Original Motion Picture Soundtrack/Deluxe Edition) · Song · 2016"/>"#;
        let a = parse_track_page(html).unwrap();
        assert_eq!(
            a.album,
            "Moana (Original Motion Picture Soundtrack/Deluxe Edition)"
        );
        assert_eq!(a.year.as_deref(), Some("2016"));
        assert_eq!(a.artists, "Auli'i Cravalho, Disney");
    }

    #[test]
    fn finds_ids_in_links() {
        let link = "https://open.spotify.com/playlist/1acYFjOOIQbL60zsnuxspn?si=5M-mBPrbS0SL-T_w5H1TBA&utm_source=copy-link";
        assert_eq!(playlist_id(link).as_deref(), Some("1acYFjOOIQbL60zsnuxspn"));
        assert_eq!(
            playlist_id("1acYFjOOIQbL60zsnuxspn").as_deref(),
            Some("1acYFjOOIQbL60zsnuxspn")
        );
        assert_eq!(
            playlist_id("https://open.spotify.com/album/2BnyvyHc9OVBBU8LlgA4C8"),
            None
        );
        assert_eq!(playlist_id("not a link"), None);
    }

    #[test]
    fn only_spotify_images_are_fetched() {
        assert!(cover_url_ok("https://i.scdn.co/image/ab67"));
        assert!(!cover_url_ok("https://i.scdn.co.evil.example/x"));
        assert!(!cover_url_ok("http://i.scdn.co/image/ab67"));
        assert!(!cover_url_ok("https://169.254.169.254/latest"));
    }
}
