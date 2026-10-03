//! Client for the root helper (`vexnasd`).

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use tokio::net::UnixStream;
use vexnas_proto::{read_frame, write_frame, Request, Response, Secret};

const CALL_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub enum HelperError {
    Unavailable(io::Error),
    Protocol(String),
}

impl std::fmt::Display for HelperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(e) => write!(f, "helper unavailable: {e}"),
            Self::Protocol(m) => write!(f, "helper protocol error: {m}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Helper {
    socket: PathBuf,
}

impl Helper {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    async fn call(&self, req: &Request) -> Result<Response, HelperError> {
        let fut = async {
            let mut stream = UnixStream::connect(&self.socket).await?;
            write_frame(&mut stream, req).await?;
            read_frame::<_, Response>(&mut stream).await
        };
        tokio::time::timeout(CALL_TIMEOUT, fut)
            .await
            .map_err(|_| HelperError::Unavailable(io::ErrorKind::TimedOut.into()))?
            .map_err(HelperError::Unavailable)
    }

    pub async fn ping(&self) -> Result<String, HelperError> {
        match self.call(&Request::Ping {}).await? {
            Response::Pong { version } => Ok(version),
            other => Err(HelperError::Protocol(format!("unexpected {other:?}"))),
        }
    }

    /// `Ok(Some(groups))` on successful authentication, `Ok(None)` on bad credentials.
    pub async fn auth_pam(
        &self,
        user: &str,
        password: &str,
    ) -> Result<Option<Vec<String>>, HelperError> {
        let req = Request::AuthPam {
            user: user.to_string(),
            password: Secret::new(password),
        };
        match self.call(&req).await? {
            Response::Auth { ok: true, groups } => Ok(Some(groups)),
            Response::Auth { ok: false, .. } => Ok(None),
            Response::Error { message } => Err(HelperError::Protocol(message)),
            other => Err(HelperError::Protocol(format!("unexpected {other:?}"))),
        }
    }

    /// `Ok(None)` if the user no longer exists.
    pub async fn user_groups(&self, user: &str) -> Result<Option<Vec<String>>, HelperError> {
        let req = Request::UserGroups {
            user: user.to_string(),
        };
        match self.call(&req).await? {
            Response::Groups { groups } => Ok(groups),
            Response::Error { message } => Err(HelperError::Protocol(message)),
            other => Err(HelperError::Protocol(format!("unexpected {other:?}"))),
        }
    }
}
