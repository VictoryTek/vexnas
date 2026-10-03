use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum_server::tls_rustls::RustlsAcceptor;
use axum_server::Handle;
use socket2::{Domain, Protocol, Socket, Type};
use vexnas_web::allowlist::{Allowlist, AllowlistAcceptor};
use vexnas_web::config::Config;
use vexnas_web::helper::Helper;
use vexnas_web::rate_limit::LoginRateLimiter;
use vexnas_web::security::{compute_csp, SecurityHeaders};
use vexnas_web::session_store::{self, SqliteSessionStore};
use vexnas_web::state::AppState;
use vexnas_web::{build_app, db, tls};

fn bind_listener(ip: IpAddr, port: u16) -> std::io::Result<std::net::TcpListener> {
    let addr = SocketAddr::new(ip, port);
    let sock = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    if ip.is_ipv6() {
        // Separate v4 and v6 sockets, so "0.0.0.0" + "::" never collide.
        sock.set_only_v6(true)?;
    }
    sock.set_reuse_address(true)?;
    sock.bind(&addr.into())?;
    sock.listen(1024)?;
    sock.set_nonblocking(true)?;
    Ok(sock.into())
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config_path: PathBuf = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("VEXNAS_CONFIG").ok())
        .unwrap_or_else(|| "/etc/vexnas/config.toml".into())
        .into();
    let config = Config::load(&config_path).map_err(anyhow::Error::msg)?;
    let allow = Allowlist::parse(&config.server.allowed_cidrs).map_err(anyhow::Error::msg)?;
    let hostname = hostname();

    std::fs::create_dir_all(&config.paths.data_dir)
        .with_context(|| format!("create {}", config.paths.data_dir.display()))?;
    let pool = db::connect(&config.paths.data_dir.join("vexnas.db")).await?;
    let store = SqliteSessionStore::new(pool.clone());
    store.migrate().await?;
    tokio::spawn(session_store::cleanup_loop(
        store.clone(),
        Duration::from_secs(3600),
    ));

    let index_html = std::fs::read_to_string(config.server.assets_path.join("index.html")).ok();
    if index_html.is_none() {
        tracing::warn!(path = %config.server.assets_path.display(), "index.html not found; serving API only");
    }
    let headers = SecurityHeaders::new(&compute_csp(index_html.as_deref()));

    let (tls_config, _tls_task) =
        tls::setup(&config.tls, &config.paths.data_dir, &hostname).await?;

    let state = AppState {
        helper: Helper::new(config.helper.socket.clone()),
        login_limiter: Arc::new(LoginRateLimiter::new(
            config.auth.login_rate_limit_attempts,
            config.auth.login_rate_limit_window_secs,
        )),
        hostname: hostname.into(),
        db: pool,
        config: Arc::new(config.clone()),
    };
    let app = build_app(state, store, headers);

    let mut listeners = Vec::new();
    for l in &config.server.listen {
        let ip: IpAddr = l
            .parse()
            .with_context(|| format!("server.listen: invalid address {l:?}"))?;
        match bind_listener(ip, config.server.port) {
            Ok(sock) => {
                tracing::info!(
                    "listening on https://{}",
                    SocketAddr::new(ip, config.server.port)
                );
                listeners.push(sock);
            }
            Err(e) => tracing::warn!("cannot bind {ip}:{}: {e}", config.server.port),
        }
    }
    if listeners.is_empty() {
        bail!("could not bind any listen address");
    }

    let handle = Handle::new();
    let mut tasks = Vec::new();
    for listener in listeners {
        let acceptor =
            AllowlistAcceptor::new(RustlsAcceptor::new(tls_config.clone()), allow.clone());
        let server = axum_server::from_tcp(listener)
            .acceptor(acceptor)
            .handle(handle.clone());
        let svc = app
            .clone()
            .into_make_service_with_connect_info::<SocketAddr>();
        tasks.push(tokio::spawn(async move { server.serve(svc).await }));
    }

    let shutdown = handle.clone();
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        tracing::info!("shutting down");
        shutdown.graceful_shutdown(Some(Duration::from_secs(10)));
    });

    for t in tasks {
        t.await.context("server task panicked")??;
    }
    Ok(())
}
