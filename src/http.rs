//! Routes. The page and its assets are embedded in the binary; every `/api` route
//! except sign-in needs a session. Songs and covers are only served when they are
//! on a current playlist, so a signed-in device can reach nothing else.

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use crate::{
    AppState, Error, Result,
    auth::require_session,
    settings::Settings,
    spotify::{is_id, playlist_id},
};

const IMAGE_HEADERS: &[header::HeaderName] = &[header::CONTENT_TYPE, header::CONTENT_LENGTH];

pub fn routes(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/playlists", get(playlists))
        .route("/api/playlist/{id}", get(playlist))
        .route("/api/pcover/{id}", get(playlist_cover))
        .route("/api/audio/{id}", get(audio))
        .route("/api/cover/{id}", get(song_cover))
        .route("/api/parents/status", get(parents_status))
        .route("/api/parents/refresh", post(refresh))
        .route("/api/settings", get(get_settings))
        .route("/api/parents/settings", put(put_settings))
        .route("/api/parents/playlists", post(add_playlist))
        .route("/api/parents/playlists/{id}", delete(remove_playlist))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_session,
        ));
    Router::new()
        .route("/", get(index))
        .route("/sw.js", get(|| asset("text/javascript", SW, true)))
        .route("/manifest.webmanifest", get(manifest))
        .route("/icon-180.png", get(|| asset("image/png", ICON_180, false)))
        .route("/icon-192.png", get(|| asset("image/png", ICON_192, false)))
        .route("/icon-512.png", get(|| asset("image/png", ICON_512, false)))
        .route("/api/login", post(login))
        .merge(api)
        .with_state(state)
}

const INDEX: &str = include_str!("../static/index.html");
const SW: &[u8] = include_bytes!("../static/sw.js");
const MANIFEST: &str = include_str!("../static/manifest.webmanifest");

/// The page and manifest with the listener's name filled in, once at start-up.
/// The names were checked to be plain text (`config::checked_name`), so they go
/// in without escaping.
pub struct Pages {
    index: String,
    manifest: String,
}

impl Pages {
    pub fn new(short_name: &str, title: &str) -> Self {
        let fill = |s: &str| {
            s.replace("{{short_name}}", short_name)
                .replace("{{title}}", title)
        };
        Self {
            index: fill(INDEX),
            manifest: fill(MANIFEST),
        }
    }
}

async fn index(State(state): State<AppState>) -> Response {
    let headers = [
        (header::CONTENT_TYPE, "text/html; charset=utf-8"),
        (header::CACHE_CONTROL, "no-cache"),
    ];
    (headers, state.pages.index.clone()).into_response()
}

async fn manifest(State(state): State<AppState>) -> Response {
    let headers = [
        (header::CONTENT_TYPE, "application/manifest+json"),
        (header::CACHE_CONTROL, "no-cache"),
    ];
    (headers, state.pages.manifest.clone()).into_response()
}
const ICON_180: &[u8] = include_bytes!("../static/icon-180.png");
const ICON_192: &[u8] = include_bytes!("../static/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../static/icon-512.png");

async fn asset(content_type: &'static str, body: &'static [u8], revalidate: bool) -> Response {
    let cache = if revalidate {
        "no-cache"
    } else {
        "public, max-age=86400"
    };
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache),
        ],
        body,
    )
        .into_response()
}

#[derive(Deserialize)]
struct Login {
    password: String,
}

async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<Login>,
) -> Result<(CookieJar, Json<Value>)> {
    let cookie = state.auth.login(body.password).await?;
    Ok((jar.add(cookie), Json(json!({ "ok": true }))))
}

fn checked_id(id: &str) -> Result<&str> {
    if is_id(id) {
        Ok(id)
    } else {
        Err(Error::NotFound)
    }
}

async fn playlists(State(state): State<AppState>) -> Json<Value> {
    let tiles: Vec<Value> = state
        .playlists
        .all(false)
        .await
        .into_iter()
        .map(|(id, p)| match p {
            Some(p) => json!({ "id": id, "name": p.name, "count": p.tracks.len(), "total": p.total, "missing": p.missing.len() }),
            None => json!({ "id": id, "name": "Loading…", "count": 0, "total": 0, "missing": 0 }),
        })
        .collect();
    Json(json!({ "playlists": tiles, "keep": state.playlists.song_ids() }))
}

