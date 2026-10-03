//! TLS material: a user-supplied certificate, or a self-signed one generated on
//! first start (ECDSA P-256) and regenerated when the host's names/addresses change.

use std::io::Write;
use std::net::IpAddr;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use axum_server::tls_rustls::RustlsConfig;

use crate::config::TlsConfig;

/// Regenerate a self-signed certificate this long before its 825-day lifetime ends.
const MAX_AGE: Duration = Duration::from_secs(60 * 60 * 24 * 700);
const RELOAD_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Names/addresses the self-signed certificate must cover.
pub fn desired_sans(hostname: &str) -> Vec<String> {
    let mut sans: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    let host = hostname.trim();
    if !host.is_empty() && host != "localhost" {
        sans.push(host.to_string());
        if !host.ends_with(".local") {
            sans.push(format!("{host}.local"));
        }
    }
    if let Ok(addrs) = if_addrs::get_if_addrs() {
        for a in addrs {
            let ip = a.ip();
            if !ip.is_loopback() && !is_link_local(ip) {
                sans.push(ip.to_string());
            }
        }
    }
    sans.sort();
    sans.dedup();
    sans
}

fn is_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

struct Paths {
    cert: PathBuf,
    key: PathBuf,
    sans: PathBuf,
}

impl Paths {
    fn new(data_dir: &Path) -> Self {
        let dir = data_dir.join("tls");
        Self {
            cert: dir.join("cert.pem"),
            key: dir.join("key.pem"),
            sans: dir.join("sans.txt"),
        }
    }
}

fn write_private(path: &Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("create {}", tmp.display()))?;
    f.write_all(contents.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))
}

fn needs_regeneration(p: &Paths, sans: &[String]) -> bool {
    let (Ok(meta), Ok(_), Ok(stored)) = (
        std::fs::metadata(&p.cert),
        std::fs::metadata(&p.key),
        std::fs::read_to_string(&p.sans),
    ) else {
        return true;
    };
    let expired = meta
        .modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_none_or(|age| age > MAX_AGE);
    expired || stored.lines().map(str::to_string).collect::<Vec<_>>() != sans
}

fn generate(p: &Paths, hostname: &str, sans: &[String]) -> Result<()> {
    use rcgen::{CertificateParams, DnType, KeyPair};

    std::fs::create_dir_all(p.cert.parent().expect("tls dir"))?;
    let mut params = CertificateParams::new(sans.to_vec()).context("invalid subject alt name")?;
    params
        .distinguished_name
        .push(DnType::CommonName, format!("vexnas on {hostname}"));
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(825);

    let key_pair = KeyPair::generate().context("generate key")?;
    let cert = params.self_signed(&key_pair).context("self-sign")?;

    write_private(&p.key, &key_pair.serialize_pem())?;
    write_private(&p.cert, &cert.pem())?;
    write_private(&p.sans, &(sans.join("\n") + "\n"))?;
    tracing::info!(sans = ?sans, "generated self-signed TLS certificate");
    Ok(())
}

fn ensure_self_signed(data_dir: &Path, hostname: &str) -> Result<Paths> {
    let p = Paths::new(data_dir);
    let sans = desired_sans(hostname);
    if needs_regeneration(&p, &sans) {
        generate(&p, hostname, &sans)?;
    }
    Ok(p)
}

/// Build the rustls config and, if useful, return the loop that keeps it fresh.
pub async fn setup(
    cfg: &TlsConfig,
    data_dir: &Path,
    hostname: &str,
) -> Result<(RustlsConfig, tokio::task::JoinHandle<()>)> {
    // ring everywhere (no aws-lc, no cmake). Ignore "already installed".
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (cert, key, generated) = match (&cfg.cert_file, &cfg.key_file) {
        (Some(c), Some(k)) => (c.clone(), k.clone(), false),
        _ => {
            let p = ensure_self_signed(data_dir, hostname)?;
            (p.cert, p.key, true)
        }
    };
    let config = RustlsConfig::from_pem_file(&cert, &key)
        .await
        .with_context(|| format!("load TLS certificate {}", cert.display()))?;

    let reload_cfg = config.clone();
    let data_dir = data_dir.to_path_buf();
    let hostname = hostname.to_string();
    let task = tokio::spawn(async move {
        loop {
            tokio::time::sleep(RELOAD_INTERVAL).await;
            if generated {
                if let Err(e) = ensure_self_signed(&data_dir, &hostname) {
                    tracing::warn!("self-signed certificate refresh failed: {e:#}");
                    continue;
                }
            }
            if let Err(e) = reload_cfg.reload_from_pem_file(&cert, &key).await {
                tracing::warn!("TLS reload failed: {e}");
            }
        }
    });
    Ok((config, task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sans_cover_hostname_and_loopback_and_are_unique() {
        let sans = desired_sans("nas1");
        for want in ["localhost", "127.0.0.1", "::1", "nas1", "nas1.local"] {
            assert!(sans.iter().any(|s| s == want), "missing {want}: {sans:?}");
        }
        let mut sorted = sans.clone();
        sorted.dedup();
        assert_eq!(sans, sorted);
        assert!(!sans.iter().any(|s| s.starts_with("fe80")));
    }

    #[test]
    fn generate_then_reuse_then_regenerate_on_san_change() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::new(dir.path());
        let sans = vec!["localhost".to_string(), "127.0.0.1".to_string()];
        assert!(needs_regeneration(&p, &sans));
        generate(&p, "test", &sans).unwrap();
        assert!(!needs_regeneration(&p, &sans));
        let mut other = sans.clone();
        other.push("10.0.0.9".into());
        assert!(needs_regeneration(&p, &other));

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&p.key).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[tokio::test]
    async fn setup_produces_a_loadable_config() {
        let dir = tempfile::tempdir().unwrap();
        let (cfg, task) = setup(&TlsConfig::default(), dir.path(), "test")
            .await
            .unwrap();
        assert!(!cfg.get_inner().alpn_protocols.is_empty());
        task.abort();
    }
}
