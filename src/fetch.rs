//! Gets playlist songs onto disk, one song at a time, from YouTube with yt-dlp.
//!
//! Runs only when the player notices songs it doesn't have (`kick`), never on a
//! timer. For each song: search YouTube Music's songs for its title and first
//! artist (then plain YouTube if that finds nothing), take the first result that
//! is really the song (`choose`), download its audio, and fetch the cover from
//! the song's Spotify page. Songs are stored as `<spotify song id>.<ext>` with
//! `<id>.jpg` beside them, so a playlist knows which of its songs are here.
//!
//! A song that can't be found is remembered with the reason and retried on a
//! later kick, at most every `RETRY_AFTER`, or at once when a grown-up asks.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{process::Command, sync::mpsc};
use tracing::{info, warn};

use crate::{
    Error, Result,
    spotify::{Spotify, Track, TrackPage},
    store::JsonStore,
    text::norm,
};

const RETRY_AFTER: Duration = Duration::from_secs(6 * 3600);
/// Results looked at per search.
const SEARCH_RESULTS: u32 = 6;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(120);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const TAG_TIMEOUT: Duration = Duration::from_secs(60);
/// How far a result's length may be from Spotify's: this many seconds, or this
/// share of the song if that is more.
const LENGTH_SLACK_S: u64 = 3;
const LENGTH_SLACK_SHARE: f64 = 0.02;
/// Audio extensions songs may be saved as, preferred first.
pub const AUDIO_EXTS: &[&str] = &["m4a", "webm", "opus", "mp3"];
/// Versions of a song a result may only be if the Spotify title asks for it.
const VERSIONS: &[&str] = &[
    "live",
    "cover",
    "karaoke",
    "instrumental",
    "remix",
    "sped",
    "slowed",
    "nightcore",
    "8d",
    "acoustic",
    "reverb",
    "lofi",
    "piano",
    "orchestral",
    "demo",
    "reprise",
    "mashup",
    "parody",
    // another language's version, unless the Spotify title names it too
    "tagalog",
    "filipino",
    "spanish",
    "latino",
    "castellano",
    "french",
    "italian",
    "german",
    "portuguese",
    "brazilian",
    "japanese",
    "korean",
    "mandarin",
    "cantonese",
    "chinese",
    "thai",
    "vietnamese",
    "indonesian",
    "malay",
    "russian",
    "polish",
    "dutch",
    "hindi",
];
/// Artist names that every dub carries too, so they don't identify the singer.
const GENERIC_ARTISTS: &[&str] = &[
    "disney",
    "various artists",
    "cast",
    "soundtrack",
    "original cast",
];
/// Words that say nothing about which song a title is.
const FLUFF: &[&str] = &[
    "remastered",
    "remaster",
    "version",
    "soundtrack",
    "from",
    "original",
    "motion",
    "picture",
    "edit",
    "radio",
    "mono",
    "stereo",
    "official",
    "audio",
    "video",
    "lyrics",
    "lyric",
    "the",
    "a",
    "and",
    "of",
    "hd",
    "hq",
    "film",
    "movie",
    "disney",
    "single",
];

/// One search result.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub url: String,
    pub title: String,
    pub seconds: Option<u64>,
    /// Who it's by: YouTube Music's artists, or the channel ("X - Topic" as X).
    pub artists: Vec<String>,
}

impl Candidate {
    fn from_json(v: &Value) -> Option<Self> {
        let url = v["webpage_url"]
            .as_str()
            .or_else(|| v["url"].as_str())
            .map(String::from)?;
        let mut artists: Vec<String> = match v["artists"].as_array() {
            Some(list) => list
                .iter()
                .filter_map(|a| a.as_str())
                .map(String::from)
                .collect(),
            None => v["artist"]
                .as_str()
                .map(|a| a.split(", ").map(String::from).collect())
                .unwrap_or_default(),
        };
        for key in ["channel", "uploader"] {
            if let Some(name) = v[key].as_str() {
                artists.push(name.trim_end_matches(" - Topic").to_string());
            }
        }
        Some(Self {
            url,
            title: v["title"].as_str()?.to_string(),
            seconds: v["duration"].as_f64().map(|d| d.round() as u64),
            artists,
        })
    }
}

