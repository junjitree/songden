//! Gets missing playlist songs downloaded, album by album.
//!
//! Runs only when the player notices songs that are not in the library yet
//! (`kick`), never on a timer. Each song's album is learned from Spotify once and
//! remembered. Then, per album:
//!
//! 1. Lidarr: find the album in its catalogue (name and year must agree),
//!    monitor it, and run Lidarr's own search.
//! 2. If that grabbed nothing: search the indexers through Prowlarr by
//!    Spotify's album name, and offer the best few releases to Lidarr under a
//!    clean "Artist - Album (Year)" name. Lidarr still decides whether a release
//!    really is that album, so a wrong pick is rejected rather than imported.
//!    Soundtracks need this: their catalogue names ("Encanto (2021 film
//!    soundtrack, English version)") never match how torrents are named.
//!
//! An album that got nowhere is retried on a later kick, at most every
//! `RETRY_AFTER`.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{
    Error, Result,
    spotify::{Spotify, Track, TrackAlbum},
    store::JsonStore,
    text::{core, overlap, words},
};

const RETRY_AFTER: Duration = Duration::from_secs(6 * 3600);
/// Pause between Spotify song-page reads, to stay a polite visitor.
const SPOTIFY_GAP: Duration = Duration::from_millis(500);
/// How long Lidarr's own album search may run before we fall back.
const LIDARR_SEARCH_WAIT: Duration = Duration::from_secs(180);
const LIDARR_POLL: Duration = Duration::from_secs(5);
/// Releases offered to Lidarr per album in the fallback before giving up for now.
const FALLBACK_TRIES: usize = 3;
/// Catalogue match: share of identifying title words that must agree.
const MIN_OVERLAP: f64 = 0.6;
/// Lidarr's API answers slowly while it talks to its metadata server.
const API_TIMEOUT: Duration = Duration::from_secs(90);
/// A release naming one of these is another language's edition, unless the album does.
const LANGUAGES: &[&str] = &[
    "russian",
    "korean",
    "japanese",
    "french",
    "spanish",
    "latino",
    "latin",
    "portuguese",
    "german",
    "italian",
    "hindi",
    "tamil",
    "telugu",
    "polish",
    "dutch",
    "chinese",
    "mandarin",
    "cantonese",
    "thai",
    "vietnamese",
    "indonesian",
    "malay",
    "turkish",
    "arabic",
    "hebrew",
];

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AlbumState {
    #[default]
    Failed,
    Downloading,
    Imported,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AlbumRecord {
    pub state: AlbumState,
    pub album: String,
    #[serde(default)]
    pub lidarr_album_id: Option<i64>,
    #[serde(default)]
    pub note: String,
    /// Unix seconds of the last attempt.
    pub tried: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The catalogue entry that is this Spotify album: identifying words must mostly
/// agree and the year may differ by one at most.
pub fn pick_catalogue<'a>(info: &TrackAlbum, candidates: &'a [Value]) -> Option<&'a Value> {
    let year: Option<i32> = info.year.as_deref().and_then(|y| y.parse().ok());
    candidates
        .iter()
        .filter_map(|c| {
            let title = c["title"].as_str()?;
            let score = overlap(&info.album, title);
            let their_year: Option<i32> = c["releaseDate"]
                .as_str()
                .and_then(|d| d.get(..4)?.parse().ok());
            let year_ok = match (year, their_year) {
                (Some(a), Some(b)) => (a - b).abs() <= 1,
                _ => true,
            };
            (score >= MIN_OVERLAP && year_ok).then_some((score, c))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, c)| c)
}

/// Lossy releases that look like this album, best first: the standard edition
/// before a deluxe one (whatever Spotify's album is called), then ones that say
/// they are MP3 (uploaders also write "320", "V0", "CBR"...), then by seeders.
/// Lossless releases are never offered: they are many times the size, slower to
/// fetch, and what a phone caches anyway is the MP3. A release that names no
/// format is still offered, after the labelled ones; Lidarr checks the files.
fn first_artist(info: &TrackAlbum) -> &str {
    info.artists.split(',').next().unwrap_or_default().trim()
}

/// What the fallback searches the indexers for, in order: the album's full
/// name, then just the words that identify it, with "soundtrack" for a
/// soundtrack or else the first artist.
pub fn search_queries(info: &TrackAlbum) -> Vec<String> {
    let full = words(&info.album).join(" ");
    let mut loose: Vec<String> = core(&info.album).into_iter().collect();
    // `core` is a set; keep the words in the order the name has them
    let order = words(&info.album);
    loose.sort_by_key(|w| order.iter().position(|o| o == w));
    if words(&info.album).iter().any(|w| w == "soundtrack") {
        loose.push("soundtrack".into());
    } else if let Some(artist) = info.artists.split(',').next() {
        loose.extend(words(artist));
    }
    let loose = loose.join(" ");
    if loose.is_empty() || loose == full {
        vec![full]
    } else {
        vec![full, loose]
    }
}

static LOSSLESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(flac|alac|wav|lossless|hi-?res)\b|\b24[ -]?(bit|96|192)\b")
        .expect("valid regex")
});
static MP3: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bmp3\b|\b(320|256|192)(k|kbps|cbr)?\b|\bv[02]\b|\b(cbr|vbr)\b")
        .expect("valid regex")
});

/// Words that may follow an album's name in a release title without making it
/// a different album: soundtrack boilerplate and how a release describes itself.
const AFTER_NAME: &[&str] = &[
    "ost",
    "score",
    "deluxe",
    "edition",
    "expanded",
    "remastered",
    "remaster",
    "version",
    "soundtrack",
    "original",
    "motion",
    "picture",
    "music",
    "songs",
    "mp3",
    "flac",
    "cbr",
    "vbr",
    "web",
    "cd",
    "va",
    "various",
    "artists",
    "kbps",
    "album",
    "the",
    "from",
    "by",
    "and",
    "a",
];

/// Whether the release title has the album's identifying words in a row,
/// followed by nothing, an extra from `AFTER_NAME`, or something with digits (a
/// year, "320kbps", "1CD"). Uploaders put the artist before the name and all
/// sorts after it, so only the word right after the name is checked.
pub fn names_the_album(album: &str, title: &str) -> bool {
    let want = core(album);
    let name: Vec<String> = {
        let mut seen = BTreeSet::new();
        words(album)
            .into_iter()
            .filter(|w| want.contains(w) && seen.insert(w.clone()))
            .collect()
    };
    if name.is_empty() {
        return true;
    }
    let ordered = words(title);
    ordered.windows(name.len()).enumerate().any(|(i, run)| {
        run == name.as_slice()
            && ordered.get(i + name.len()).is_none_or(|next| {
                AFTER_NAME.contains(&next.as_str()) || next.chars().any(|c| c.is_ascii_digit())
            })
    })
}

fn is_lossless(title: &str) -> bool {
    LOSSLESS.is_match(title)
}

fn says_mp3(title: &str) -> bool {
    MP3.is_match(title)
}

pub fn rank_releases<'a>(
    album: &str,
    artist: &str,
    year: Option<&str>,
    results: &'a [Value],
) -> Vec<&'a Value> {
    let year: Option<i32> = year.and_then(|y| y.parse().ok());
    // A self-titled album ("The Lumineers" by The Lumineers) shares every word
    // with the artist's other albums, so only its year tells them apart.
    let artist_words: BTreeSet<String> = words(artist).into_iter().collect();
    let self_titled = !core(album).is_empty() && core(album).is_subset(&artist_words);
    let album_words: BTreeSet<String> = words(album).into_iter().collect();
    let want = core(album);
    let mut picks: Vec<(bool, bool, u64, &Value)> = results
        .iter()
        .filter_map(|r| {
            let title = r["title"].as_str()?;
            let seeders = r["seeders"].as_u64().unwrap_or(0);
            let title_words: BTreeSet<String> = words(title).into_iter().collect();
            if seeders < 1 || !want.is_subset(&title_words) || is_lossless(title) {
                return None;
            }
            // "Frozen Planet II" holds every word of "Frozen" but is another album:
            // the album's own words must be followed by the usual extras, not more title.
            if !names_the_album(album, title) {
                return None;
            }
            // For a self-titled album the release must name its year: "The
            // Lumineers - Automatic (2025)" is not their 2012 "The Lumineers".
            // Only then: other releases often carry the year they were uploaded
            // ("Moana (2020)" for the 2016 album), so the year can't be a rule.
            if self_titled {
                let names_the_year = year.is_some_and(|year| {
                    title_words
                        .iter()
                        .filter_map(|w| w.parse::<i32>().ok())
                        .any(|y| (y - year).abs() <= 1)
                });
                if !names_the_year {
                    return None;
                }
            }
            // "Moana 2" is not "Moana": a number right after one of the album's own
            // words, which the album name doesn't have. Other numbers ("24-96 FLAC",
            // "320kbps") describe the files, not the album.
            let ordered = words(title);
            let sequel = ordered.windows(2).any(|pair| {
                want.contains(&pair[0])
                    && pair[1].len() < 4
                    && pair[1].chars().all(|c| c.is_ascii_digit())
                    && !album_words.contains(&pair[1])
            });
            let other_language = LANGUAGES
                .iter()
                .any(|l| title_words.contains(*l) && !album_words.contains(*l));
            if sequel || other_language {
                return None;
            }
            // uploaders abbreviate Deluxe Edition as "D E"
            let is_deluxe = title_words.contains("deluxe") || title.to_lowercase().contains("d e)");
            Some((!is_deluxe, says_mp3(title), seeders, r))
        })
        .collect();
    picks.sort_by_key(|p| std::cmp::Reverse((p.0, p.1, p.2)));
    picks.into_iter().map(|p| p.3).collect()
}

