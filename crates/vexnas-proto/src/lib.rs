//! Wire protocol between `vexnas-web` (unprivileged, network-facing) and `vexnasd`
//! (root helper).
//!
//! Framing: a 4-byte big-endian length followed by that many bytes of JSON. One
//! request and one response per connection. The verb set is closed (see
//! `docs/spec.md` §4.3); the helper deserialises with `deny_unknown_fields`.

use std::fmt;
use std::io;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Upper bound on a single frame. Requests are tiny; anything bigger is hostile.
pub const MAX_FRAME_BYTES: u32 = 64 * 1024;

pub const MAX_USERNAME_LEN: usize = 64;
pub const MAX_PASSWORD_LEN: usize = 1024;

/// A string that never appears in `Debug` output (passwords).
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// (An empty struct variant, not a unit variant: serde ignores
    /// `deny_unknown_fields` on unit variants of internally tagged enums.)
    Ping {},
    /// Authenticate `user` against PAM and return their group names on success.
    AuthPam { user: String, password: Secret },
    /// Current group names of `user` (`None` in the response if the user is gone).
    UserGroups { user: String },
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Pong { version: String },
    Auth { ok: bool, groups: Vec<String> },
    Groups { groups: Option<Vec<String>> },
    Error { message: String },
}

/// Reject usernames/passwords that can never be valid before they reach PAM.
pub fn validate_username(user: &str) -> Result<(), &'static str> {
    if user.is_empty() || user.len() > MAX_USERNAME_LEN {
        return Err("invalid username length");
    }
    if user
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == ':' || c == '/')
    {
        return Err("invalid character in username");
    }
    Ok(())
}

pub fn validate_password(password: &str) -> Result<(), &'static str> {
    if password.is_empty() || password.len() > MAX_PASSWORD_LEN {
        return Err("invalid password length");
    }
    if password.contains('\0') {
        return Err("invalid character in password");
    }
    Ok(())
}

pub async fn write_frame<W, T>(w: &mut W, value: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let body = serde_json::to_vec(value).map_err(io::Error::other)?;
    let len = u32::try_from(body.len())
        .ok()
        .filter(|l| *l <= MAX_FRAME_BYTES)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

pub async fn read_frame<R, T>(r: &mut R) -> io::Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    serde_json::from_slice(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn frame_round_trip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let req = Request::AuthPam {
            user: "alice".into(),
            password: Secret::new("pw"),
        };
        write_frame(&mut a, &req).await.unwrap();
        let got: Request = read_frame(&mut b).await.unwrap();
        match got {
            Request::AuthPam { user, password } => {
                assert_eq!(user, "alice");
                assert_eq!(password.expose(), "pw");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_on_read() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&(MAX_FRAME_BYTES + 1).to_be_bytes())
            .await
            .unwrap();
        let err = read_frame::<_, Request>(&mut b).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn unknown_fields_and_verbs_are_rejected() {
        assert!(serde_json::from_str::<Request>(r#"{"verb":"ping","extra":1}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"verb":"rm_rf"}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"verb":"ping"}"#).is_ok());
    }

    #[test]
    fn secret_is_redacted_in_debug() {
        let req = Request::AuthPam {
            user: "a".into(),
            password: Secret::new("hunter2"),
        };
        assert!(!format!("{req:?}").contains("hunter2"));
    }

    #[test]
    fn validation() {
        assert!(validate_username("alice").is_ok());
        assert!(validate_username("").is_err());
        assert!(validate_username("a b").is_err());
        assert!(validate_username("a:b").is_err());
        assert!(validate_username("a\0b").is_err());
        assert!(validate_password("x").is_ok());
        assert!(validate_password("").is_err());
        assert!(validate_password("a\0b").is_err());
    }
}