fn words(s: &str) -> BTreeSet<String> {
    norm(s)
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

fn telling(words: &BTreeSet<String>) -> BTreeSet<String> {
    words
        .iter()
        .filter(|w| !FLUFF.contains(&w.as_str()) && !w.chars().all(|c| c.is_ascii_digit()))
        .cloned()
        .collect()
}

/// Whether a search result is this Spotify song:
///
/// - its length is within `LENGTH_SLACK_S` (or `LENGTH_SLACK_SHARE`) of
///   Spotify's — the strongest signal, since covers, live takes and edits run
///   different lengths. A result without a length only passes if the titles
///   agree exactly.
/// - it isn't a version (live, cover, karaoke, sped up…) the title didn't ask for.
/// - most of the words that tell titles apart agree.
/// - it is by one of Spotify's artists (`same_singer`): a dub is timed to the
///   same scene, so it runs the same length and sometimes keeps the title, but
///   it is sung by someone else.
pub fn is_the_song(track: &Track, c: &Candidate) -> bool {
    if !same_singer(track, c) {
        return false;
    }
    let want = words(&track.title);
    let have = words(&c.title);
    if VERSIONS
        .iter()
        .any(|v| have.contains(*v) && !want.contains(*v))
    {
        return false;
    }
    let (want_t, have_t) = (telling(&want), telling(&have));
    let union = want_t.union(&have_t).count();
    let shared = want_t.intersection(&have_t).count();
    let titles_agree = union > 0 && shared as f64 / union as f64 >= 0.6 && shared > 0;
    match (track.seconds, c.seconds) {
        (0, _) | (_, None) => norm(&track.title) == norm(&c.title),
        (want_s, Some(have_s)) => {
            let slack = LENGTH_SLACK_S.max((want_s as f64 * LENGTH_SLACK_SHARE).round() as u64);
            titles_agree && want_s.abs_diff(have_s) <= slack
        }
    }
}

/// Whether one of Spotify's artists (other than a label like "Disney") is among
/// the result's artists or names its channel. Names are compared folded, and one
/// may contain the other ("Carolina Gaitán - La Gaita" and "Carolina Gaitán").
pub fn same_singer(track: &Track, c: &Candidate) -> bool {
    let theirs: Vec<String> = c
        .artists
        .iter()
        .map(|a| norm(a))
        .filter(|a| !a.is_empty())
        .collect();
    let ours: Vec<String> = track
        .artists
        .split(',')
        .map(norm)
        .filter(|a| a.len() >= 3 && !GENERIC_ARTISTS.contains(&a.as_str()))
        .collect();
    if ours.is_empty() {
        return true; // nothing to tell singers apart by
    }
    ours.iter().any(|o| {
        theirs
            .iter()
            .any(|t| t == o || t.contains(o.as_str()) || o.contains(t.as_str()) && t.len() >= 3)
    })
}

/// The first result that is the song, in the search's own order.
pub fn choose<'a>(track: &Track, candidates: &'a [Candidate]) -> Option<&'a Candidate> {
    candidates.iter().find(|c| is_the_song(track, c))
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum SongState {
    #[default]
    NotFound,
}

/// A song that could not be fetched.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Failure {
    pub state: SongState,
    pub title: String,
    pub artist: String,
    pub note: String,
    /// Unix seconds of the last attempt.
    pub tried: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Where songs live: `<id>.<ext>` and `<id>.jpg`.
#[derive(Clone)]
pub struct Library {
    dir: PathBuf,
}

impl Library {
    pub fn new(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// The song's audio file, if it has been fetched.
    pub fn audio(&self, id: &str) -> Option<PathBuf> {
        AUDIO_EXTS
            .iter()
            .map(|ext| self.dir.join(format!("{id}.{ext}")))
            .find(|p| p.is_file())
    }

    pub fn cover(&self, id: &str) -> Option<PathBuf> {
        Some(self.dir.join(format!("{id}.jpg"))).filter(|p| p.is_file())
    }

    fn part(&self, id: &str) -> PathBuf {
        self.dir.join(format!(".part-{id}"))
    }
}

#[derive(Clone)]
pub struct Fetcher {
    tx: mpsc::UnboundedSender<Vec<Track>>,
    failures: Arc<Mutex<JsonStore<BTreeMap<String, Failure>>>>,
    queued: Arc<Mutex<BTreeMap<String, Track>>>,
}

impl Fetcher {
    pub fn start(library: Library, ytdlp: String, state_dir: &Path, spotify: Spotify) -> Self {
        let failures = Arc::new(Mutex::new(JsonStore::load(state_dir, "failures.json")));
        let queued = Arc::new(Mutex::new(BTreeMap::new()));
        let (tx, rx) = mpsc::unbounded_channel();
        let worker = Worker {
            library,
            ytdlp,
            spotify,
            failures: failures.clone(),
            queued: queued.clone(),
        };
        tokio::spawn(worker.run(rx));
        Self {
            tx,
            failures,
            queued,
        }
    }