/// The name a fallback release is offered to Lidarr under. Lidarr parses it, so
/// it must read as "Artist - Album (Year)" and carry a quality its profiles
/// recognise: a bare "MP3" parses as Unknown, which the Standard profile refuses.
/// The bitrate is a label for Lidarr's checks; the files decide what is imported.
pub fn clean_title(artist: &str, album: &str, year: &str) -> String {
    format!("{artist} - {album} ({year}) [MP3-320]")
}

/// The `/release/push` body for an indexer result.
///
/// Prowlarr hands out links to itself as `localhost`, which means each container
/// itself; Lidarr and the download client reach it by name, so both links are
/// rewritten. A "magnet" that is really an http link to Prowlarr (which then
/// redirects to the magnet) goes in as the download link: Lidarr follows that
/// redirect itself, while a download client given it as a magnet silently drops
/// it.
pub fn push_body(result: &Value, title: &str, prowlarr_url: &str) -> Value {
    let link = |key: &str| {
        result[key]
            .as_str()
            .map(|u| u.replace("http://localhost:9696", prowlarr_url))
    };
    let (download, magnet) = match (link("downloadUrl"), link("magnetUrl")) {
        (Some(d), m) => (Some(d), m.filter(|m| m.starts_with("magnet:"))),
        (None, Some(m)) if m.starts_with("magnet:") => (None, Some(m)),
        (None, m) => (m, None),
    };
    json!({
        "title": title, "downloadUrl": download, "magnetUrl": magnet, "protocol": "torrent",
        "publishDate": result["publishDate"], "indexer": result["indexer"],
        "size": result["size"], "seeders": result["seeders"],
    })
}

struct Arr {
    client: reqwest::Client,
    url: String,
    key: String,
}

impl Arr {
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<Value>,
    ) -> Result<Value> {
        let mut req = self
            .client
            .request(method, format!("{}/api/v1{path}", self.url))
            .header("X-Api-Key", &self.key)
            .query(query);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req.send().await?;
        let status = res.status();
        let text = res.text().await?;
        if !status.is_success() {
            return Err(Error::Upstream(format!(
                "{path}: {status} {}",
                text.chars().take(300).collect::<String>()
            )));
        }
        Ok(if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text)?
        })
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        self.call(reqwest::Method::GET, path, query, None).await
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.call(reqwest::Method::POST, path, &[], Some(body))
            .await
    }

    async fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.call(reqwest::Method::PUT, path, &[], Some(body)).await
    }
}

pub struct IngestConfig {
    /// Lidarr quality profile (by name) for artists songden adds.
    pub quality_profile: String,
    pub lidarr_url: String,
    pub lidarr_key: String,
    pub prowlarr_url: String,
    pub prowlarr_key: String,
    pub music_root: String,
}

#[derive(Clone)]
pub struct Ingester {
    tx: mpsc::UnboundedSender<Vec<Track>>,
    albums: Arc<Mutex<JsonStore<BTreeMap<String, AlbumRecord>>>>,
}

impl Ingester {
    pub fn start(cfg: IngestConfig, state_dir: &Path, spotify: Spotify) -> Result<Self> {
        let client = reqwest::Client::builder().timeout(API_TIMEOUT).build()?;
        let albums = Arc::new(Mutex::new(JsonStore::load(state_dir, "albums.json")));
        let (tx, rx) = mpsc::unbounded_channel();
        let worker = Worker {
            lidarr: Arr {
                client: client.clone(),
                url: cfg.lidarr_url,
                key: cfg.lidarr_key,
            },
            prowlarr: Arr {
                client,
                url: cfg.prowlarr_url.clone(),
                key: cfg.prowlarr_key,
            },
            prowlarr_url: cfg.prowlarr_url,
            music_root: cfg.music_root,
            quality_profile: cfg.quality_profile,
            spotify,
            albums_of: JsonStore::load(state_dir, "track-albums.json"),
            albums: albums.clone(),
        };
        tokio::spawn(worker.run(rx));
        Ok(Self { tx, albums })
    }

