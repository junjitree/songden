use std::{env, path::PathBuf};

use crate::{Error, Result};

pub const DEFAULT_PORT: u16 = 8090;

/// Everything read from the environment at start-up.
pub struct Config {
    pub port: u16,
    pub state_dir: PathBuf,
    pub jellyfin_url: String,
    pub jellyfin_token: String,
    pub lidarr_url: String,
    pub lidarr_key: String,
    pub prowlarr_url: String,
    pub prowlarr_key: String,
    /// Where Lidarr puts music (the root folder for artists it adds).
    pub music_root: String,
    /// Argon2 PHC string of the shared family password (`songden hash-password`).
    pub password_hash: String,
    /// Unix seconds; sessions started before it are refused. Raise it to sign
    /// every device out.
    pub sessions_since: u64,
    /// Mark the session cookie Secure. Only off for plain-HTTP local runs.
    pub secure_cookies: bool,
    /// Playlist ids added on first start when none are stored yet.
    pub seed_playlists: Vec<String>,
    /// Who the player is for ("Mia"): names the home-screen app and the badge.
    pub listener: Option<String>,
    /// Trimmed from playlist names on tiles; defaults to `listener`.
    pub playlist_prefix: Option<String>,
}

/// A name that goes into the page and the manifest as-is, so only plain name
/// characters are allowed: letters, digits, spaces, apostrophes and hyphens.
pub fn checked_name(var: &str, value: String) -> Result<String> {
    let value = value.trim().to_string();
    let plain = value
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '\'' | '-' | '\u{2019}'));
    if value.is_empty() || value.chars().count() > 30 || !plain {
        return Err(Error::Internal(format!(
            "{var} must be 1-30 letters, digits, spaces, apostrophes or hyphens"
        )));
    }
    Ok(value)
}

fn required(name: &str) -> Result<String> {
    env::var(name).map_err(|_| Error::Internal(format!("{name} must be set")))
}

fn optional(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

impl Config {
    /// The home-screen name and the badge text.
    pub fn branding(&self) -> (String, String) {
        match &self.listener {
            Some(name) => (name.clone(), format!("{name}'s Songs")),
            None => ("songden".into(), "Songs".into()),
        }
    }

    pub fn prefix(&self) -> Option<&str> {
        self.playlist_prefix.as_deref().or(self.listener.as_deref())
    }

    pub fn from_env() -> Result<Self> {
        Ok(Self {
            port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(DEFAULT_PORT),
            state_dir: optional("STATE_DIR", "/state").into(),
            jellyfin_url: optional("JELLYFIN_URL", "http://jellyfin:8096"),
            jellyfin_token: required("JELLYFIN_TOKEN")?,
            lidarr_url: optional("LIDARR_URL", "http://lidarr:8686"),
            lidarr_key: required("LIDARR_KEY")?,
            prowlarr_url: optional("PROWLARR_URL", "http://prowlarr:9696"),
            prowlarr_key: required("PROWLARR_KEY")?,
            music_root: optional("MUSIC_ROOT", "/data/media/music"),
            password_hash: required("FAMILY_PASSWORD_HASH")?,
            sessions_since: optional("SESSIONS_SINCE", "0").parse().unwrap_or(0),
            secure_cookies: optional("SECURE_COOKIES", "true") != "false",
            seed_playlists: optional("SEED_PLAYLISTS", "")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
            listener: env::var("LISTENER")
                .ok()
                .map(|v| checked_name("LISTENER", v))
                .transpose()?,
            playlist_prefix: env::var("PLAYLIST_PREFIX")
                .ok()
                .map(|v| checked_name("PLAYLIST_PREFIX", v))
                .transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::checked_name;

    #[test]
    fn names_are_plain_text_only() {
        assert_eq!(checked_name("X", " Mia ".into()).unwrap(), "Mia");
        assert!(checked_name("X", "Zoë-Ann O'Neil".into()).is_ok());
        assert!(checked_name("X", "<script>".into()).is_err());
        assert!(checked_name("X", "Mia\"}".into()).is_err());
        assert!(checked_name("X", "".into()).is_err());
    }
}
