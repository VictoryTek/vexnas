//! Integration tests: the real router (sessions, CSRF, auth, audit, headers)
//! against a fake root helper on a unix socket. No TLS — that layer is covered
//! by the NixOS VM test.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Method, Request, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::post;
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::net::UnixListener;
use tower::ServiceExt;
use vexnas_proto::{read_frame, write_frame, Request as HReq, Response as HResp};
use vexnas_web::api::middleware::{require_admin_for_writes, require_session};
use vexnas_web::config::Config;
use vexnas_web::helper::Helper;
use vexnas_web::rate_limit::LoginRateLimiter;
use vexnas_web::security::{compute_csp, SecurityHeaders};
use vexnas_web::session_store::SqliteSessionStore;
use vexnas_web::state::AppState;
use vexnas_web::{build_app, db};

const HOST: &str = "nas.test:7290";
const ORIGIN: &str = "https://nas.test:7290";

type Users = Arc<Mutex<HashMap<String, (String, Vec<String>)>>>;

struct Harness {
    app: Router,
    state: AppState,
    users: Users,
    _dir: tempfile::TempDir,
}

async fn fake_helper(path: &Path, users: Users) {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            let users = users.clone();
            tokio::spawn(async move {
                let Ok(req) = read_frame::<_, HReq>(&mut s).await else {
                    return;
                };
                let resp = match req {
                    HReq::Ping {} => HResp::Pong {
                        version: "test".into(),
                    },
                    HReq::AuthPam { user, password } => match users.lock().unwrap().get(&user) {
                        Some((pw, groups)) if pw == password.expose() => HResp::Auth {
                            ok: true,
                            groups: groups.clone(),
                        },
                        _ => HResp::Auth {
                            ok: false,
                            groups: vec![],
                        },
                    },
                    HReq::UserGroups { user } => HResp::Groups {
                        groups: users.lock().unwrap().get(&user).map(|(_, g)| g.clone()),
                    },
                };
                let _ = write_frame(&mut s, &resp).await;
            });
        }
    });
}

async fn harness(tweak: impl FnOnce(&mut Config)) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mut config = Config::default();
    config.paths.data_dir = dir.path().to_path_buf();
    config.helper.socket = dir.path().join("helper.sock");
    config.server.assets_path = dir.path().join("assets");
    config.auth.viewer_group = Some("nas-view".into());
    std::fs::create_dir_all(&config.server.assets_path).unwrap();
    std::fs::write(
        config.server.assets_path.join("index.html"),
        "<html><script>boot()</script></html>",
    )
    .unwrap();
    tweak(&mut config);

    let users: Users = Arc::default();
    {
        let mut u = users.lock().unwrap();
        u.insert("alice".into(), ("alicepw".into(), vec!["wheel".into()]));
        u.insert("vera".into(), ("verapw".into(), vec!["nas-view".into()]));
        u.insert("bob".into(), ("bobpw".into(), vec!["users".into()]));
    }
    fake_helper(&config.helper.socket, users.clone()).await;

    let pool = db::connect(&dir.path().join("t.db")).await.unwrap();
    let store = SqliteSessionStore::new(pool.clone());
    store.migrate().await.unwrap();
    let state = AppState {
        helper: Helper::new(config.helper.socket.clone()),
        login_limiter: Arc::new(LoginRateLimiter::new(
            config.auth.login_rate_limit_attempts,
            config.auth.login_rate_limit_window_secs,
        )),
        hostname: "nas".into(),
        db: pool,
        config: Arc::new(config.clone()),
    };
    let index = std::fs::read_to_string(config.server.assets_path.join("index.html")).ok();
    let headers = SecurityHeaders::new(&compute_csp(index.as_deref()));
    let app = build_app(state.clone(), store, headers);
    Harness {
        app,
        state,
        users,
        _dir: dir,
    }
}