    /// Queue songs that aren't on disk yet.
    pub fn kick(&self, tracks: Vec<Track>) {
        if self.tx.send(tracks).is_err() {
            warn!("fetch: worker has stopped; songs not queued");
        }
    }

    /// Lets songs that weren't found be tried again at once (a grown-up's check).
    pub fn retry_failed(&self) -> Result<()> {
        let mut failures = self
            .failures
            .lock()
            .map_err(|_| Error::Internal("failures lock poisoned".into()))?;
        for f in failures.data.values_mut() {
            f.tried = 0;
        }
        failures.save()
    }

    /// Songs waiting their turn or being fetched, and songs not found (by id).
    pub fn status(&self) -> (Vec<Track>, Vec<(String, Failure)>) {
        let queued = self
            .queued
            .lock()
            .map(|q| q.values().cloned().collect())
            .unwrap_or_default();
        let failed = self
            .failures
            .lock()
            .map(|f| {
                f.data
                    .iter()
                    .map(|(id, f)| (id.clone(), f.clone()))
                    .collect()
            })
            .unwrap_or_default();
        (queued, failed)
    }
}

struct Worker {
    library: Library,
    ytdlp: String,
    spotify: Spotify,
    failures: Arc<Mutex<JsonStore<BTreeMap<String, Failure>>>>,
    queued: Arc<Mutex<BTreeMap<String, Track>>>,
}

impl Worker {
    async fn run(self, mut rx: mpsc::UnboundedReceiver<Vec<Track>>) {
        while let Some(first) = rx.recv().await {
            let mut batch: Vec<Track> = first;
            while let Ok(more) = rx.try_recv() {
                batch.extend(more);
            }
            let untagged: Vec<Track> = {
                let mut seen = BTreeSet::new();
                batch
                    .iter()
                    .filter(|t| t.id().is_some_and(|id| seen.insert(id.to_string())))
                    .filter(|t| {
                        t.id().is_some_and(|id| {
                            self.library.audio(id).is_some() && !self.is_tagged(id)
                        })
                    })
                    .cloned()
                    .collect()
            };
            for track in untagged {
                let id = track.id().unwrap_or_default().to_string();
                if let Err(err) = self.describe(&track, &id).await {
                    warn!("fetch: could not tag {:?}: {err}", track.title);
                }
            }
            let todo: Vec<Track> = {
                let failures = self
                    .failures
                    .lock()
                    .map(|f| f.data.clone())
                    .unwrap_or_default();
                let mut seen = BTreeSet::new();
                batch
                    .into_iter()
                    .filter(|t| t.id().is_some_and(|id| seen.insert(id.to_string())))
                    .filter(|t| t.id().is_some_and(|id| self.library.audio(id).is_none()))
                    .filter(|t| {
                        failures
                            .get(t.id().unwrap_or_default())
                            .is_none_or(|f| now().saturating_sub(f.tried) >= RETRY_AFTER.as_secs())
                    })
                    .collect()
            };
            if let Ok(mut q) = self.queued.lock() {
                for t in &todo {
                    q.insert(t.id().unwrap_or_default().to_string(), t.clone());
                }
            }
            for track in todo {
                let id = track.id().unwrap_or_default().to_string();
                match self.fetch(&track, &id).await {
                    Ok(()) => self.forget_failure(&id),
                    Err(err) => self.fail(&track, &id, &err.to_string()),
                }
                if let Ok(mut q) = self.queued.lock() {
                    q.remove(&id);
                }
            }
        }
    }

    fn forget_failure(&self, id: &str) {
        if let Ok(mut f) = self.failures.lock()
            && f.data.remove(id).is_some()
            && let Err(err) = f.save()
        {
            warn!("fetch: could not save failures: {err}");
        }
    }