    /// Queue songs that are not in the library yet.
    pub fn kick(&self, tracks: Vec<Track>) {
        if self.tx.send(tracks).is_err() {
            warn!("ingest: worker has stopped; songs not queued");
        }
    }

    /// Lets albums that were not found, or whose download vanished, be tried
    /// again on the next kick without waiting out `RETRY_AFTER` (the grown-ups'
    /// "check now"). A download still in Lidarr's queue is left alone: the
    /// worker checks the queue before retrying.
    pub fn retry_failed(&self) -> Result<()> {
        let mut albums = self
            .albums
            .lock()
            .map_err(|_| Error::Internal("album states lock poisoned".into()))?;
        for record in albums.data.values_mut() {
            if record.state != AlbumState::Imported {
                record.tried = 0;
            }
        }
        albums.save()
    }

    pub fn albums(&self) -> BTreeMap<String, AlbumRecord> {
        self.albums
            .lock()
            .map(|a| a.data.clone())
            .unwrap_or_default()
    }
}

struct Worker {
    lidarr: Arr,
    prowlarr: Arr,
    prowlarr_url: String,
    music_root: String,
    quality_profile: String,
    spotify: Spotify,
    albums_of: JsonStore<BTreeMap<String, TrackAlbum>>,
    albums: Arc<Mutex<JsonStore<BTreeMap<String, AlbumRecord>>>>,
}

