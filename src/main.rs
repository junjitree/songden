//! songden: a kid-safe music player. Spotify playlists, with each song fetched
//! from YouTube onto your own server.
//!
//! The page never talks to Spotify or YouTube itself; this server reads the
//! playlists, keeps the songs, and only hands out songs on the chosen playlists.

use std::{net::SocketAddr, sync::Arc};

use tracing::info;
use tracing_subscriber::EnvFilter;

mod auth;
mod config;
mod error;
mod fetch;
mod http;
mod playlists;
mod result;
mod spotify;
mod store;
mod text;

pub use self::error::*;
pub use self::result::*;

use crate::{
    auth::Auth,
    config::Config,
    fetch::{Fetcher, Library},
    playlists::Playlists,
    spotify::Spotify,
};

#[derive(Clone)]
pub struct AppState {
    pub pages: Arc<http::Pages>,
    pub auth: Arc<Auth>,
    pub playlists: Arc<Playlists>,
    pub fetcher: Fetcher,
    pub library: Library,
    pub spotify: Spotify,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // `songden hash-password` reads a password from stdin and prints its hash for
    // FAMILY_PASSWORD_HASH, so the password itself is never stored.
    if std::env::args().nth(1).as_deref() == Some("hash-password") {
        let mut password = String::new();
        std::io::stdin().read_line(&mut password)?;
        println!(
            "{}",
            auth::hash_password(password.trim_end_matches(['\r', '\n']))?
        );
        return Ok(());
    }

    let cfg = Config::from_env()?;
    std::fs::create_dir_all(&cfg.state_dir)?;
    // Checked first: a bad password hash should stop start-up before any work.
    let auth = Arc::new(Auth::new(
        &cfg.state_dir,
        cfg.password_hash.clone(),
        cfg.sessions_since,
        cfg.secure_cookies,
    )?);

    let spotify = Spotify::new()?;
    let library = Library::new(cfg.music_dir.clone())?;
    let fetcher = Fetcher::start(
        library.clone(),
        cfg.ytdlp.clone(),
        &cfg.state_dir,
        spotify.clone(),
    );
    let playlists = Arc::new(Playlists::new(
        &cfg.state_dir,
        cfg.prefix(),
        spotify.clone(),
        library.clone(),
        fetcher.clone(),
    ));
    if playlists.ids().is_empty() {
        for id in &cfg.seed_playlists {
            playlists.add(id).await?;
        }
    }

    let (short_name, title) = cfg.branding();
    let state = AppState {
        pages: Arc::new(http::Pages::new(&short_name, &title)),
        auth,
        playlists,
        fetcher,
        library,
        spotify,
    };
    let addr = SocketAddr::from(([0, 0, 0, 0], cfg.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("songden on http://{addr}");
    axum::serve(listener, http::routes(state)).await?;
    Ok(())
}