    fn fail(&self, track: &Track, id: &str, note: &str) {
        info!("fetch: {:?} not fetched: {note}", track.title);
        if let Ok(mut f) = self.failures.lock() {
            f.data.insert(
                id.to_string(),
                Failure {
                    state: SongState::NotFound,
                    title: track.title.clone(),
                    artist: track.first_artist().to_string(),
                    note: note.to_string(),
                    tried: now(),
                },
            );
            if let Err(err) = f.save() {
                warn!("fetch: could not save failures: {err}");
            }
        }
    }

    async fn fetch(&self, track: &Track, id: &str) -> Result<()> {
        let query = format!("{} {}", track.title, track.first_artist());
        let music =
            reqwest::Url::parse_with_params("https://music.youtube.com/search", &[("q", &query)])
                .map(|mut u| {
                    u.set_fragment(Some("songs"));
                    u.to_string()
                })
                .map_err(|e| Error::Internal(e.to_string()))?;
        let plain = format!("ytsearch{SEARCH_RESULTS}:{query}");
        let mut picked = None;
        for search in [music, plain] {
            let candidates = self.search(&search).await?;
            if let Some(c) = choose(track, &candidates) {
                picked = Some(c.clone());
                break;
            }
        }
        let c = picked.ok_or_else(|| Error::Upstream("no matching song on YouTube".into()))?;
        info!(
            "fetch: {:?} <- {:?} ({:?}s)",
            track.title, c.title, c.seconds
        );
        self.download(id, &c.url).await?;
        // what was used for this song, so a wrong-sounding one can be traced
        let source = serde_json::json!({
            "url": c.url, "title": c.title, "artists": c.artists, "seconds": c.seconds,
            "spotify": { "title": track.title, "artists": track.artists, "seconds": track.seconds },
            "tagged": false,
        });
        self.write_source(id, &source)?;
        self.describe(track, id).await
    }

    fn source_path(&self, id: &str) -> PathBuf {
        self.library.dir.join(format!("{id}.source.json"))
    }

    fn write_source(&self, id: &str, source: &Value) -> Result<()> {
        std::fs::write(self.source_path(id), serde_json::to_vec_pretty(source)?)?;
        Ok(())
    }

    /// Whether the song's file has had its tags written (per its source record).
    fn is_tagged(&self, id: &str) -> bool {
        std::fs::read(self.source_path(id))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|v| v["tagged"].as_bool() == Some(true))
    }

    /// Fetches the cover and writes the tags, then marks the song tagged. Run
    /// after a download, and for songs fetched before tagging existed.
    async fn describe(&self, track: &Track, id: &str) -> Result<()> {
        let page = self.spotify.track_page(track).await.unwrap_or_else(|err| {
            warn!("fetch: no song page for {:?}: {err}", track.title);
            Default::default()
        });
        let cover = self.library.dir.join(format!("{id}.jpg"));
        if let Some(url) = &page.cover {
            match self.spotify.image_bytes(url).await {
                Ok(bytes) => std::fs::write(&cover, bytes)?,
                Err(err) => warn!("fetch: no cover for {:?}: {err}", track.title),
            }
        }
        match self.tag(id, track, &page).await {
            Ok(()) => {
                let mut source = std::fs::read(self.source_path(id))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                source["tagged"] = serde_json::json!(true);
                self.write_source(id, &source)?;
            }
            Err(err) => warn!("fetch: {:?} left untagged: {err}", track.title),
        }
        Ok(())
    }

