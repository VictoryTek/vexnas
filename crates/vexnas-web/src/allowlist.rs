//! Source-address allowlist, enforced at accept time — before the TLS handshake —
//! so addresses outside the allowlist get a closed connection and nothing else.

use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;

use axum_server::accept::Accept;
use ipnet::IpNet;
use tokio::net::TcpStream;

#[derive(Debug, Clone)]
pub struct Allowlist {
    nets: Arc<Vec<IpNet>>,
}

impl Allowlist {
    pub fn parse(cidrs: &[String]) -> Result<Self, String> {
        let mut nets = Vec::with_capacity(cidrs.len());
        for c in cidrs {
            let net = c
                .parse::<IpNet>()
                .or_else(|_| c.parse::<IpAddr>().map(IpNet::from))
                .map_err(|_| format!("invalid CIDR/IP in allowed_cidrs: {c:?}"))?;
            nets.push(net);
        }
        Ok(Self {
            nets: Arc::new(nets),
        })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        // Dual-stack sockets report IPv4 peers as ::ffff:a.b.c.d.
        let ip = ip.to_canonical();
        self.nets.iter().any(|n| n.contains(&ip))
    }
}

/// Wraps another acceptor (the TLS one) and refuses disallowed peers first.
#[derive(Clone, Debug)]
pub struct AllowlistAcceptor<A> {
    inner: A,
    allow: Allowlist,
}

impl<A> AllowlistAcceptor<A> {
    pub fn new(inner: A, allow: Allowlist) -> Self {
        Self { inner, allow }
    }
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send>>;

impl<A, S> Accept<TcpStream, S> for AllowlistAcceptor<A>
where
    A: Accept<TcpStream, S>,
    A::Future: Send + 'static,
{
    type Stream = A::Stream;
    type Service = A::Service;
    type Future = BoxFuture<(A::Stream, A::Service)>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        let allowed = stream
            .peer_addr()
            .map(|a| self.allow.contains(a.ip()))
            .unwrap_or(false);
        if !allowed {
            tracing::debug!(peer = ?stream.peer_addr().ok(), "connection refused: source not allowed");
            return Box::pin(async {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "source address not allowed",
                ))
            });
        }
        Box::pin(self.inner.accept(stream, service))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(c: &[&str]) -> Allowlist {
        Allowlist::parse(&c.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn matches_cidrs_and_bare_ips() {
        let a = list(&["192.168.0.0/16", "100.64.0.0/10", "::1", "fc00::/7"]);
        assert!(a.contains("192.168.1.5".parse().unwrap()));
        assert!(a.contains("100.100.1.1".parse().unwrap()));
        assert!(a.contains("::1".parse().unwrap()));
        assert!(a.contains("fd00::1".parse().unwrap()));
        assert!(!a.contains("8.8.8.8".parse().unwrap()));
        assert!(!a.contains("2001:db8::1".parse().unwrap()));
    }

    #[test]
    fn ipv4_mapped_addresses_are_canonicalised() {
        let a = list(&["10.0.0.0/8"]);
        assert!(a.contains("::ffff:10.1.2.3".parse().unwrap()));
        assert!(!a.contains("::ffff:8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn invalid_entries_are_errors_and_empty_denies_all() {
        assert!(Allowlist::parse(&["nope".to_string()]).is_err());
        assert!(!list(&[]).contains("127.0.0.1".parse().unwrap()));
    }
}
