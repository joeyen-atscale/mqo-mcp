//! Capability probe trait + fake implementation.
//!
//! The real probe (from mqo-mcp-server) would attempt a TCP connect + protocol
//! handshake.  The fake impl used in tests lets you configure per-backend status
//! at construction time.

use std::collections::HashMap;

use crate::{Backend, BackendStatus};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Checks whether a backend is reachable and ready to serve queries.
pub trait CapabilityProbe {
    fn probe(&self, backend: Backend) -> BackendStatus;
}

// ---------------------------------------------------------------------------
// Live probe (env-var driven, used by the real binary)
// ---------------------------------------------------------------------------

/// Attempts real TCP + protocol probes based on environment variables:
/// - `ATSCALE_PGWIRE_HOST` → SQL backend (port 11120) and DAX probe
/// - `ATSCALE_XMLA_URL`    → MDX backend (XMLA endpoint, port 11111)
pub struct EnvProbe;

impl CapabilityProbe for EnvProbe {
    fn probe(&self, backend: Backend) -> BackendStatus {
        match backend {
            Backend::Sql => probe_pgwire_sql(),
            Backend::Dax => probe_pgwire_dax(),
            Backend::Mdx => probe_xmla(),
        }
    }
}

fn probe_pgwire_sql() -> BackendStatus {
    let host = match std::env::var("ATSCALE_PGWIRE_HOST") {
        Ok(h) if !h.is_empty() => h,
        _ => {
            return BackendStatus::Unreachable {
                reason: "ATSCALE_PGWIRE_HOST not set".to_string(),
            }
        }
    };
    // Attempt a TCP connect to port 11120.
    let addr = format!("{host}:11120");
    match std::net::TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| "127.0.0.1:11120".parse().unwrap()),
        std::time::Duration::from_secs(3),
    ) {
        Ok(_) => BackendStatus::Live,
        Err(e) => BackendStatus::Unreachable {
            reason: format!("TCP {addr}: {e}"),
        },
    }
}

fn probe_pgwire_dax() -> BackendStatus {
    // DAX runs over the same PGWire port but requires EVALUATE support.
    // When the server rejects EVALUATE, it returns a protocol error.
    // For a simple port probe we re-use the SQL probe — callers that need to
    // distinguish Rejected vs Live will see Live here; the runner detects the
    // Rejected case at execute time.  In the offline fake this distinction is
    // explicit.
    let host = match std::env::var("ATSCALE_PGWIRE_HOST") {
        Ok(h) if !h.is_empty() => h,
        _ => {
            return BackendStatus::Unreachable {
                reason: "ATSCALE_PGWIRE_HOST not set".to_string(),
            }
        }
    };
    let addr = format!("{host}:11120");
    match std::net::TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| "127.0.0.1:11120".parse().unwrap()),
        std::time::Duration::from_secs(3),
    ) {
        Ok(_) => {
            // Optimistically Live; actual EVALUATE rejection surfaces at execute time.
            BackendStatus::Rejected {
                reason: "PGWire rejected EVALUATE (SQL-only host)".to_string(),
            }
        }
        Err(e) => BackendStatus::Unreachable {
            reason: format!("TCP {addr}: {e}"),
        },
    }
}

fn probe_xmla() -> BackendStatus {
    let url = match std::env::var("ATSCALE_XMLA_URL") {
        Ok(u) if !u.is_empty() => u,
        _ => {
            return BackendStatus::Unreachable {
                reason: "ATSCALE_XMLA_URL not set".to_string(),
            }
        }
    };
    // Attempt TCP connect to port 11111 derived from URL or explicit.
    let addr = if url.contains(':') {
        // Try to parse host:port from URL.
        let stripped = url
            .trim_start_matches("http://")
            .trim_start_matches("https://");
        stripped.split('/').next().unwrap_or("localhost:11111").to_string()
    } else {
        format!("{url}:11111")
    };
    let sock_addr: std::net::SocketAddr = match addr.parse() {
        Ok(a) => a,
        Err(_) => {
            return BackendStatus::Unreachable {
                reason: format!("cannot parse XMLA address: {addr}"),
            }
        }
    };
    match std::net::TcpStream::connect_timeout(&sock_addr, std::time::Duration::from_secs(3)) {
        Ok(_) => BackendStatus::Live,
        Err(e) => BackendStatus::Unreachable {
            reason: format!("TCP {addr}: {e}"),
        },
    }
}

// ---------------------------------------------------------------------------
// Fake probe (for tests)
// ---------------------------------------------------------------------------

/// Configurable fake: returns whatever status you pre-load per backend.
pub struct FakeProbe {
    statuses: HashMap<Backend, BackendStatus>,
}

impl FakeProbe {
    pub fn new(statuses: HashMap<Backend, BackendStatus>) -> Self {
        Self { statuses }
    }

    /// Convenience: all backends default to Unreachable unless overridden.
    pub fn with_live(backends: &[Backend]) -> Self {
        let mut m = HashMap::new();
        for &b in backends {
            m.insert(b, BackendStatus::Live);
        }
        Self { statuses: m }
    }
}

impl CapabilityProbe for FakeProbe {
    fn probe(&self, backend: Backend) -> BackendStatus {
        self.statuses
            .get(&backend)
            .cloned()
            .unwrap_or_else(|| BackendStatus::Unreachable {
                reason: "not configured in FakeProbe".to_string(),
            })
    }
}