    /// Writes the song's title, artists, album and year (and the cover, for an
    /// .m4a) into the file, so other players and Jellyfin show it properly. The
    /// audio is copied untouched into a new file that then replaces the old.
    async fn tag(&self, id: &str, track: &Track, page: &TrackPage) -> Result<()> {
        let Some(audio) = self.library.audio(id) else {
            return Ok(());
        };
        let ext = audio
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("m4a")
            .to_string();
        let tagged = self.library.dir.join(format!(".tag-{id}.{ext}"));
        let cover = self.library.cover(id).filter(|_| ext == "m4a");
        let mut args: Vec<String> = vec![
            "-nostdin".into(),
            "-y".into(),
            "-loglevel".into(),
            "error".into(),
        ];
        args.extend(["-i".into(), audio.display().to_string()]);
        if let Some(c) = &cover {
            args.extend(["-i".into(), c.display().to_string()]);
        }
        args.extend(["-map".into(), "0:a".into()]);
        if cover.is_some() {
            args.extend([
                "-map".into(),
                "1:v".into(),
                "-disposition:v:0".into(),
                "attached_pic".into(),
            ]);
        }
        args.extend(["-c".into(), "copy".into()]);
        let tags = [
            ("title", Some(track.title.as_str())),
            ("artist", Some(track.artists.as_str())),
            ("album_artist", Some(track.first_artist())),
            ("album", page.album.as_deref()),
            ("date", page.year.as_deref()),
        ];
        for (key, value) in tags {
            if let Some(value) = value {
                args.extend(["-metadata".into(), format!("{key}={value}")]);
            }
        }
        args.push(tagged.display().to_string());
        let run = Command::new("ffmpeg")
            .args(&args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(TAG_TIMEOUT, run)
            .await
            .map_err(|_| Error::Upstream("ffmpeg timed out".into()))?
            .map_err(|e| Error::Internal(format!("could not run ffmpeg: {e}")))?;
        if !out.status.success() {
            if let Err(e) = std::fs::remove_file(&tagged)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                warn!("fetch: could not remove {}: {e}", tagged.display());
            }
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(Error::Upstream(format!(
                "ffmpeg: {}",
                err.lines().last().unwrap_or("")
            )));
        }
        std::fs::rename(&tagged, &audio)?;
        Ok(())
    }

