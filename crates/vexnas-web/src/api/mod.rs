mod auth;
pub mod middleware;
pub mod session;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use self::session::AuthSession;
use crate::state::AppState;

async fn meta(State(state): State<AppState>, Extension(_): Extension<AuthSession>) -> Response {
    let helper = match state.helper.ping().await {
        Ok(version) => json!({ "ok": true, "version": version }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    };
    let variant = std::fs::read_to_string("/etc/nixos/vexos-variant")
        .ok()
        .map(|s| s.trim().to_string());
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "hostname": &*state.hostname,
        "variant": variant,
        "helper": helper,
    }))
    .into_response()
}

async fn not_found() -> Response {
    middleware::json_error(StatusCode::NOT_FOUND, "Not found")
}

/// `/api/v1` — nested by the caller.
pub fn router(state: AppState) -> Router {
    let public = Router::new().route("/auth/login", post(auth::login));

    // Data routes: authenticated, and writes are admin-only.
    let data = Router::new()
        .route("/meta", get(meta))
        .route_layer(from_fn(middleware::require_admin_for_writes));

    // Session routes: any authenticated user (a viewer must be able to log out).
    let protected = Router::new()
        .route("/auth/logout", post(auth::logout))
        .route("/auth/session", get(auth::session_info))
        .merge(data)
        .route_layer(from_fn_with_state(
            state.clone(),
            middleware::require_session,
        ));

    Router::new()
        .merge(public)
        .merge(protected)
        .fallback(not_found)
        // Last added = outermost: csrf first, then audit, then the handlers.
        .layer(from_fn_with_state(state.clone(), middleware::audit_writes))
        .layer(from_fn(middleware::csrf_guard))
        .with_state(state)
}
