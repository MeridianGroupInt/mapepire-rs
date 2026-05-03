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

/// Fluent builder for [`crate::Pool`].
///
/// Construct via [`crate::Pool::builder`].
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

impl PoolBuilder {
    /// Construct the [`crate::Pool`]. Eagerly opens [`PoolBuilder::starting_size`]
    /// connections. Returns once all eager connections have completed
    /// their handshake.
    ///
    /// # Errors
    ///
    /// [`crate::Error::Internal`] if the deadpool builder rejects the
    /// configuration, or if any of the `starting_size` eager connections
    /// fails to open.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Pool, TlsConfig};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let pool = Pool::builder(server)
    ///     .max_size(4)
    ///     .starting_size(1)
    ///     .build()
    ///     .await?;
    /// # let _ = pool;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn build(self) -> crate::Result<crate::Pool> {
        use deadpool::managed::{Pool as DeadPool, Timeouts};

        let acquire_timeout = self.acquire_timeout;
        let starting_size = self.starting_size;
        // idle_timeout stored only — enforcement deferred to v0.4 (deadpool's
        // runtime-hooks integration). Suppress the unused-warning explicitly.
        let _idle_timeout = self.idle_timeout;

        let mgr = crate::pool::manager::JobManager::new(self.server);

        let inner = DeadPool::builder(mgr)
            .max_size(self.max_size)
            .timeouts(Timeouts {
                wait: acquire_timeout,
                create: None,
                recycle: None,
            })
            .build()
            .map_err(|e| crate::Error::Internal(format!("pool builder: {e}")))?;

        // Eagerly create starting_size connections. Each pool.get() returns an
        // Object<JobManager>; dropping it returns the connection to the pool's
        // idle list (deadpool handles the lifecycle).
        for _ in 0..starting_size {
            let _ = Box::pin(inner.get())
                .await
                .map_err(|e| crate::Error::Internal(format!("starting_size eager open: {e}")))?;
        }

        Ok(crate::Pool {
            inner,
            registry: std::sync::Arc::new(crate::pool::routing::Registry),
            acquire_timeout,
        })
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
