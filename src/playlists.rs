//! The playlist links, and each playlist matched against the songs on disk.
//!
//! Spotify is read only when a grown-up adds a playlist or asks for a check
//! (`force`), never on its own: the listener's playlists change when a grown-up
//! decides. Each playlist's song list is kept (`playlists-spotify.json`); songs
//! not on disk yet are handed to the fetcher at that moment.
//!
//! Which songs are playable is worked out every time a playlist is asked for,
//! from the kept list and the files on disk, so songs that finish downloading
//! appear by themselves. The kept lists mean the player works when Spotify or the
//! internet is unreachable.

use std::{collections::BTreeMap, path::Path, sync::Mutex};

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    Result,
    fetch::{Fetcher, Library},
    spotify::{self, Spotify, Track},
    store::JsonStore,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Song {
    /// The Spotify song id; also the file name on disk.
    pub id: String,
    pub seconds: u64,
    pub title: String,
    pub artist: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Missing {
    pub title: String,
    pub artist: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resolved {
    pub id: String,
    #[serde(rename = "spotifyName")]
    pub spotify_name: String,
    pub name: String,
    pub cover: Option<String>,
    pub tracks: Vec<Song>,
    pub total: usize,
    pub missing: Vec<Missing>,
}

#[derive(Default, Serialize, Deserialize)]
struct Order {
    order: Vec<String>,
}

/// The pattern for a playlist name prefix ("Mia - Moana" with prefix "Mia"):
/// the word itself, then a dash, colon or bar.
pub fn prefix_pattern(prefix: &str) -> Regex {
    Regex::new(&format!(
        r"(?i)^\s*{}\s*[-:\u{{2013}}\u{{2014}}|]\s*",
        regex::escape(prefix.trim())
    ))
    .expect("an escaped prefix is a valid pattern")
}

/// The name a tile shows: the Spotify name without the prefix, if one is set.
pub fn display_name(prefix: Option<&Regex>, spotify_name: &str) -> String {
    let Some(prefix) = prefix else {
        return spotify_name.to_string();
    };
    let shown = prefix.replace(spotify_name, "");
    if shown.trim().is_empty() {
        spotify_name.to_string()
    } else {
        shown.trim().to_string()
    }
}

pub struct Playlists {
    order: Mutex<JsonStore<Order>>,
    /// Each playlist as Spotify last gave it.
    from_spotify: Mutex<JsonStore<BTreeMap<String, spotify::Playlist>>>,
    prefix: Option<Regex>,
    spotify: Spotify,
    library: Library,
    fetcher: Fetcher,
}

impl Playlists {
    pub fn new(
        state_dir: &Path,
        prefix: Option<&str>,
        spotify: Spotify,
        library: Library,
        fetcher: Fetcher,
    ) -> Self {
        Self {
            order: Mutex::new(JsonStore::load(state_dir, "playlists.json")),
            from_spotify: Mutex::new(JsonStore::load(state_dir, "playlists-spotify.json")),
            prefix: prefix.map(prefix_pattern),
            spotify,
            library,
            fetcher,
        }
    }

    pub fn ids(&self) -> Vec<String> {
        self.order
            .lock()
            .map(|o| o.data.order.clone())
            .unwrap_or_default()
    }

    pub fn has(&self, id: &str) -> bool {
        self.ids().iter().any(|i| i == id)
    }

    /// Adds a playlist; None if Spotify couldn't give it to us (then it isn't kept).
    pub async fn add(&self, id: &str) -> Result<Option<Resolved>> {
        if !self.has(id) {
            let mut order = self.order.lock().map_err(poisoned)?;
            order.data.order.push(id.to_string());
            order.save()?;
        }
        let resolved = self.get(id, true).await;
        if resolved.is_none() {
            self.remove(id)?;
        }
        Ok(resolved)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        {
            let mut order = self.order.lock().map_err(poisoned)?;
            order.data.order.retain(|i| i != id);
            order.save()?;
        }
        let mut kept = self.from_spotify.lock().map_err(poisoned)?;
        kept.data.remove(id);
        kept.save()
    }

    /// The playlist with what's on disk. `force` (a grown-up's check, or a
    /// playlist never read) reads Spotify first.
    pub async fn get(&self, id: &str, force: bool) -> Option<Resolved> {
        let known = self.from_spotify.lock().ok()?.data.contains_key(id);
        if (force || !known)
            && let Err(err) = self.read_spotify(id).await
        {
            warn!("playlist {id}: Spotify read failed, keeping the last good copy: {err}");
        }
        let raw = self.from_spotify.lock().ok()?.data.get(id).cloned()?;
        Some(self.resolve(id, &raw))
    }

    /// Reads the playlist from Spotify, keeps it, and hands songs not on disk to
    /// the fetcher.
    async fn read_spotify(&self, id: &str) -> Result<()> {
        info!("playlist {id}: reading Spotify");
        let raw = self.spotify.playlist(id).await?;
        let missing: Vec<Track> = raw
            .tracks
            .iter()
            .filter(|t| t.id().is_some_and(|tid| self.library.audio(tid).is_none()))
            .cloned()
            .collect();
        info!(
            "playlist {:?}: {} songs, {} to fetch",
            raw.name,
            raw.tracks.len(),
            missing.len()
        );
        {
            let mut kept = self.from_spotify.lock().map_err(poisoned)?;
            kept.data.insert(id.to_string(), raw);
            kept.save()?;
        }
        if !missing.is_empty() {
            self.fetcher.kick(missing);
        }
        Ok(())
    }

    /// Splits a kept song list into songs on disk and songs still waiting.
    fn resolve(&self, id: &str, raw: &spotify::Playlist) -> Resolved {
        let mut tracks = Vec::new();
        let mut missing = Vec::new();
        for t in &raw.tracks {
            match t.id().filter(|tid| self.library.audio(tid).is_some()) {
                Some(tid) => tracks.push(Song {
                    id: tid.to_string(),
                    seconds: t.seconds,
                    title: t.title.clone(),
                    artist: t.artists.clone(),
                }),
                None => missing.push(Missing {
                    title: t.title.clone(),
                    artist: t.artists.clone(),
                }),
            }
        }
        Resolved {
            id: id.to_string(),
            name: display_name(self.prefix.as_ref(), &raw.name),
            spotify_name: raw.name.clone(),
            cover: raw.cover.clone(),
            total: raw.tracks.len(),
            tracks,
            missing,
        }
    }

    /// Every playlist in order; `force` re-reads each from Spotify now.
    pub async fn all(&self, force: bool) -> Vec<(String, Option<Resolved>)> {
        let mut out = Vec::new();
        for id in self.ids() {
            let p = self.get(&id, force).await;
            out.push((id, p));
        }
        out
    }

    /// Every song id on any current playlist that is on disk.
    pub fn song_ids(&self) -> Vec<String> {
        let ids = self.ids();
        let Ok(kept) = self.from_spotify.lock() else {
            return Vec::new();
        };
        let mut out: Vec<String> = ids
            .iter()
            .filter_map(|id| kept.data.get(id))
            .flat_map(|p| p.tracks.iter().filter_map(|t| t.id()))
            .filter(|tid| self.library.audio(tid).is_some())
            .map(String::from)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Whether this song is on a current playlist (the only songs served).
    pub fn on_a_playlist(&self, song_id: &str) -> bool {
        let ids = self.ids();
        self.from_spotify.lock().is_ok_and(|kept| {
            ids.iter()
                .filter_map(|id| kept.data.get(id))
                .any(|p| p.tracks.iter().any(|t| t.id() == Some(song_id)))
        })
    }

    pub fn cover(&self, id: &str) -> Option<String> {
        if !self.has(id) {
            return None;
        }
        self.from_spotify.lock().ok()?.data.get(id)?.cover.clone()
    }
}

fn poisoned<T>(_: std::sync::PoisonError<T>) -> crate::Error {
    crate::Error::Internal("state lock poisoned".into())
}

#[cfg(test)]
mod tests {
    use super::{display_name, prefix_pattern};

    #[test]
    fn tiles_drop_the_listener_prefix() {
        let mia = prefix_pattern("Mia");
        let p = Some(&mia);
        assert_eq!(display_name(p, "Mia - Moana"), "Moana");
        assert_eq!(display_name(p, "mia: Car songs"), "Car songs");
        assert_eq!(display_name(p, "Mia \u{2013} Tarzan"), "Tarzan");
        assert_eq!(display_name(p, "Mia"), "Mia");
        assert_eq!(display_name(p, "Miami Hits"), "Miami Hits");
        assert_eq!(display_name(None, "Mia - Moana"), "Mia - Moana");
    }

    #[test]
    fn a_prefix_is_taken_literally() {
        let odd = prefix_pattern("A.B");
        assert_eq!(display_name(Some(&odd), "AxB - Songs"), "AxB - Songs");
        assert_eq!(display_name(Some(&odd), "A.B - Songs"), "Songs");
    }
}
