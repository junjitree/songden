//! The family sign-in: one shared password, then a session cookie that lasts a
//! year and is renewed whenever the app is used, so a device signs in once.
//!
//! Sessions live in the state dir so one device can be cut off; raising
//! `SESSIONS_SINCE` signs every device out. Wrong passwords are limited for
//! everyone at once (one shared password, so per-person limits mean nothing):
//! after `MAX_FAILURES` within `FAILURE_WINDOW`, sign-in pauses. Devices already
//! signed in are unaffected, so nobody can lock the family out.

use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{
        SaltString,
        rand_core::{OsRng, RngCore},
    },
};
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{AppState, Error, Result, store::JsonStore};

pub const COOKIE: &str = "songden_session";
const SESSION_LIFETIME: Duration = Duration::from_secs(365 * 24 * 3600);
/// A session's expiry is pushed back at most this often (each push is a disk write).
const RENEW_EVERY: Duration = Duration::from_secs(24 * 3600);
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Serialize, Deserialize)]
struct Session {
    created: u64,
    last_seen: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The PHC string for a password, for `FAMILY_PASSWORD_HASH`.
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Internal(e.to_string()))
}

pub struct Auth {
    password_hash: String,
    sessions_since: u64,
    secure_cookies: bool,
    sessions: Mutex<JsonStore<BTreeMap<String, Session>>>,
    failures: Mutex<VecDeque<Instant>>,
}

impl Auth {
    pub fn new(
        state_dir: &Path,
        password_hash: String,
        sessions_since: u64,
        secure_cookies: bool,
    ) -> Result<Self> {
        PasswordHash::new(&password_hash)
            .map_err(|e| Error::Internal(format!("FAMILY_PASSWORD_HASH: {e}")))?;
        Ok(Self {
            password_hash,
            sessions_since,
            secure_cookies,
            sessions: Mutex::new(JsonStore::load(state_dir, "sessions.json")),
            failures: Mutex::new(VecDeque::new()),
        })
    }

    fn cookie(&self, token: String) -> Cookie<'static> {
        Cookie::build((COOKIE, token))
            .path("/")
            .http_only(true)
            .secure(self.secure_cookies)
            .same_site(SameSite::Lax)
            .max_age(time_duration(SESSION_LIFETIME))
            .build()
    }

    fn locked_out(&self) -> bool {
        let mut failures = self.failures.lock().unwrap_or_else(|p| p.into_inner());
        while failures
            .front()
            .is_some_and(|t| t.elapsed() > FAILURE_WINDOW)
        {
            failures.pop_front();
        }
        failures.len() >= MAX_FAILURES
    }

    /// Checks the password and starts a session; the cookie to set on success.
    pub async fn login(&self, password: String) -> Result<Cookie<'static>> {
        if self.locked_out() {
            return Err(Error::TooManyRequests);
        }
        let hash = self.password_hash.clone();
        // Argon2 is deliberately slow; keep it off the async workers.
        let ok = tokio::task::spawn_blocking(move || {
            PasswordHash::new(&hash)
                .map(|parsed| {
                    Argon2::default()
                        .verify_password(password.as_bytes(), &parsed)
                        .is_ok()
                })
                .unwrap_or(false)
        })
        .await
        .map_err(|e| Error::Internal(e.to_string()))?;
        if !ok {
            self.failures
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push_back(Instant::now());
            warn!("auth: wrong family password");
            return Err(Error::BadRequest("That's not the password.".into()));
        }
        let mut bytes = [0u8; 32];
        OsRng.fill_bytes(&mut bytes);
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| Error::Internal("sessions lock poisoned".into()))?;
        sessions.data.insert(
            token.clone(),
            Session {
                created: now(),
                last_seen: now(),
            },
        );
        sessions.save()?;
        Ok(self.cookie(token))
    }

    /// Whether the token is a live session; renews it (at most daily). Returns
    /// a fresh cookie when renewed, so the browser's copy is pushed back too.
    fn check(&self, token: &str) -> Option<Option<Cookie<'static>>> {
        let mut sessions = self.sessions.lock().ok()?;
        let session = sessions.data.get_mut(token)?;
        let age = now().saturating_sub(session.last_seen);
        if session.created < self.sessions_since || age > SESSION_LIFETIME.as_secs() {
            sessions.data.remove(token);
            if let Err(err) = sessions.save() {
                warn!("auth: could not save sessions: {err}");
            }
            return None;
        }
        if age < RENEW_EVERY.as_secs() {
            return Some(None);
        }
        session.last_seen = now();
        if let Err(err) = sessions.save() {
            warn!("auth: could not save sessions: {err}");
        }
        Some(Some(self.cookie(token.to_string())))
    }
}

fn time_duration(d: Duration) -> cookie::time::Duration {
    cookie::time::Duration::seconds(i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Lets a request through only with a live session cookie.
pub async fn require_session(
    State(state): State<AppState>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Result<Response> {
    let token = jar
        .get(COOKIE)
        .map(|c| c.value().to_string())
        .ok_or(Error::Unauthorized)?;
    let renewed = state.auth.check(&token).ok_or(Error::Unauthorized)?;
    let res = next.run(req).await;
    Ok(match renewed {
        Some(cookie) => {
            let (mut parts, body) = res.into_parts();
            if let Ok(value) = cookie.to_string().parse() {
                parts.headers.append(axum::http::header::SET_COOKIE, value);
            }
            Response::from_parts(parts, body)
        }
        None => res,
    })
}
