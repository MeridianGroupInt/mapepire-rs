//! [`PoolBuilder`] (spec §7.2). Fluent configuration with sibling-SDK-aligned
//! field names — `max_size`, `starting_size`, `acquire_timeout`,
//! `idle_timeout`, `recycle`, `default_page_size`, `parameter_logging`.
//!
//! `PoolBuilder::build` (added in Task 10 / PRO-440) consumes the builder
//! and returns a `Pool`.

use std::sync::Arc;
use std::time::Duration;

use crate::config::DaemonServer;

/// Strategy for verifying a pooled connection on checkout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecyclingMethod {
    /// Round-trip a `ping` before handing the connection out.
    /// Default — IBM i firewalls silently kill idle TCP sessions.
    #[default]
    Verified,
    /// Trust the pool — return without checking. Fast but risky.
    Fast,
}

/// How much parameter context to surface in `tracing` spans.
///
/// Stored by the builder in v0.3, but **not yet emitted** — full `tracing`
/// instrumentation lands in v0.4. Pick the variant you'd want when v0.4 ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParameterLogging {
    /// Log nothing about parameters. Default — privacy-safe.
    #[default]
    None,
    /// Log type names and count, but no values. Useful for shape debugging.
    TypesAndCount,
    /// Log full values (dev only — never use in production).
    Full,
}

/// Fluent builder for `Pool` (added in Task 10 / PRO-440).
///
/// Construct via `Pool::builder` (Task 10).
//
// `dead_code` is allowed crate-wide on this struct's fields and constructor
// because Task 10 (PRO-440) — which adds `Pool::builder` and the `build()`
// method that consume them — has not landed yet. The `#[cfg(test)]` block
// below exercises every field via `PoolBuilder::new`, so the test build is
// fully covered; the allow only suppresses the non-test warning until Task 10
// wires the consumer.
#[allow(dead_code)]
#[must_use]
pub struct PoolBuilder {
    pub(crate) server: Arc<DaemonServer>,
    pub(crate) max_size: usize,
    pub(crate) starting_size: usize,
    pub(crate) acquire_timeout: Option<Duration>,
    pub(crate) idle_timeout: Option<Duration>,
    pub(crate) recycle: RecyclingMethod,
    pub(crate) default_page_size: u32,
    pub(crate) parameter_logging: ParameterLogging,
}

#[allow(dead_code)] // see struct-level note above; Task 10 wires the consumer.
impl PoolBuilder {
    pub(crate) fn new(server: Arc<DaemonServer>) -> Self {
        Self {
            server,
            max_size: 16,
            starting_size: 0,
            acquire_timeout: Some(Duration::from_secs(5)),
            idle_timeout: Some(Duration::from_secs(300)),
            recycle: RecyclingMethod::Verified,
            default_page_size: 100,
            parameter_logging: ParameterLogging::None,
        }
    }

    /// Maximum simultaneously-checked-out connections. Default 16.
    pub fn max_size(mut self, n: usize) -> Self {
        self.max_size = n;
        self
    }

    /// Connections to open eagerly when `PoolBuilder::build` (added in Task 10) runs. Default 0.
    pub fn starting_size(mut self, n: usize) -> Self {
        self.starting_size = n;
        self
    }

    /// Maximum wait for a free connection. `None` = block forever. Default 5s.
    pub fn acquire_timeout(mut self, d: Option<Duration>) -> Self {
        self.acquire_timeout = d;
        self
    }

    /// Maximum idle time before a connection is closed. `None` = never. Default 5min.
    ///
    /// **v0.3:** stored only — not yet enforced. deadpool 0.12's idle-timeout
    /// hook integration lands in v0.4. Setting this value today is forward-compatible.
    pub fn idle_timeout(mut self, d: Option<Duration>) -> Self {
        self.idle_timeout = d;
        self
    }

    /// Recycling strategy on checkout. Default [`RecyclingMethod::Verified`].
    pub fn recycle(mut self, m: RecyclingMethod) -> Self {
        self.recycle = m;
        self
    }

    /// `sqlmore` page size for paged result sets. Default 100.
    pub fn default_page_size(mut self, n: u32) -> Self {
        self.default_page_size = n;
        self
    }

    /// Parameter-logging policy for v0.4 `tracing` spans. Default
    /// [`ParameterLogging::None`].
    pub fn parameter_logging(mut self, p: ParameterLogging) -> Self {
        self.parameter_logging = p;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DaemonServer, TlsConfig};

    fn server() -> Arc<DaemonServer> {
        Arc::new(
            DaemonServer::builder()
                .host("h")
                .user("u")
                .password("p".into())
                .tls(TlsConfig::Verified)
                .build()
                .expect("ok"),
        )
    }

    #[test]
    fn defaults_match_spec() {
        let b = PoolBuilder::new(server());
        assert_eq!(b.max_size, 16);
        assert_eq!(b.starting_size, 0);
        assert_eq!(b.acquire_timeout, Some(Duration::from_secs(5)));
        assert_eq!(b.idle_timeout, Some(Duration::from_secs(300)));
        assert_eq!(b.recycle, RecyclingMethod::Verified);
        assert_eq!(b.default_page_size, 100);
        assert_eq!(b.parameter_logging, ParameterLogging::None);
    }

    #[test]
    fn setters_chain() {
        let b = PoolBuilder::new(server())
            .max_size(32)
            .starting_size(2)
            .acquire_timeout(None)
            .idle_timeout(Some(Duration::from_secs(60)))
            .recycle(RecyclingMethod::Fast)
            .default_page_size(50)
            .parameter_logging(ParameterLogging::TypesAndCount);
        assert_eq!(b.max_size, 32);
        assert_eq!(b.starting_size, 2);
        assert_eq!(b.acquire_timeout, None);
        assert_eq!(b.idle_timeout, Some(Duration::from_secs(60)));
        assert_eq!(b.recycle, RecyclingMethod::Fast);
        assert_eq!(b.default_page_size, 50);
        assert_eq!(b.parameter_logging, ParameterLogging::TypesAndCount);
    }
}
