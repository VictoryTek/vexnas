use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tower_sessions::Session;

use super::middleware::json_error;
use super::session::{new_csrf_token, now_ts, role_for, AuthSession, SESSION_KEY};
use crate::db;
use crate::helper::HelperError;
use crate::state::AppState;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    username: String,
    password: String,
}

fn session_body(auth: &AuthSession) -> serde_json::Value {
    json!({
        "username": auth.username,
        "role": auth.role,
        "csrf_token": auth.csrf,
    })
}

pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    session: Session,
    Json(req): Json<LoginRequest>,
) -> Response {
    let ip = addr.ip().to_canonical();
    let cfg = &state.config.auth;

    if cfg.login_rate_limit_attempts > 0 && !state.login_limiter.check(ip) {
        db::audit(
            &state.db,
            Some(&req.username),
            ip,
            "login_rate_limited",
            None,
        )
        .await;
        return json_error(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many login attempts — try again later",
        );
    }

    let groups = match state.helper.auth_pam(&req.username, &req.password).await {
        Ok(Some(groups)) => groups,
        Ok(None) => {
            db::audit(&state.db, Some(&req.username), ip, "login_failed", None).await;
            return json_error(StatusCode::UNAUTHORIZED, "Invalid credentials");
        }
        // Syntactically invalid input never reaches PAM; same answer as a bad password.
        Err(HelperError::Protocol(_)) => {
            db::audit(
                &state.db,
                Some(&req.username),
                ip,
                "login_failed",
                Some("invalid input"),
            )
            .await;
            return json_error(StatusCode::UNAUTHORIZED, "Invalid credentials");
        }
        Err(e) => {
            tracing::error!("login: {e}");
            return json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Authentication service unavailable",
            );
        }
    };

    let Some(role) = role_for(&groups, cfg) else {
        db::audit(
            &state.db,
            Some(&req.username),
            ip,
            "login_denied",
            Some("not in an allowed group"),
        )
        .await;
        return json_error(
            StatusCode::FORBIDDEN,
            "This account is not allowed to use vexnas",
        );
    };

    let now = now_ts();
    let auth = AuthSession {
        username: req.username.clone(),
        role,
        csrf: new_csrf_token(),
        created_at: now,
        groups_checked_at: now,
    };

    // New session id on privilege change (session fixation defence).
    if let Err(e) = session.cycle_id().await {
        tracing::error!("cycle_id failed: {e}");
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "Session error");
    }
    if let Err(e) = session.insert(SESSION_KEY, &auth).await {
        tracing::error!("session insert failed: {e}");
        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "Session error");
    }

    db::audit(
        &state.db,
        Some(&auth.username),
        ip,
        "login_ok",
        Some(&format!("role={:?}", auth.role)),
    )
    .await;
    Json(session_body(&auth)).into_response()
}

pub async fn logout(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    session: Session,
    Extension(auth): Extension<AuthSession>,
) -> Response {
    let _ = session.flush().await;
    db::audit(
        &state.db,
        Some(&auth.username),
        addr.ip().to_canonical(),
        "logout",
        None,
    )
    .await;
    Json(json!({ "ok": true })).into_response()
}

pub async fn session_info(Extension(auth): Extension<AuthSession>) -> Response {
    Json(session_body(&auth)).into_response()
}