struct Reply {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

impl Harness {
    async fn send(
        &self,
        method: Method,
        path: &str,
        cookie: Option<&str>,
        csrf: Option<&str>,
        body: Option<Value>,
        origin: Option<&str>,
    ) -> Reply {
        self.send_to(&self.app, method, path, cookie, csrf, body, origin)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_to(
        &self,
        app: &Router,
        method: Method,
        path: &str,
        cookie: Option<&str>,
        csrf: Option<&str>,
        body: Option<Value>,
        origin: Option<&str>,
    ) -> Reply {
        let mut b = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, HOST);
        if let Some(c) = cookie {
            b = b.header(header::COOKIE, c);
        }
        if let Some(t) = csrf {
            b = b.header("x-csrf-token", t);
        }
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        let req = if let Some(v) = body {
            b.header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(v.to_string()))
        } else {
            b.body(Body::empty())
        }
        .unwrap();
        let mut req = req;
        req.extensions_mut().insert(ConnectInfo(
            "192.168.1.20:5555".parse::<SocketAddr>().unwrap(),
        ));
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Reply {
            status,
            headers,
            body,
        }
    }

    async fn login(&self, user: &str, pw: &str) -> Reply {
        self.send(
            Method::POST,
            "/api/v1/auth/login",
            None,
            None,
            Some(json!({"username": user, "password": pw})),
            Some(ORIGIN),
        )
        .await
    }

    /// Log in and return `(cookie header value, csrf token)`.
    async fn session(&self, user: &str, pw: &str) -> (String, String) {
        let r = self.login(user, pw).await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        let set_cookie = r.headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
        let cookie = set_cookie.split(';').next().unwrap().to_string();
        (cookie, r.body["csrf_token"].as_str().unwrap().to_string())
    }

    async fn audit_actions(&self) -> Vec<String> {
        sqlx::query_scalar("SELECT action FROM audit ORDER BY id")
            .fetch_all(&self.state.db)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn login_sets_a_hardened_cookie_and_returns_csrf() {
    let h = harness(|_| {}).await;
    let r = h.login("alice", "alicepw").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["role"], "admin");
    assert!(r.body["csrf_token"].as_str().unwrap().len() >= 43);

    let sc = r.headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(sc.starts_with("__Host-vexnas="), "{sc}");
    for flag in ["Secure", "HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(sc.contains(flag), "missing {flag} in {sc}");
    }
    assert!(
        !sc.contains("Domain"),
        "__Host- cookies must not set Domain: {sc}"
    );
}

#[tokio::test]
async fn bad_password_unknown_user_and_unauthorised_group() {
    let h = harness(|_| {}).await;
    assert_eq!(
        h.login("alice", "wrong").await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        h.login("nobody", "x").await.status,
        StatusCode::UNAUTHORIZED
    );
    // bob authenticates fine but is in neither the admin nor the viewer group.
    let r = h.login("bob", "bobpw").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.headers.get(header::SET_COOKIE).is_none());
    let actions = h.audit_actions().await;
    assert_eq!(
        actions,
        vec!["login_failed", "login_failed", "login_denied"]
    );
}

