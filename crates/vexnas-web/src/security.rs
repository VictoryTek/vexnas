//! Security headers and the Content-Security-Policy.
//!
//! Trunk injects an inline `<script type="module">` that boots the WASM bundle,
//! which a plain `script-src 'self'` would block. Instead of weakening the policy
//! with `'unsafe-inline'`, the CSP is computed at startup from the actual
//! `index.html`: each inline script is allowed by its SHA-256 hash only.

use axum::extract::{Request, State};
use axum::http::{header, HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use sha2::{Digest, Sha256};

/// `<script ...>inline</script>` bodies of `html` (scripts with `src=` are skipped).
pub fn inline_scripts(html: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find("<script") {
        let after = &rest[start..];
        let Some(tag_end) = after.find('>') else {
            break;
        };
        let tag = &after[..tag_end];
        let body_start = tag_end + 1;
        let Some(close) = after[body_start..].find("</script>") else {
            break;
        };
        if !tag.contains(" src=") && !tag.contains("\tsrc=") && !tag.contains("\nsrc=") {
            out.push(&after[body_start..body_start + close]);
        }
        rest = &after[body_start + close + "</script>".len()..];
    }
    out
}

pub fn compute_csp(index_html: Option<&str>) -> String {
    let hashes: String = index_html
        .map(inline_scripts)
        .unwrap_or_default()
        .into_iter()
        .map(|s| {
            format!(
                " 'sha256-{}'",
                STANDARD.encode(Sha256::digest(s.as_bytes()))
            )
        })
        .collect();
    format!(
        "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'{hashes}; style-src 'self'; \
         img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; \
         base-uri 'none'; form-action 'self'"
    )
}

#[derive(Clone)]
pub struct SecurityHeaders {
    csp: HeaderValue,
}

impl SecurityHeaders {
    pub fn new(csp: &str) -> Self {
        Self {
            csp: HeaderValue::from_str(csp).expect("CSP is ASCII"),
        }
    }
}

pub async fn security_headers(
    State(h): State<SecurityHeaders>,
    req: Request,
    next: Next,
) -> Response {
    let is_api = req.uri().path().starts_with("/api/");
    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, h.csp.clone());
    headers.insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=31536000"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    if is_api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_inline_scripts() {
        let html = r#"<head><script src="/a.js"></script><script type="module">import x from '/x.js';</script>
            <script>var a=1;</script><link rel="stylesheet" href="/s.css"></head>"#;
        assert_eq!(
            inline_scripts(html),
            vec!["import x from '/x.js';", "var a=1;"]
        );
    }

    #[test]
    fn csp_hashes_inline_scripts_and_never_allows_unsafe_inline() {
        let csp = compute_csp(Some("<script>abc</script>"));
        // sha256("abc") = ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=
        assert!(csp.contains("'sha256-ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0='"));
        assert!(!csp.contains("unsafe-inline"));
        assert!(csp.contains("frame-ancestors 'none'"));
    }

    #[test]
    fn csp_without_index_has_no_hashes() {
        assert!(!compute_csp(None).contains("sha256"));
    }

    #[test]
    fn unterminated_script_does_not_loop_or_panic() {
        assert!(inline_scripts("<script>never closed").is_empty());
        assert!(inline_scripts("<script").is_empty());
    }
}
