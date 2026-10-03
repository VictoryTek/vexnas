//! Thin JSON client for `/api/v1`. The browser adds `Origin` and the session
//! cookie; this adds `Content-Type` and, for writes, the CSRF token.

use gloo_net::http::{Request, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Session {
    pub username: String,
    pub role: String,
    pub csrf_token: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HelperStatus {
    pub ok: bool,
    pub version: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Meta {
    pub version: String,
    pub hostname: String,
    pub variant: Option<String>,
    pub helper: HelperStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    pub fn is_unauthorized(&self) -> bool {
        self.status == 401
    }
}

fn network(e: impl ToString) -> ApiError {
    ApiError {
        status: 0,
        message: format!("Network error: {}", e.to_string()),
    }
}

async fn parse<T: DeserializeOwned>(resp: Response) -> Result<T, ApiError> {
    if resp.ok() {
        return resp.json::<T>().await.map_err(network);
    }
    let status = resp.status();
    let message = resp
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or_else(|| format!("HTTP {status}"));
    Err(ApiError { status, message })
}

pub async fn get_json<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    parse(Request::get(path).send().await.map_err(network)?).await
}

pub async fn post_json<T: DeserializeOwned>(
    path: &str,
    csrf: Option<&str>,
    body: &impl Serialize,
) -> Result<T, ApiError> {
    let mut req = Request::post(path);
    if let Some(token) = csrf {
        req = req.header("X-CSRF-Token", token);
    }
    let req = req.json(body).map_err(network)?;
    parse(req.send().await.map_err(network)?).await
}
