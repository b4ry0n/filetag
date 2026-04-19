//! Optional password authentication for filetag-web.
//!
//! When a password is configured (via `--password` or `$FILETAG_PASSWORD`),
//! every request is checked for a valid session cookie.  Unauthenticated
//! requests to API endpoints receive `401 Unauthorized`; requests to page
//! URLs are redirected to `/login`.
//!
//! Session tokens are random 32-byte hex strings kept in an in-memory
//! `HashSet`.  They are lost on server restart (users must log in again).
//!
//! When no password is configured, the middleware is a no-op.

use std::collections::HashSet;
use std::sync::Mutex;

use axum::Form;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use rand::Rng as _;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Session store (lives inside AppState)
// ---------------------------------------------------------------------------

pub struct SessionStore {
    /// Active session tokens.
    pub tokens: Mutex<HashSet<String>>,
    /// The password hash (SHA-256 hex). `None` = auth disabled.
    pub password_hash: Option<String>,
}

impl SessionStore {
    pub fn disabled() -> Self {
        Self {
            tokens: Mutex::new(HashSet::new()),
            password_hash: None,
        }
    }

    pub fn with_password(password: &str) -> Self {
        Self {
            tokens: Mutex::new(HashSet::new()),
            password_hash: Some(sha256_hex(password.as_bytes())),
        }
    }

    /// Returns true when authentication is enabled.
    pub fn is_enabled(&self) -> bool {
        self.password_hash.is_some()
    }

    /// Verify a password and, if correct, return a fresh session token.
    pub fn authenticate(&self, password: &str) -> Option<String> {
        let hash = self.password_hash.as_ref()?;
        if sha256_hex(password.as_bytes()) == *hash {
            let token = random_token();
            self.tokens.lock().unwrap().insert(token.clone());
            Some(token)
        } else {
            None
        }
    }

    /// Return true when the token is a known valid session.
    pub fn is_valid(&self, token: &str) -> bool {
        self.tokens.lock().unwrap().contains(token)
    }

    /// Invalidate a token (logout).
    pub fn revoke(&self, token: &str) {
        self.tokens.lock().unwrap().remove(token);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const SESSION_COOKIE: &str = "ft_session";

fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write as _;
    let hash = Sha256::digest(data);
    hash.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{:02x}", b);
        s
    })
}

fn random_token() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{:02x}", b);
        s
    })
}