async fn playlist(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Value>> {
    let id = checked_id(&id)?;
    if !state.playlists.has(id) {
        return Err(Error::NotFound);
    }
    let p = state
        .playlists
        .get(id, false)
        .await
        .ok_or(Error::NotFound)?;
    Ok(Json(serde_json::to_value(p)?))
}

/// Passes an upstream response through: its status, the listed headers, a
/// private cache header, and the body as a stream.
fn relay(res: reqwest::Response, keep: &[header::HeaderName]) -> Response {
    let status = StatusCode::from_u16(res.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut headers = HeaderMap::new();
    for name in keep {
        if let Some(value) = res
            .headers()
            .get(name.as_str())
            .and_then(|v| HeaderValue::from_bytes(v.as_bytes()).ok())
        {
            headers.insert(name.clone(), value);
        }
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    (status, headers, Body::from_stream(res.bytes_stream())).into_response()
}

async fn playlist_cover(State(state): State<AppState>, Path(id): Path<String>) -> Result<Response> {
    let url = state
        .playlists
        .cover(checked_id(&id)?)
        .ok_or(Error::NotFound)?;
    Ok(relay(state.spotify.image(&url).await?, IMAGE_HEADERS))
}

/// Serves a song's file from disk: Range requests (seeking, and how Safari
/// fetches audio) are answered by tower-http's `ServeFile`.
async fn serve(path: std::path::PathBuf, req: Request) -> Result<Response> {
    let mut res = ServeFile::new(path)
        .oneshot(req)
        .await
        .map_err(|e| Error::Internal(e.to_string()))?
        .map(Body::new);
    // The guessed type for .m4a is "audio/m4a", which isn't a registered type;
    // Safari wants the standard "audio/mp4" for AAC in an MP4 container.
    if res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        == Some("audio/m4a")
    {
        res.headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/mp4"));
    }
    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    Ok(res)
}

/// A song on a current playlist, by its Spotify id; nothing else is served.
fn served_song(state: &AppState, id: &str) -> Result<()> {
    if is_id(id) && state.playlists.on_a_playlist(id) {
        Ok(())
    } else {
        Err(Error::NotFound)
    }
}

async fn audio(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Request,
) -> Result<Response> {
    served_song(&state, &id)?;
    let path = state.library.audio(&id).ok_or(Error::NotFound)?;
    serve(path, req).await
}

async fn song_cover(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Request,
) -> Result<Response> {
    served_song(&state, &id)?;
    let path = state.library.cover(&id).ok_or(Error::NotFound)?;
    serve(path, req).await
}

async fn parents_status(State(state): State<AppState>) -> Json<Value> {
    let lists: Vec<Value> = state
        .playlists
        .all(false)
        .await
        .into_iter()
        .map(|(id, p)| match p {
            Some(p) => json!({ "id": id, "name": p.spotify_name, "count": p.tracks.len(), "total": p.total, "missing": p.missing }),
            None => json!({ "id": id, "name": "Loading…", "count": 0, "total": 0, "missing": [] }),
        })
        .collect();
    // Songs on their way, then songs not found (only those still on a playlist).
    let (queued, failed) = state.fetcher.status();
    let mut downloads: Vec<Value> = queued
        .iter()
        .map(|t| json!({ "name": t.title, "artist": t.first_artist(), "state": "downloading" }))
        .collect();
    downloads.extend(
        failed
            .iter()
            .filter(|(id, _)| state.playlists.on_a_playlist(id))
            .map(|(_, f)| json!({ "name": f.title, "artist": f.artist, "state": "not-found", "note": f.note })),
    );
    Json(json!({ "playlists": lists, "downloads": downloads }))
}

/// The grown-ups' "check now": retry songs not found, re-read every playlist
/// from Spotify (which hands songs not on disk to the fetcher), and answer with
/// the fresh status.
async fn refresh(State(state): State<AppState>) -> Result<Json<Value>> {
    state.fetcher.retry_failed()?;
    state.playlists.all(true).await;
    Ok(parents_status(State(state)).await)
}

async fn get_settings(State(state): State<AppState>) -> Json<Settings> {
    Json(state.settings.get())
}

async fn put_settings(
    State(state): State<AppState>,
    Json(body): Json<Settings>,
) -> Result<Json<Settings>> {
    Ok(Json(state.settings.set(body)?))
}

#[derive(Deserialize)]
struct AddPlaylist {
    link: String,
}

async fn add_playlist(
    State(state): State<AppState>,
    Json(body): Json<AddPlaylist>,
) -> Result<Json<Value>> {
    let id = playlist_id(&body.link)
        .ok_or_else(|| Error::BadRequest("That isn't a Spotify playlist link.".into()))?;
    let p =
        state.playlists.add(&id).await?.ok_or_else(|| {
            Error::BadRequest("Couldn't read that playlist. Is it public?".into())
        })?;
    Ok(Json(json!({ "ok": true, "name": p.spotify_name })))
}

async fn remove_playlist(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>> {
    state.playlists.remove(checked_id(&id)?)?;
    Ok(Json(json!({ "ok": true })))
}