impl Worker {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Vec<Track>>) {
        while let Some(first) = rx.recv().await {
            // Everything queued meanwhile goes into the same batch.
            let mut batch: BTreeMap<String, Track> =
                first.into_iter().map(|t| (t.uri.clone(), t)).collect();
            while let Ok(more) = rx.try_recv() {
                batch.extend(more.into_iter().map(|t| (t.uri.clone(), t)));
            }
            self.process(batch.into_values().collect()).await;
        }
    }

    async fn process(&mut self, tracks: Vec<Track>) {
        let mut wanted: BTreeMap<String, TrackAlbum> = BTreeMap::new();
        for t in tracks {
            let info = match self.albums_of.data.get(&t.uri) {
                Some(info) => info.clone(),
                None => {
                    let found = self.spotify.track_album(&t.uri).await;
                    tokio::time::sleep(SPOTIFY_GAP).await;
                    match found {
                        Ok(info) => {
                            self.albums_of.data.insert(t.uri.clone(), info.clone());
                            if let Err(err) = self.albums_of.save() {
                                warn!("ingest: could not save song albums: {err}");
                            }
                            info
                        }
                        Err(err) => {
                            warn!("ingest: album of {:?} unknown: {err}", t.title);
                            continue;
                        }
                    }
                }
            };
            let key = format!("{}|{}", info.album, info.year.as_deref().unwrap_or(""));
            wanted.entry(key).or_insert(info);
        }
        for (key, info) in wanted {
            let rec = self
                .albums
                .lock()
                .ok()
                .and_then(|a| a.data.get(&key).cloned());
            if let Some(rec) = &rec {
                match rec.state {
                    AlbumState::Imported => continue,
                    AlbumState::Downloading if self.still_downloading(&key, rec).await => continue,
                    _ if now().saturating_sub(rec.tried) < RETRY_AFTER.as_secs() => continue,
                    _ => {}
                }
            }
            self.fetch_album(&key, &info).await;
        }
    }

    fn record(
        &self,
        key: &str,
        state: AlbumState,
        album: &str,
        lidarr_album_id: Option<i64>,
        note: &str,
    ) {
        info!("ingest: {key}: {state:?} {note}");
        if let Ok(mut albums) = self.albums.lock() {
            albums.data.insert(
                key.to_string(),
                AlbumRecord {
                    state,
                    album: album.to_string(),
                    lidarr_album_id,
                    note: note.to_string(),
                    tried: now(),
                },
            );
            if let Err(err) = albums.save() {
                warn!("ingest: could not save album states: {err}");
            }
        }
    }

    /// Re-checks an album marked downloading. False once it has vanished from
    /// Lidarr's queue without files, so it gets retried.
    async fn still_downloading(&self, key: &str, rec: &AlbumRecord) -> bool {
        let Some(id) = rec.lidarr_album_id else {
            return false;
        };
        match self.lidarr.get(&format!("/album/{id}"), &[]).await {
            Ok(album) if album["statistics"]["trackFileCount"].as_u64().unwrap_or(0) > 0 => {
                self.record(key, AlbumState::Imported, &rec.album, Some(id), "");
                true
            }
            Ok(_) => self.queued(id).await.unwrap_or(true),
            Err(err) => {
                warn!("ingest: {key}: could not re-check: {err}");
                true // unknown: leave it for the next kick
            }
        }
    }

    async fn fetch_album(&self, key: &str, info: &TrackAlbum) {
        let album = match self.catalogue_album(info).await {
            Ok(Some(album)) => album,
            Ok(None) => {
                return self.record(
                    key,
                    AlbumState::Failed,
                    &info.album,
                    None,
                    "not in Lidarr's catalogue",
                );
            }
            Err(err) => {
                return self.record(
                    key,
                    AlbumState::Failed,
                    &info.album,
                    None,
                    &format!("Lidarr lookup failed: {err}"),
                );
            }
        };
        let id = album["id"].as_i64();
        let title = album["title"].as_str().unwrap_or(&info.album).to_string();
        if album["statistics"]["trackFileCount"].as_u64().unwrap_or(0) > 0 {
            return self.record(key, AlbumState::Imported, &title, id, "already downloaded");
        }
        let Some(id) = id else {
            return self.record(
                key,
                AlbumState::Failed,
                &title,
                None,
                "Lidarr gave no album id",
            );
        };
        let grabbed = match self.lidarr_search(id).await {
            Ok(true) => true,
            Ok(false) => self.fallback(&album, info).await,
            Err(err) => {
                warn!("ingest: {key}: Lidarr search failed: {err}");
                self.fallback(&album, info).await
            }
        };
        if grabbed {
            self.record(key, AlbumState::Downloading, &title, Some(id), "");
        } else {
            self.record(
                key,
                AlbumState::Failed,
                &title,
                Some(id),
                "no release found",
            );
        }
    }

    /// The album in Lidarr, added (and monitored) if it wasn't there.
    async fn catalogue_album(&self, info: &TrackAlbum) -> Result<Option<Value>> {
        let found = self
            .lidarr
            .get("/album/lookup", &[("term", &info.album)])
            .await?;
        let Some(cand) = found
            .as_array()
            .and_then(|c| pick_catalogue(info, c))
            .cloned()
        else {
            return Ok(None);
        };
        let foreign = cand["foreignAlbumId"].as_str().unwrap_or_default();
        let existing = self
            .lidarr
            .get("/album", &[("foreignAlbumId", foreign)])
            .await?;
        if let Some(album) = existing.as_array().and_then(|a| a.first()).cloned() {
            if !album["monitored"].as_bool().unwrap_or(false) {
                self.lidarr
                    .put(
                        "/album/monitor",
                        json!({ "albumIds": [album["id"]], "monitored": true }),
                    )
                    .await?;
            }
            self.monitor_artist(&album).await?;
            return Ok(Some(album));
        }
        let artist_id = cand["artist"]["foreignArtistId"]
            .as_str()
            .unwrap_or_default();
        let artists = self.lidarr.get("/artist", &[]).await?;
        let artist = match artists
            .as_array()
            .and_then(|a| a.iter().find(|x| x["foreignArtistId"] == artist_id))
        {
            Some(artist) => artist.clone(),
            None => {
                let quality = self.quality_profile().await?;
                let metadata = self.lidarr.get("/metadataprofile", &[]).await?[0]["id"].clone();
                let mut artist = cand["artist"].clone();
                artist["qualityProfileId"] = quality;
                artist["metadataProfileId"] = metadata;
                artist["rootFolderPath"] = json!(self.music_root);
                artist["monitored"] = json!(true);
                artist["monitorNewItems"] = json!("none");
                artist["addOptions"] =
                    json!({ "monitor": "none", "searchForMissingAlbums": false });
                artist
            }
        };
        let mut body = cand;
        body["monitored"] = json!(true);
        body["artist"] = artist;
        body["addOptions"] = json!({ "searchForNewAlbum": false });
        let album = self.lidarr.post("/album", body).await?;
        self.monitor_artist(&album).await?;
        Ok(Some(album))
    }

    /// The id of the configured quality profile; an error naming the ones that
    /// exist if it isn't there.
    async fn quality_profile(&self) -> Result<Value> {
        let profiles = self.lidarr.get("/qualityprofile", &[]).await?;
        let list = profiles.as_array().cloned().unwrap_or_default();
        list.iter()
            .find(|p| p["name"].as_str() == Some(self.quality_profile.as_str()))
            .map(|p| p["id"].clone())
            .ok_or_else(|| {
                let names: Vec<&str> = list.iter().filter_map(|p| p["name"].as_str()).collect();
                Error::Internal(format!(
                    "Lidarr has no quality profile {:?} (it has {names:?}); set LIDARR_QUALITY_PROFILE",
                    self.quality_profile
                ))
            })
    }

    /// Makes sure the album's artist is monitored. Lidarr refuses every release
    /// for an unmonitored artist ("Artist is not monitored"), and adding one with
    /// "monitor no albums" leaves it unmonitored. Its other albums stay
    /// unmonitored: `monitorNewItems` is none, and only albums asked for are
    /// monitored, so this downloads nothing else.
    async fn monitor_artist(&self, album: &Value) -> Result<()> {
        let Some(artist_id) = album["artistId"].as_i64() else {
            return Ok(());
        };
        let mut artist = self
            .lidarr
            .get(&format!("/artist/{artist_id}"), &[])
            .await?;
        if artist["monitored"].as_bool() != Some(true) {
            artist["monitored"] = json!(true);
            artist["monitorNewItems"] = json!("none");
            self.lidarr
                .put(&format!("/artist/{artist_id}"), artist)
                .await?;
            info!("ingest: now monitoring artist {artist_id} (no other albums)");
        }
        Ok(())
    }

    /// Lidarr's own search; true if it grabbed a release.
    async fn lidarr_search(&self, album_id: i64) -> Result<bool> {
        let cmd = self
            .lidarr
            .post(
                "/command",
                json!({ "name": "AlbumSearch", "albumIds": [album_id] }),
            )
            .await?;
        let cmd_id = cmd["id"].as_i64().unwrap_or_default();
        let deadline = tokio::time::Instant::now() + LIDARR_SEARCH_WAIT;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(LIDARR_POLL).await;
            let status = self.lidarr.get(&format!("/command/{cmd_id}"), &[]).await?;
            if matches!(
                status["status"].as_str(),
                Some("completed" | "failed" | "aborted")
            ) {
                break;
            }
        }
        self.queued(album_id).await
    }

    async fn queued(&self, album_id: i64) -> Result<bool> {
        let queue = self.lidarr.get("/queue", &[("pageSize", "200")]).await?;
        Ok(queue["records"]
            .as_array()
            .is_some_and(|r| r.iter().any(|q| q["albumId"].as_i64() == Some(album_id))))
    }

    /// Offers indexer releases named like Spotify's album to Lidarr; true if one was taken.
    async fn fallback(&self, album: &Value, info: &TrackAlbum) -> bool {
        // The full name first; uploaders rarely spell out "Original Motion Picture
        // Soundtrack / Deluxe Edition" though, and indexers want every word, so
        // if that finds nothing usable, search by the identifying words alone.
        let mut results = Vec::new();
        for query in search_queries(info) {
            match self
                .prowlarr
                .get(
                    "/search",
                    &[
                        ("query", &query),
                        ("type", "search"),
                        ("categories", "3000"),
                    ],
                )
                .await
            {
                Ok(found) => results = found.as_array().cloned().unwrap_or_default(),
                Err(err) => {
                    warn!("ingest: indexer search {query:?} failed: {err}");
                    continue;
                }
            }
            if !rank_releases(
                &info.album,
                first_artist(info),
                info.year.as_deref(),
                &results,
            )
            .is_empty()
            {
                break;
            }
            info!("ingest: nothing usable for {query:?}");
        }
        let artist = album["artist"]["artistName"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| {
                info.artists
                    .split(',')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            });
        let title = album["title"].as_str().unwrap_or(&info.album);
        let year = album["releaseDate"]
            .as_str()
            .and_then(|d| d.get(..4))
            .or(info.year.as_deref())
            .unwrap_or("");
        for r in rank_releases(
            &info.album,
            first_artist(info),
            info.year.as_deref(),
            &results,
        )
        .into_iter()
        .take(FALLBACK_TRIES)
        {
            let raw_title = r["title"].as_str().unwrap_or_default();
            let clean = clean_title(&artist, title, year);
            let push = push_body(r, &clean, &self.prowlarr_url);
            match self.lidarr.post("/release/push", push).await {
                Ok(out) => {
                    let decision = if out.is_array() { out[0].clone() } else { out };
                    if decision["approved"].as_bool() == Some(true) {
                        info!("ingest: fallback took {raw_title:?} as {clean:?}");
                        return true;
                    }
                    info!(
                        "ingest: fallback {raw_title:?} rejected: {}",
                        decision["rejections"]
                    );
                }
                Err(err) => warn!("ingest: push {clean:?} refused: {err}"),
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(name: &str, year: &str) -> TrackAlbum {
        TrackAlbum {
            album: name.into(),
            year: Some(year.into()),
            artists: String::new(),
        }
    }

    #[test]
    fn catalogue_match_needs_name_and_year() {
        let candidates = vec![
            json!({"title": "Mouna Geethangal (Original Motion Picture Soundtrack)", "releaseDate": "1981-01-01"}),
            json!({"title": "Moana 2 (Original Score)", "releaseDate": "2025-01-01"}),
            json!({"title": "Moana: Original Motion Picture Soundtrack", "releaseDate": "2016-11-18"}),
        ];
        let info = album(
            "Moana (Original Motion Picture Soundtrack/Deluxe Edition)",
            "2016",
        );
        let picked = pick_catalogue(&info, &candidates).unwrap();
        assert_eq!(picked["title"], "Moana: Original Motion Picture Soundtrack");
        let wrong_year = album(
            "Moana (Original Motion Picture Soundtrack/Deluxe Edition)",
            "1990",
        );
        assert!(pick_catalogue(&wrong_year, &candidates).is_none());
    }

    #[test]
    fn releases_are_mp3_standard_editions_first() {
        let results = vec![
            json!({"title": "Moana 2 (Original Motion Picture Soundtrack) (2024) Mp3 320kbps", "seeders": 19}),
            json!({"title": "Various Artists Moana 2 (Russian Original Motion Picture Soundtrack) (2024) Mp3", "seeders": 1}),
            json!({"title": "Moana (Russian Original Motion Picture Soundtrack) Mp3", "seeders": 30}),
            json!({"title": "Moana (Original Motion Picture Soundtrack) (2020) Mp3 [PMEDIA]", "seeders": 8}),
            json!({"title": "Moana Original Motion Picture Soundtrack (Deluxe Edition) (2017) Mp3 320", "seeders": 25}),
            json!({"title": "VA Moana Original Motion Picture Soundtrack (Deluxe Edition) (2017) [24 96 FLAC]", "seeders": 40}),
            json!({"title": "Moana (Original Motion Picture Soundtrack) FLAC", "seeders": 50}),
            json!({"title": "Moana Soundtrack Mp3 dead swarm", "seeders": 0}),
        ];
        let ranked = rank_releases(
            "Moana (Original Motion Picture Soundtrack/Deluxe Edition)",
            "Auli'i Cravalho",
            Some("2016"),
            &results,
        );
        let titles: Vec<&str> = ranked
            .iter()
            .map(|r| r["title"].as_str().unwrap())
            .collect();
        // the standard MP3 first even though Spotify's album is the deluxe one and
        // the deluxe MP3 has more seeders; no FLAC, sequel, other language or dead swarm
        assert_eq!(
            titles,
            [
                "Moana (Original Motion Picture Soundtrack) (2020) Mp3 [PMEDIA]",
                "Moana Original Motion Picture Soundtrack (Deluxe Edition) (2017) Mp3 320",
            ]
        );
    }

    #[test]
    fn prowlarr_links_reach_prowlarr_and_redirects_go_as_downloads() {
        let p = "http://prowlarr:9696";
        // Pirate Bay through Prowlarr: the "magnet" is an http link to Prowlarr
        let tpb = json!({"magnetUrl": "http://localhost:9696/2/download?link=x", "title": "t"});
        let body = push_body(&tpb, "A - B (2016) [MP3-320]", p);
        assert_eq!(
            body["downloadUrl"],
            "http://prowlarr:9696/2/download?link=x"
        );
        assert!(body["magnetUrl"].is_null());
        // a plain torrent link
        let lime = json!({"downloadUrl": "http://localhost:9696/3/download?x", "title": "t"});
        assert_eq!(
            push_body(&lime, "t", p)["downloadUrl"],
            "http://prowlarr:9696/3/download?x"
        );
        // a real magnet stays a magnet
        let real = json!({"magnetUrl": "magnet:?xt=urn:btih:abc", "title": "t"});
        let body = push_body(&real, "t", p);
        assert_eq!(body["magnetUrl"], "magnet:?xt=urn:btih:abc");
        assert!(body["downloadUrl"].is_null());
    }

    #[test]
    fn offered_names_carry_a_quality_lidarr_recognises() {
        let t = clean_title(
            "Lin‐Manuel Miranda",
            "Moana: Original Motion Picture Soundtrack",
            "2016",
        );
        assert_eq!(
            t,
            "Lin‐Manuel Miranda - Moana: Original Motion Picture Soundtrack (2016) [MP3-320]"
        );
    }

    #[test]
    fn lossy_releases_count_even_without_saying_mp3() {
        let results = vec![
            json!({"title": "Brother.Bear.1CD.2003.Soundtrack.[WmC]", "seeders": 5}),
            json!({"title": "Brother Bear - Soundtrack 320cbr (Big Papi)", "seeders": 1}),
            json!({"title": "Brother Bear Soundtrack [24-96 Hi-Res]", "seeders": 9}),
        ];
        let ranked = rank_releases("Brother Bear", "Phil Collins", None, &results);
        let titles: Vec<&str> = ranked
            .iter()
            .map(|r| r["title"].as_str().unwrap())
            .collect();
        // the 320 one says it is MP3, so it beats the unlabelled one despite fewer
        // seeders; the hi-res one is lossless and never offered
        assert_eq!(
            titles,
            [
                "Brother Bear - Soundtrack 320cbr (Big Papi)",
                "Brother.Bear.1CD.2003.Soundtrack.[WmC]"
            ]
        );
    }

    #[test]
    fn a_second_search_drops_the_boilerplate() {
        let frozen = TrackAlbum {
            album: "Frozen (Original Motion Picture Soundtrack / Deluxe Edition)".into(),
            year: Some("2013".into()),
            artists: "Idina Menzel".into(),
        };
        assert_eq!(
            search_queries(&frozen),
            [
                "frozen original motion picture soundtrack deluxe edition",
                "frozen soundtrack"
            ]
        );
        let single = TrackAlbum {
            album: "I Just Might".into(),
            year: None,
            artists: "Bruno Mars".into(),
        };
        assert_eq!(
            search_queries(&single),
            ["i just might", "i just might bruno mars"]
        );
    }

    #[test]
    fn a_self_titled_album_is_not_the_artists_other_albums() {
        let results = vec![
            json!({"title": "The Lumineers - Automatic (2025) Mp3 320kbps [PMEDIA]", "seeders": 36}),
            json!({"title": "The Lumineers  BRIGHTSIDE (2022) Mp3 320kbps [PMEDIA]", "seeders": 15}),
            json!({"title": "The Lumineers - The Lumineers (2012) MP3 320", "seeders": 3}),
        ];
        let ranked = rank_releases("The Lumineers", "The Lumineers", Some("2012"), &results);
        let titles: Vec<&str> = ranked
            .iter()
            .map(|r| r["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["The Lumineers - The Lumineers (2012) MP3 320"]);
    }

    #[test]
    fn another_title_that_starts_with_the_name_is_not_the_album() {
        let frozen = "Frozen (Original Motion Picture Soundtrack / Deluxe Edition)";
        assert!(!names_the_album(
            frozen,
            "Hans Zimmer  Frozen Planet II (Original Television Soundtrack) (2022) Mp3 320kbps"
        ));
        assert!(names_the_album(
            frozen,
            "Frozen (Original Motion Picture Soundtrack) (2013) Mp3 320kbps"
        ));
        assert!(names_the_album(
            "Hercules (Original Motion Picture Soundtrack)",
            "Hercules Soundtrack (1997)"
        ));
        assert!(names_the_album(
            "Brother Bear",
            "Brother.Bear.1CD.2003.Soundtrack.[WmC]"
        ));
        assert!(names_the_album(
            "The Lumineers",
            "The Lumineers - The Lumineers 2012 Songwriter 320kbps CBR MP3"
        ));
        assert!(names_the_album(
            "Moana (Original Motion Picture Soundtrack/Deluxe Edition)",
            "Moana (Original Motion Picture Soundtrack) (2020) Mp3 [PMEDIA]"
        ));
        assert!(names_the_album(
            "Wish (Original Motion Picture Soundtrack)",
            "Julia Michaels  Wish (Original Motion Picture Soundtrack) (2023) Mp3 320kbps"
        ));
    }

    #[test]
    fn frozen_planet_is_not_offered_for_frozen() {
        let results = vec![
            json!({"title": "Hans Zimmer  Frozen Planet II (Original Television Soundtrack) (2022) Mp3 320kbps [PMEDIA]", "seeders": 30}),
            json!({"title": "Frozen (Original Motion Picture Soundtrack) (2013) Mp3 320kbps", "seeders": 4}),
        ];
        let ranked = rank_releases(
            "Frozen (Original Motion Picture Soundtrack / Deluxe Edition)",
            "Idina Menzel",
            Some("2013"),
            &results,
        );
        let titles: Vec<&str> = ranked
            .iter()
            .map(|r| r["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            titles,
            ["Frozen (Original Motion Picture Soundtrack) (2013) Mp3 320kbps"]
        );
    }

    #[test]
    fn lossless_is_never_offered() {
        let results = vec![
            json!({"title": "Lin Manuel Miranda Encanto (Original Motion Picture Soundtrack) (2021) FLAC", "seeders": 18}),
            json!({"title": "Encanto (Original Motion Picture Soundtrack) [24-96 FLAC]", "seeders": 9}),
        ];
        assert!(
            rank_releases(
                "Encanto (Original Motion Picture Soundtrack)",
                "Lin-Manuel Miranda",
                None,
                &results
            )
            .is_empty()
        );
    }
}
