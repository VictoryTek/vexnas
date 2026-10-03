//! Request guards: CSRF/origin checks, authentication, write-authorisation, audit.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, OriginalUri, Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use subtle::ConstantTimeEq;
use tower_sessions::Session;

use super::session::{now_ts, role_for, AuthSession, Role, SESSION_KEY};
use crate::state::AppState;

pub fn json_error(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// The origin the browser must have sent: always https, on the host it addressed.
///
/// HTTP/2 (what browsers negotiate over TLS) has no `Host` header: the host is the
/// `:authority` pseudo-header, which hyper puts in the request URI. HTTP/1.1 has
/// only the `Host` header. Accept either.
fn expected_origin(req: &Request) -> Option<String> {
    let host = match req.uri().authority() {
        Some(a) => a.as_str(),
        None => req.headers().get(header::HOST)?.to_str().ok()?,
    };
    Some(format!("https://{host}"))
}

/// For every non-safe request: JSON only, `Origin` equal to our own origin, and —
/// when the caller is authenticated — a matching `X-CSRF-Token`.
///
/// (An unauthenticated caller has no authority to protect; `require_auth` turns
/// it away with 401. Login CSRF is covered by the origin check + SameSite=Strict.)
pub async fn csrf_guard(session: Session, req: Request, next: Next) -> Response {
    if is_safe(req.method()) {
        return next.run(req).await;
    }

    let is_json = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/json"));
    if !is_json {
        return json_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Content-Type must be application/json",
        );
    }

    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if origin.is_none() || origin != expected_origin(&req).as_deref() {
        return json_error(StatusCode::FORBIDDEN, "Bad origin");
    }

    if let Ok(Some(auth)) = session.get::<AuthSession>(SESSION_KEY).await {
        let supplied = req
            .headers()
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !bool::from(supplied.as_bytes().ct_eq(auth.csrf.as_bytes())) {
            return json_error(StatusCode::FORBIDDEN, "Missing or invalid CSRF token");
        }
    }

    next.run(req).await
}

/// Requires a live session within its absolute lifetime and re-checks group
/// membership against the helper every `recheck_minutes`. Puts the `AuthSession`
/// in the request extensions.
pub async fn require_session(
    State(state): State<AppState>,
    session: Session,
    mut req: Request,
    next: Next,
) -> Response {
    let Ok(Some(mut auth)) = session.get::<AuthSession>(SESSION_KEY).await else {
        return json_error(StatusCode::UNAUTHORIZED, "Not authenticated");
    };
    let cfg = &state.config.auth;
    let now = now_ts();

    if now - auth.created_at > cfg.absolute_hours * 3600 {
        let _ = session.flush().await;
        return json_error(StatusCode::UNAUTHORIZED, "Session expired");
    }

    if now - auth.groups_checked_at > cfg.recheck_minutes * 60 {
        match state.helper.user_groups(&auth.username).await {
            Ok(Some(groups)) => match role_for(&groups, cfg) {
                Some(role) => {
                    auth.role = role;
                    auth.groups_checked_at = now;
                    if session.insert(SESSION_KEY, &auth).await.is_err() {
                        return json_error(StatusCode::INTERNAL_SERVER_ERROR, "Session error");
                    }
                }
                None => {
                    let _ = session.flush().await;
                    return json_error(StatusCode::UNAUTHORIZED, "Access revoked");
                }
            },
            Ok(None) => {
                let _ = session.flush().await;
                return json_error(StatusCode::UNAUTHORIZED, "Account no longer exists");
            }
            Err(e) => {
                // Fail closed: cannot confirm the user still belongs.
                tracing::warn!("group re-check failed: {e}");
                return json_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Authentication service unavailable",
                );
            }
        }
    }

    req.extensions_mut().insert(auth);
    next.run(req).await
}

/// Viewers are read-only: non-safe methods need the admin role. Layered *inside*
/// `require_session`, and only on data routes (a viewer must still be able to log out).
pub async fn require_admin_for_writes(req: Request, next: Next) -> Response {
    let is_admin = req
        .extensions()
        .get::<AuthSession>()
        .is_some_and(|a| a.role == Role::Admin);
    if !is_safe(req.method()) && !is_admin {
        return json_error(StatusCode::FORBIDDEN, "Admin role required");
    }
    next.run(req).await
}

/// Audit every mutating request (login/logout audit themselves with more detail).
pub async fn audit_writes(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    session: Session,
    req: Request,
    next: Next,
) -> Response {
    if is_safe(req.method()) {
        return next.run(req).await;
    }
    let path = req
        .extensions()
        .get::<OriginalUri>()
        .map(|u| u.0.path().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    let method = req.method().clone();
    let resp = next.run(req).await;

    if !path.starts_with("/api/v1/auth/") {
        let user = session
            .get::<AuthSession>(SESSION_KEY)
            .await
            .ok()
            .flatten()
            .map(|a| a.username);
        crate::db::audit(
            &state.db,
            user.as_deref(),
            addr.ip().to_canonical(),
            &format!("{method} {path}"),
            Some(&format!("status={}", resp.status().as_u16())),
        )
        .await;
    }
    resp
}
