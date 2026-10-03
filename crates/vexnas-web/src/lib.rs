//! vexnas-web — unprivileged, network-facing backend. Everything privileged goes
//! through the root helper (`vexnasd`) over a closed verb set.

pub mod allowlist;
pub mod api;
pub mod config;
pub mod db;
pub mod helper;
pub mod rate_limit;
pub mod security;
pub mod session_store;
pub mod state;
pub mod tls;

use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};
use tower_sessions::cookie::SameSite;
use tower_sessions::{Expiry, SessionManagerLayer};

use crate::security::SecurityHeaders;
use crate::session_store::SqliteSessionStore;
use crate::state::AppState;

pub const SESSION_COOKIE: &str = "__Host-vexnas";

/// The full application: health, API, static assets (SPA fallback), sessions, headers.
pub fn build_app(state: AppState, store: SqliteSessionStore, headers: SecurityHeaders) -> Router {
    let idle = time::Duration::minutes(state.config.auth.idle_minutes);
    // `__Host-` cookies must be Secure, Path=/, and carry no Domain attribute.
    let session_layer = SessionManagerLayer::new(store)
        .with_name(SESSION_COOKIE)
        .with_secure(true)
        .with_http_only(true)
        .with_same_site(SameSite::Strict)
        .with_path("/")
        .with_expiry(Expiry::OnInactivity(idle));

    let assets = state.config.server.assets_path.clone();
    let spa = ServeDir::new(&assets).fallback(ServeFile::new(assets.join("index.html")));

    Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest("/api/v1", api::router(state))
        .fallback_service(spa)
        .layer(session_layer)
        .layer(from_fn_with_state(headers, security::security_headers))
}
