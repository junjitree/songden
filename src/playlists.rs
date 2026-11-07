//! The playlist links, and each playlist matched against the library.
//!
//! Spotify is read only when a grown-up adds a playlist or asks for a check
//! (`force`), never on its own: the listener's playlists change when a grown-up
//! decides. Spotify's song list is kept (`playlist-spotify.json`) and matched
//! against the library; songs not in it yet are handed to the ingester.
//!
//! The library side is re-checked without Spotify: when a playlist with songs
//! still waiting is asked for, at most every `LIBRARY_RECHECK`, its kept song
//! list is matched again, so finished downloads appear by themselves. The last
//! good copy of everything is on disk, so the player works when Spotify or the
//! internet is unreachable.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};

use regex::Regex;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    Result,
    ingest::Ingester,
    jellyfin::Jellyfin,
    spotify::{self, Spotify},
    store::JsonStore,
};

/// How often a playlist with songs still waiting is matched against the library again.
const LIBRARY_RECHECK: Duration = Duration::from_secs(300);

/// The pattern for a playlist name prefix ("Mia - Moana" with prefix "Mia"):
/// the word itself, then a dash, colon or bar.
pub fn prefix_pattern(prefix: &str) -> Regex {
    Regex::new(&format!(
        r"(?i)^\s*{}\s*[-:\u{{2013}}\u{{2014}}|]\s*",
        regex::escape(prefix.trim())
    ))
    .expect("an escaped prefix is a valid pattern")
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Song {
    /// Jellyfin item id.
    pub id: String,
    #[serde(rename = "albumId")]
    pub album_id: Option<String>,
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
    cache: Mutex<JsonStore<BTreeMap<String, Resolved>>>,
    /// Each playlist as Spotify last gave it.
    from_spotify: Mutex<JsonStore<BTreeMap<String, spotify::Playlist>>>,
    /// When each playlist was last matched against the library.
    matched: Mutex<HashMap<String, Instant>>,
    prefix: Option<Regex>,
    spotify: Spotify,
    jellyfin: Jellyfin,
    ingest: Ingester,
}

impl Playlists {
    pub fn new(
        state_dir: &Path,
        prefix: Option<&str>,
        spotify: Spotify,
        jellyfin: Jellyfin,
        ingest: Ingester,
    ) -> Self {
        Self {
            prefix: prefix.map(prefix_pattern),
            order: Mutex::new(JsonStore::load(state_dir, "playlists.json")),
            cache: Mutex::new(JsonStore::load(state_dir, "playlist-cache.json")),
            from_spotify: Mutex::new(JsonStore::load(state_dir, "playlist-spotify.json")),
            matched: Mutex::new(HashMap::new()),
            spotify,
            jellyfin,
            ingest,
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
        {
            let mut kept = self.from_spotify.lock().map_err(poisoned)?;
            kept.data.remove(id);
            kept.save()?;
        }
        let mut cache = self.cache.lock().map_err(poisoned)?;
        cache.data.remove(id);
        cache.save()
    }

    /// The playlist matched against the library. `force` (a grown-up's check, or
    /// a playlist never read) reads Spotify; otherwise only the library is
    /// re-checked, and only while songs are waiting.
    pub async fn get(&self, id: &str, force: bool) -> Option<Resolved> {
        let known = self.from_spotify.lock().ok()?.data.contains_key(id);
        let result = if force || !known {
            self.read_spotify(id).await
        } else if self.recheck_due(id) {
            self.match_library(id).await
        } else {
            Ok(())
        };
        if let Err(err) = result {
            warn!("playlist {id}: refresh failed, keeping the last good copy: {err}");
        }
        self.cache.lock().ok()?.data.get(id).cloned()
    }

    /// Whether a playlist has songs waiting and wasn't matched in the last
    /// `LIBRARY_RECHECK`; marks it matched now if so.
    fn recheck_due(&self, id: &str) -> bool {
        let waiting = self
            .cache
            .lock()
            .ok()
            .and_then(|c| c.data.get(id).map(|p| !p.missing.is_empty()))
            .unwrap_or(false);
        let Ok(mut matched) = self.matched.lock() else {
            return false;
        };
        let due = waiting
            && matched
                .get(id)
                .is_none_or(|at| at.elapsed() >= LIBRARY_RECHECK);
        if due {
            matched.insert(id.to_string(), Instant::now());
        }
        due
    }

    /// Reads the playlist from Spotify, keeps it, matches it, and hands songs not
    /// in the library to the ingester.
    async fn read_spotify(&self, id: &str) -> Result<()> {
        info!("playlist {id}: reading Spotify");
        let raw = self.spotify.playlist(id).await?;
        {
            let mut kept = self.from_spotify.lock().map_err(poisoned)?;
            kept.data.insert(id.to_string(), raw.clone());
            kept.save()?;
        }
        let missing = self.resolve(id, &raw).await?;
        if !missing.is_empty() {
            self.ingest.kick(missing);
        }
        Ok(())
    }

    /// Matches the kept Spotify song list against the library again.
    async fn match_library(&self, id: &str) -> Result<()> {
        let raw = self
            .from_spotify
            .lock()
            .map_err(poisoned)?
            .data
            .get(id)
            .cloned();
        match raw {
            Some(raw) => self.resolve(id, &raw).await.map(|_| ()),
            None => Ok(()),
        }
    }

    /// Matches a Spotify song list against the library and stores the result;
    /// returns the songs not in the library.
    async fn resolve(&self, id: &str, raw: &spotify::Playlist) -> Result<Vec<spotify::Track>> {
        let mut tracks = Vec::new();
        let mut missing = Vec::new();
        for t in &raw.tracks {
            match self.jellyfin.find(&t.title, &t.artists).await? {
                Some(item) => tracks.push(Song {
                    seconds: item.run_time_ticks.unwrap_or(0) / 10_000_000,
                    id: item.id,
                    album_id: item.album_id,
                    title: t.title.clone(),
                    artist: t.artists.clone(),
                }),
                None => missing.push(t.clone()),
            }
        }
        info!(
            "playlist {:?}: {} playable, {} not in the library yet",
            raw.name,
            tracks.len(),
            missing.len()
        );
        let resolved = Resolved {
            id: id.to_string(),
            name: display_name(self.prefix.as_ref(), &raw.name),
            spotify_name: raw.name.clone(),
            cover: raw.cover.clone(),
            total: raw.tracks.len(),
            missing: missing
                .iter()
                .map(|t| Missing {
                    title: t.title.clone(),
                    artist: t.artists.clone(),
                })
                .collect(),
            tracks,
        };
        let mut cache = self.cache.lock().map_err(poisoned)?;
        cache.data.insert(id.to_string(), resolved);
        cache.save()?;
        Ok(missing)
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

    /// Every song id on any current playlist.
    pub fn song_ids(&self) -> BTreeSet<String> {
        let ids: BTreeSet<String> = self.ids().into_iter().collect();
        self.cache
            .lock()
            .map(|c| {
                c.data
                    .values()
                    .filter(|p| ids.contains(&p.id))
                    .flat_map(|p| p.tracks.iter().map(|t| t.id.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The song with this Jellyfin id, if it is on a current playlist.
    pub fn song(&self, item_id: &str) -> Option<Song> {
        let ids: BTreeSet<String> = self.ids().into_iter().collect();
        let cache = self.cache.lock().ok()?;
        cache
            .data
            .values()
            .filter(|p| ids.contains(&p.id))
            .flat_map(|p| p.tracks.iter())
            .find(|t| t.id == item_id)
            .cloned()
    }

    pub fn cover(&self, id: &str) -> Option<String> {
        if !self.has(id) {
            return None;
        }
        self.cache.lock().ok()?.data.get(id)?.cover.clone()
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