#[tokio::test]
async fn writes_require_json_and_a_matching_origin() {
    let h = harness(|_| {}).await;
    let body = Some(json!({"username": "alice", "password": "alicepw"}));
    let send = |origin| {
        h.send(
            Method::POST,
            "/api/v1/auth/login",
            None,
            None,
            body.clone(),
            origin,
        )
    };
    assert_eq!(send(None).await.status, StatusCode::FORBIDDEN);
    assert_eq!(
        send(Some("https://evil.example")).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(Some("http://nas.test:7290")).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(send(Some(ORIGIN)).await.status, StatusCode::OK);

    // wrong content type
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/login")
        .header(header::HOST, HOST)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from("x"))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("192.168.1.20:1".parse::<SocketAddr>().unwrap()));
    let resp = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn session_endpoint_needs_a_cookie() {
    let h = harness(|_| {}).await;
    let r = h
        .send(Method::GET, "/api/v1/auth/session", None, None, None, None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    let (cookie, csrf) = h.session("alice", "alicepw").await;
    let r = h
        .send(
            Method::GET,
            "/api/v1/auth/session",
            Some(&cookie),
            None,
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["username"], "alice");
    assert_eq!(r.body["csrf_token"], csrf.as_str());
}

async fn logout(h: &Harness, cookie: &str, token: Option<&str>) -> Reply {
    h.send(
        Method::POST,
        "/api/v1/auth/logout",
        Some(cookie),
        token,
        Some(json!({})),
        Some(ORIGIN),
    )
    .await
}

#[tokio::test]
async fn logout_requires_csrf_then_destroys_the_session() {
    let h = harness(|_| {}).await;
    let (cookie, csrf) = h.session("alice", "alicepw").await;
    assert_eq!(
        logout(&h, &cookie, None).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        logout(&h, &cookie, Some("not-the-token")).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        logout(&h, &cookie, Some(&csrf)).await.status,
        StatusCode::OK
    );

    let r = h
        .send(
            Method::GET,
            "/api/v1/auth/session",
            Some(&cookie),
            None,
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(h.audit_actions().await.contains(&"logout".to_string()));
}

#[tokio::test]
async fn login_cycles_the_session_id() {
    let h = harness(|_| {}).await;
    let (c1, _) = h.session("alice", "alicepw").await;
    let (c2, _) = h.session("alice", "alicepw").await;
    assert_ne!(c1, c2);
}

#[tokio::test]
async fn viewers_can_read_and_log_out_but_not_write() {
    let h = harness(|_| {}).await;
    let (cookie, csrf) = h.session("vera", "verapw").await;
    let r = h
        .send(Method::GET, "/api/v1/meta", Some(&cookie), None, None, None)
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["helper"]["ok"], true);

    // A protected write route (none exist yet in the API) built from the same layers.
    let probe = Router::new()
        .route("/w", post(|| async { "wrote" }))
        .route_layer(from_fn(require_admin_for_writes))
        .route_layer(from_fn_with_state(h.state.clone(), require_session))
        .layer(from_fn(vexnas_web::api::middleware::csrf_guard))
        .layer(
            tower_sessions::SessionManagerLayer::new(SqliteSessionStore::new(h.state.db.clone()))
                .with_secure(true)
                .with_name(vexnas_web::SESSION_COOKIE),
        );
    let r = h
        .send_to(
            &probe,
            Method::POST,
            "/w",
            Some(&cookie),
            Some(&csrf),
            Some(json!({})),
            Some(ORIGIN),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.body);

    let r = h
        .send(
            Method::POST,
            "/api/v1/auth/logout",
            Some(&cookie),
            Some(&csrf),
            Some(json!({})),
            Some(ORIGIN),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);

    // Admins pass the same probe.
    let (acookie, acsrf) = h.session("alice", "alicepw").await;
    let r = h
        .send_to(
            &probe,
            Method::POST,
            "/w",
            Some(&acookie),
            Some(&acsrf),
            Some(json!({})),
            Some(ORIGIN),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn login_is_rate_limited() {
    let h = harness(|c| c.auth.login_rate_limit_attempts = 3).await;
    for _ in 0..3 {
        assert_eq!(
            h.login("alice", "wrong").await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        h.login("alice", "alicepw").await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn helper_down_fails_closed_with_503() {
    let h = harness(|_| {}).await;
    std::fs::remove_file(&h.state.config.helper.socket).unwrap();
    assert_eq!(
        h.login("alice", "alicepw").await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

async fn age_session(h: &Harness, field: &str, value: i64) {
    sqlx::query(&format!(
        "UPDATE tower_sessions SET data = json_set(data, '$.auth.{field}', ?)"
    ))
    .bind(value)
    .execute(&h.state.db)
    .await
    .unwrap();
}

#[tokio::test]
async fn revoked_group_membership_ends_the_session_at_recheck() {
    let h = harness(|_| {}).await;
    let (cookie, _) = h.session("alice", "alicepw").await;
    // Fresh check interval → still fine even if the helper now disagrees.
    h.users.lock().unwrap().get_mut("alice").unwrap().1 = vec!["users".into()];
    let ok = h
        .send(Method::GET, "/api/v1/meta", Some(&cookie), None, None, None)
        .await;
    assert_eq!(ok.status, StatusCode::OK);

    // Force the periodic re-check to be due.
    age_session(&h, "groups_checked_at", 0).await;
    let r = h
        .send(Method::GET, "/api/v1/meta", Some(&cookie), None, None, None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = h
        .send(Method::GET, "/api/v1/meta", Some(&cookie), None, None, None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "session must be gone");
}

#[tokio::test]
async fn demotion_at_recheck_downgrades_the_role() {
    let h = harness(|_| {}).await;
    let (cookie, _) = h.session("alice", "alicepw").await;
    h.users.lock().unwrap().get_mut("alice").unwrap().1 = vec!["nas-view".into()];
    age_session(&h, "groups_checked_at", 0).await;
    let r = h
        .send(
            Method::GET,
            "/api/v1/auth/session",
            Some(&cookie),
            None,
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["role"], "viewer");
}

#[tokio::test]
async fn deleted_user_ends_the_session_at_recheck() {
    let h = harness(|_| {}).await;
    let (cookie, _) = h.session("alice", "alicepw").await;
    h.users.lock().unwrap().remove("alice");
    age_session(&h, "groups_checked_at", 0).await;
    let r = h
        .send(Method::GET, "/api/v1/meta", Some(&cookie), None, None, None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn absolute_lifetime_is_enforced() {
    let h = harness(|_| {}).await;
    let (cookie, _) = h.session("alice", "alicepw").await;
    age_session(&h, "created_at", 0).await;
    let r = h
        .send(Method::GET, "/api/v1/meta", Some(&cookie), None, None, None)
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn security_headers_and_csp_hash_for_inline_boot_script() {
    let h = harness(|_| {}).await;
    let r = h.send(Method::GET, "/health", None, None, None, None).await;
    assert_eq!(r.status, StatusCode::OK);
    let csp = r
        .headers
        .get(header::CONTENT_SECURITY_POLICY)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp.contains("'sha256-"), "{csp}");
    assert!(!csp.contains("unsafe-inline"));
    assert!(r.headers.contains_key(header::STRICT_TRANSPORT_SECURITY));
    assert_eq!(
        r.headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
        "nosniff"
    );

    let api = h
        .send(Method::GET, "/api/v1/nope", None, None, None, None)
        .await;
    assert_eq!(api.status, StatusCode::NOT_FOUND);
    assert_eq!(api.headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
}

#[tokio::test]
async fn spa_fallback_serves_index_for_unknown_paths() {
    let h = harness(|_| {}).await;
    let mut req = Request::builder()
        .uri("/shares/media")
        .header(header::HOST, HOST)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo("192.168.1.20:1".parse::<SocketAddr>().unwrap()));
    let resp = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert!(String::from_utf8_lossy(&bytes).contains("boot()"));
}

#[tokio::test]
async fn passwords_never_reach_the_audit_log() {
    let h = harness(|_| {}).await;
    h.login("alice", "super-secret-pw").await;
    let rows: Vec<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT action, detail FROM audit")
            .fetch_all(&h.state.db)
            .await
            .unwrap();
    for (a, d) in rows {
        assert!(!a.unwrap_or_default().contains("super-secret-pw"));
        assert!(!d.unwrap_or_default().contains("super-secret-pw"));
    }
}

/// Browsers speak HTTP/2 over TLS: no `Host` header, the host is in the URI's
/// authority. (Found by the NixOS VM test, which uses real TLS.)
#[tokio::test]
async fn origin_check_works_for_http2_style_requests_without_a_host_header() {
    let h = harness(|_| {}).await;
    let send = |origin: &str| {
        let mut req = Request::builder()
            .method(Method::POST)
            .uri("https://nas.test:7290/api/v1/auth/login") // authority, no Host header
            .header(header::ORIGIN, origin.to_string())
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"username": "alice", "password": "alicepw"}).to_string(),
            ))
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo("192.168.1.20:1".parse::<SocketAddr>().unwrap()));
        h.app.clone().oneshot(req)
    };
    assert_eq!(send(ORIGIN).await.unwrap().status(), StatusCode::OK);
    assert_eq!(
        send("https://evil.example").await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}