    async fn ytdlp(&self, args: &[&str], timeout: Duration) -> Result<String> {
        let run = Command::new(&self.ytdlp)
            .args(["--no-update", "--no-warnings"])
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(timeout, run)
            .await
            .map_err(|_| Error::Upstream("yt-dlp timed out".into()))?
            .map_err(|e| Error::Internal(format!("could not run yt-dlp: {e}")))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let last = err
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            return Err(Error::Upstream(format!("yt-dlp: {last}")));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Search results with their lengths (a full lookup per result, so the
    /// length is known).
    async fn search(&self, search: &str) -> Result<Vec<Candidate>> {
        let end = SEARCH_RESULTS.to_string();
        let out = self
            .ytdlp(
                &[
                    "--dump-json",
                    "--skip-download",
                    "--ignore-errors",
                    "--playlist-end",
                    &end,
                    search,
                ],
                SEARCH_TIMEOUT,
            )
            .await
            .or_else(|err| match err {
                // --ignore-errors still exits non-zero when one result fails;
                // the others are on stdout either way
                Error::Upstream(_) => Ok(String::new()),
                other => Err(other),
            })?;
        Ok(out
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| Candidate::from_json(&v))
            .collect())
    }

    /// Downloads the audio to `.part-<id>.<ext>`, then renames it into place so a
    /// half-written file is never taken for a song.
    async fn download(&self, id: &str, url: &str) -> Result<()> {
        let part = self.library.part(id);
        let template = format!("{}.%(ext)s", part.display());
        let out = self
            .ytdlp(
                &[
                    "--no-playlist",
                    "--no-progress",
                    "-f",
                    "140/bestaudio[ext=m4a]/bestaudio",
                    "-o",
                    &template,
                    "--print",
                    "after_move:filepath",
                    url,
                ],
                DOWNLOAD_TIMEOUT,
            )
            .await?;
        let path = PathBuf::from(out.lines().last().unwrap_or_default().trim());
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .filter(|e| AUDIO_EXTS.contains(e))
            .ok_or_else(|| Error::Upstream(format!("unexpected download {}", path.display())))?
            .to_string();
        std::fs::rename(&path, self.library.dir.join(format!("{id}.{ext}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Spotify song; every one also lists "Test Singer", whom `found` results
    /// are by, so tests about titles and lengths aren't about singers.
    fn track(title: &str, artists: &str, seconds: u64) -> Track {
        Track {
            uri: "spotify:track:626IyVO7cgaCTFbjDE3KPB".into(),
            title: title.into(),
            artists: format!("{artists}, Test Singer"),
            explicit: false,
            seconds,
        }
    }

    fn found(title: &str, seconds: Option<u64>) -> Candidate {
        by(title, seconds, &[])
    }

    /// A result by these artists; with none, by whoever the track's first artist is.
    fn by(title: &str, seconds: Option<u64>, artists: &[&str]) -> Candidate {
        Candidate {
            url: format!("https://youtube.com/watch?v={}", title.len()),
            title: title.into(),
            seconds,
            artists: if artists.is_empty() {
                vec!["Test Singer - Topic".trim_end_matches(" - Topic").into()]
            } else {
                artists.iter().map(|a| a.to_string()).collect()
            },
        }
    }

    #[test]
    fn a_dub_with_the_same_title_and_length_is_not_the_song() {
        let song = Track {
            uri: "spotify:track:626IyVO7cgaCTFbjDE3KPB".into(),
            title: "Where You Are".into(),
            artists: "Christopher Jackson, Rachel House, Disney".into(),
            explicit: false,
            seconds: 210,
        };
        // a dub sung by others, carrying the same title and the "Disney" label
        assert!(!is_the_song(
            &song,
            &by("Where You Are", Some(210), &["Luca Velletri", "Disney"])
        ));
        assert!(is_the_song(
            &song,
            &by(
                "Where You Are",
                Some(210),
                &["Christopher Jackson", "Rachel House"]
            )
        ));
        // the channel counts when YouTube Music gives no artists
        assert!(is_the_song(
            &song,
            &by("Where You Are", Some(211), &["Christopher Jackson"])
        ));
        // no way to tell who sings it: not taken
        let unknown = Candidate {
            artists: vec![],
            ..by("Where You Are", Some(210), &["x"])
        };
        assert!(!is_the_song(&song, &unknown));
    }

    #[test]
    fn another_languages_version_must_be_asked_for() {
        let english = track("How Far I'll Go", "Auli'i Cravalho", 163);
        assert!(!is_the_song(
            &english,
            &found("How Far I'll Go (Tagalog Version)", Some(163))
        ));
        let tagalog = track("How Far I'll Go - Tagalog Version", "Janella Salvador", 168);
        assert!(is_the_song(
            &tagalog,
            &found("How Far I'll Go (Tagalog Version)", Some(168))
        ));
    }

    #[test]
    fn the_right_length_and_title_is_the_song() {
        let bruno = track(
            "We Don't Talk About Bruno",
            "Carolina Gaitán - La Gaita, Mauro Castillo",
            216,
        );
        let results = [
            found("Non si nomina Bruno", Some(216)),
            found("We Don't Talk About Bruno (Sped Up)", Some(180)),
            found("We Don\u{2019}t Talk About Bruno", Some(217)),
        ];
        assert_eq!(
            choose(&bruno, &results).unwrap().title,
            "We Don\u{2019}t Talk About Bruno"
        );
    }

    #[test]
    fn versions_must_be_asked_for() {
        let song = track("How Far I'll Go", "Auli'i Cravalho", 163);
        assert!(!is_the_song(
            &song,
            &found("How Far I'll Go (Live at the Hollywood Bowl)", Some(163))
        ));
        assert!(!is_the_song(
            &song,
            &found("How Far I'll Go - Karaoke", Some(163))
        ));
        assert!(!is_the_song(
            &song,
            &found("How Far I'll Go (Reprise)", Some(163))
        ));
        let reprise = track("How Far I'll Go - Reprise", "Auli'i Cravalho", 90);
        assert!(is_the_song(
            &reprise,
            &found("How Far I'll Go (Reprise)", Some(90))
        ));
        let karaoke = track("How Far I'll Go - Instrumental", "Moana Karaoke", 163);
        assert!(is_the_song(
            &karaoke,
            &found("How Far I'll Go (Instrumental)", Some(164))
        ));
    }

    #[test]
    fn a_different_length_is_another_recording() {
        let song = track(
            "Let It Go - From \"Frozen\"/Soundtrack Version",
            "Idina Menzel",
            224,
        );
        assert!(is_the_song(
            &song,
            &found("Let It Go (From \"Frozen\"/Soundtrack Version)", Some(225))
        ));
        assert!(!is_the_song(&song, &found("Let It Go", Some(244))));
    }

    #[test]
    fn titles_must_mostly_agree() {
        let song = track("Try Everything", "Shakira", 196);
        assert!(!is_the_song(&song, &found("Everything Now", Some(196))));
        assert!(is_the_song(
            &song,
            &found("Try Everything (From \"Zootopia\")", Some(196))
        ));
    }

    #[test]
    fn without_lengths_only_an_exact_title_will_do() {
        let song = track("Ho Hey", "The Lumineers", 0);
        assert!(is_the_song(&song, &found("Ho Hey", None)));
        assert!(!is_the_song(&song, &found("Ho Hey (Acoustic)", None)));
        let timed = track("Ho Hey", "The Lumineers", 163);
        assert!(!is_the_song(&timed, &found("Ho Hey Tutorial", None)));
    }
}