/// Generate a human-friendly random password: 4 groups of 4 alphanumeric characters
/// separated by hyphens (e.g. `a3Kx-9mRp-Zq2w-Lf7v`). Easy to read and type across
/// devices while still providing ~95 bits of entropy.
pub fn random_password() -> String {
    const CHARS: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKMNPQRSTUVWXYZ23456789";
    let bytes: [u8; 16] = rand::rng().random();
    bytes
        .chunks(4)
        .map(|chunk| {
            chunk
                .iter()
                .map(|&b| CHARS[b as usize % CHARS.len()] as char)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Extract the session token from the `Cookie` header.
fn extract_token(req: &Request<Body>) -> Option<String> {
    let cookie_header = req.headers().get(header::COOKIE)?.to_str().ok()?;
    cookie_header
        .split(';')
        .map(str::trim)
        .find(|p| p.starts_with(SESSION_COOKIE))?
        .split_once('=')
        .map(|x| x.1)
        .map(str::to_owned)
}

// ---------------------------------------------------------------------------
// Axum middleware
// ---------------------------------------------------------------------------

/// Axum middleware that enforces authentication when a password is configured.
/// Passes through all requests when auth is disabled.
pub async fn auth_middleware(
    State(state): State<std::sync::Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if !state.sessions.is_enabled() {
        return next.run(req).await;
    }

    let path = req.uri().path().to_owned();

    // Always allow the login page itself and its static assets.
    if path == "/login" || path == "/favicon.svg" {
        return next.run(req).await;
    }

    // Check session cookie.
    if let Some(token) = extract_token(&req)
        && state.sessions.is_valid(&token)
    {
        return next.run(req).await;
    }

    // Unauthenticated: API → 401, everything else → redirect to /login.
    if path.starts_with("/api/")
        || path.starts_with("/preview/")
        || path.starts_with("/thumb/")
        || path.starts_with("/css/")
        || path.starts_with("/js/")
    {
        (StatusCode::UNAUTHORIZED, "Unauthorised").into_response()
    } else {
        Redirect::to("/login").into_response()
    }
}

// ---------------------------------------------------------------------------
// Login / logout handlers
// ---------------------------------------------------------------------------

static LOGIN_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>filetag — login</title>
<style>
  *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
  :root { color-scheme: light dark; }
  body {
    min-height: 100dvh;
    display: flex; align-items: center; justify-content: center;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    background: #f2f2f7;
  }
  @media (prefers-color-scheme: dark) { body { background: #1c1c1e; } }
  .card {
    background: #fff;
    border-radius: 12px;
    padding: 32px 28px;
    width: min(340px, 92vw);
    box-shadow: 0 4px 24px rgba(0,0,0,.10);
  }
  @media (prefers-color-scheme: dark) {
    .card { background: #2c2c2e; color: #f2f2f7; }
  }
  h1 { font-size: 20px; font-weight: 600; margin-bottom: 20px; }
  label { display: block; font-size: 13px; margin-bottom: 4px; }
  input[type=password] {
    width: 100%; padding: 8px 10px; border-radius: 6px;
    border: 1px solid #d1d1d6; font-size: 14px; margin-bottom: 14px;
    background: transparent; color: inherit;
  }
  input[type=password]:focus { outline: 2px solid #0071e3; border-color: transparent; }
  button {
    width: 100%; padding: 9px; border-radius: 8px; border: none;
    background: #0071e3; color: #fff; font-size: 15px; font-weight: 500;
    cursor: pointer;
  }
  button:hover { background: #0077ed; }
  .error { color: #ff3b30; font-size: 13px; margin-top: 10px; }
</style>
</head>
<body>
<div class="card">
  <h1>filetag</h1>
  <form method="post" action="/login">
    <label for="pw">Password</label>
    <input type="password" id="pw" name="password" autofocus autocomplete="current-password">
    <button type="submit">Sign in</button>
    {{ERROR}}
  </form>
</div>
</body>
</html>
"#;

pub async fn login_page(State(state): State<std::sync::Arc<AppState>>) -> Response {
    if !state.sessions.is_enabled() {
        return Redirect::to("/").into_response();
    }
    Html(LOGIN_HTML.replace("{{ERROR}}", "")).into_response()
}

#[derive(Deserialize)]
pub struct LoginForm {
    password: String,
}

pub async fn login_submit(
    State(state): State<std::sync::Arc<AppState>>,
    Form(form): Form<LoginForm>,
) -> Response {
    if !state.sessions.is_enabled() {
        return Redirect::to("/").into_response();
    }

    if let Some(token) = state.sessions.authenticate(&form.password) {
        // Set HttpOnly, SameSite=Strict cookie; no Secure flag since this is
        // typically on localhost (HTTPS not guaranteed).
        let cookie = format!(
            "{}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=86400",
            SESSION_COOKIE, token
        );
        ([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response()
    } else {
        Html(LOGIN_HTML.replace("{{ERROR}}", r#"<p class="error">Incorrect password.</p>"#))
            .into_response()
    }
}

pub async fn logout(State(state): State<std::sync::Arc<AppState>>, req: Request<Body>) -> Response {
    if let Some(token) = extract_token(&req) {
        state.sessions.revoke(&token);
    }
    // Clear cookie.
    let cookie = format!(
        "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
        SESSION_COOKIE
    );
    ([(header::SET_COOKIE, cookie)], Redirect::to("/login")).into_response()
}
