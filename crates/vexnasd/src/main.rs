//! vexnasd — the root helper. Socket-activated; speaks the closed verb set in
//! `vexnas-proto` over a unix socket. Phase 0 verbs: `ping`, `auth_pam`, `user_groups`.

mod pam;
mod users;

use std::os::fd::FromRawFd;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use vexnas_proto::{read_frame, write_frame, Request, Response};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Delay applied (while holding a PAM permit) after a failed authentication, so
/// the helper as a whole can only absorb a few guesses per second no matter how
/// many web-side rate limiters are bypassed.
const FAILURE_DELAY: Duration = Duration::from_millis(1000);
const MAX_CONCURRENT_PAM: usize = 4;

struct Config {
    /// Dev only: bind this path when not socket-activated.
    socket: Option<String>,
    peer_user: String,
    pam_service: String,
}

fn parse_args() -> Config {
    let mut cfg = Config {
        socket: None,
        peer_user: "vexnas".into(),
        pam_service: "vexnas".into(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--socket" => cfg.socket = args.next(),
            "--peer-user" => {
                if let Some(v) = args.next() {
                    cfg.peer_user = v
                }
            }
            "--pam-service" => {
                if let Some(v) = args.next() {
                    cfg.pam_service = v
                }
            }
            other => {
                eprintln!("vexnasd: unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    cfg
}

/// systemd socket activation: fd 3 when `LISTEN_PID` is us and `LISTEN_FDS` is 1.
fn activated_listener() -> Option<std::os::unix::net::UnixListener> {
    let pid: u32 = std::env::var("LISTEN_PID").ok()?.parse().ok()?;
    let fds: i32 = std::env::var("LISTEN_FDS").ok()?.parse().ok()?;
    if pid != std::process::id() || fds != 1 {
        return None;
    }
    std::env::remove_var("LISTEN_PID");
    std::env::remove_var("LISTEN_FDS");
    // SAFETY: systemd guarantees fd 3 is an open listening socket owned by us.
    Some(unsafe { std::os::unix::net::UnixListener::from_raw_fd(3) })
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = parse_args();

    // Only root and the web service user may talk to us (belt and braces on top
    // of the socket's 0660 root:vexnas mode).
    let mut allowed_uids = vec![0u32];
    match users::lookup_user(&cfg.peer_user) {
        Some((uid, _)) => allowed_uids.push(uid),
        None => {
            tracing::warn!(user = %cfg.peer_user, "peer user does not exist; only root may connect")
        }
    }
    let allowed_uids = Arc::new(allowed_uids);

    let listener = match activated_listener() {
        Some(std_listener) => {
            std_listener
                .set_nonblocking(true)
                .expect("set_nonblocking on activated socket");
            UnixListener::from_std(std_listener).expect("adopt activated socket")
        }
        None => {
            let path = cfg.socket.as_deref().unwrap_or_else(|| {
                eprintln!("vexnasd: not socket-activated and no --socket given");
                std::process::exit(2);
            });
            let _ = std::fs::remove_file(path);
            UnixListener::bind(path).expect("bind helper socket")
        }
    };
    tracing::info!("vexnasd ready");

    let pam_permits = Arc::new(Semaphore::new(MAX_CONCURRENT_PAM));
    let pam_service: Arc<str> = cfg.pam_service.into();

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let allowed = allowed_uids.clone();
        let permits = pam_permits.clone();
        let service = pam_service.clone();
        tokio::spawn(async move {
            match stream.peer_cred() {
                Ok(c) if allowed.contains(&c.uid()) => {}
                Ok(c) => {
                    tracing::warn!(uid = c.uid(), "rejected connection from unexpected uid");
                    return;
                }
                Err(e) => {
                    tracing::warn!("peer_cred failed: {e}");
                    return;
                }
            }
            if let Err(e) = handle(stream, permits, service).await {
                tracing::debug!("connection ended: {e}");
            }
        });
    }
}

async fn handle(
    mut stream: UnixStream,
    permits: Arc<Semaphore>,
    service: Arc<str>,
) -> std::io::Result<()> {
    let req: Request = tokio::time::timeout(REQUEST_TIMEOUT, read_frame(&mut stream))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "request timeout"))??;
    let resp = dispatch(req, &permits, &service).await;
    write_frame(&mut stream, &resp).await
}

async fn dispatch(req: Request, permits: &Semaphore, service: &Arc<str>) -> Response {
    match req {
        Request::Ping {} => Response::Pong {
            version: env!("CARGO_PKG_VERSION").into(),
        },
        Request::AuthPam { user, password } => {
            if let Err(e) = vexnas_proto::validate_username(&user)
                .and_then(|_| vexnas_proto::validate_password(password.expose()))
            {
                return Response::Error {
                    message: e.to_string(),
                };
            }
            let Ok(_permit) = permits.acquire().await else {
                return Response::Error {
                    message: "shutting down".into(),
                };
            };
            let (u, p, s) = (user.clone(), password, service.clone());
            let ok = tokio::task::spawn_blocking(move || pam::authenticate(&s, &u, p.expose()))
                .await
                .unwrap_or(false);
            if !ok {
                tokio::time::sleep(FAILURE_DELAY).await;
                return Response::Auth {
                    ok: false,
                    groups: vec![],
                };
            }
            let groups = tokio::task::spawn_blocking(move || users::user_groups(&user))
                .await
                .ok()
                .flatten();
            match groups {
                Some(groups) => Response::Auth { ok: true, groups },
                // Authenticated by PAM but unknown to NSS: treat as failure.
                None => Response::Auth {
                    ok: false,
                    groups: vec![],
                },
            }
        }
        Request::UserGroups { user } => {
            if let Err(e) = vexnas_proto::validate_username(&user) {
                return Response::Error {
                    message: e.to_string(),
                };
            }
            let groups = tokio::task::spawn_blocking(move || users::user_groups(&user))
                .await
                .ok()
                .flatten();
            Response::Groups { groups }
        }
    }
}
