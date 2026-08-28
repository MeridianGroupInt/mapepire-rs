//! [`PoolBuilder`] (spec §7.2). Fluent configuration with sibling-SDK-aligned
//! field names — `max_size`, `starting_size`, `acquire_timeout`,
//! `idle_timeout`, `default_page_size`, `parameter_logging`.
//!
//! Recycle on deadpool checkout always pings (`Job::ping`). There is no
//! skip-ping Fast path: IBM i firewalls silently kill idle TCP.
//!
//! `PoolBuilder::build` (added in Task 10 / PRO-440) consumes the builder
//! and returns a `Pool`.

use std::sync::Arc;
use std::time::Duration;

use crate::config::DaemonServer;

/// How much parameter context to surface in `tracing` spans.
///
/// **v0.4+:** enforced — the [`crate::Pool`] reads this value when emitting
/// span fields on [`crate::Pool::execute_with`]. Direct
/// [`crate::Job::execute_with`] users get [`ParameterLogging::None`]
/// semantics (no parameter values on spans — `param_count` is the only
/// param-related field).
///
/// Effective only when the `tracing` feature is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParameterLogging {
    /// Default — privacy-safe. Spans carry only `param_count`, no
    /// `param_types` or `params` field.
    #[default]
    None,
    /// Spans carry `param_types` (an array of JSON value type names like
    /// `"String"`, `"Number"`). No values. Useful for shape debugging.
    TypesAndCount,
    /// Spans carry `params` with full `Debug`-formatted values.
    /// **Dev only — never use in production.**
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
    /// **Enforcement (v0.4 / Task 15 / PRO-593).** When `Some(d)`, [`Self::build`]
    /// spawns a background reaper task that wakes every `d / 4` (clamped to
    /// `[1s, 60s]`) and calls `deadpool::managed::Pool::retain` with the
    /// predicate `metrics.last_used() < d`. An idle connection is therefore
    /// reaped within `d..=d * 1.25` of its last use. The reaper is aborted on
    /// the last [`crate::Pool`] clone's drop so it does not leak across the
    /// lifetime of the pool itself.
    ///
    /// **Interaction with [`Self::acquire_timeout`].** The reaper only removes
    /// **idle** (returned-to-pool) connections; in-flight checkouts are
    /// untouched. A subsequent acquire that finds the pool drained will pay a
    /// fresh-connect cost (bounded by `acquire_timeout`), exactly as if the
    /// pool had not yet warmed up.
    ///
    /// **Note on `last_used()` semantics.** deadpool's `Metrics::last_used()`
    /// returns "time since last successful recycle on acquire" (or creation
    /// if never recycled). Each checkout runs `Job::ping`, so the timestamp
    /// matches "time since last use" closely for `acquire` / fair-queue
    /// checkout. The registry idle fast path on [`crate::Pool::execute`]
    /// does not recycle; idle reaping still uses the last checkout ping.
    pub fn idle_timeout(mut self, d: Option<Duration>) -> Self {
        self.idle_timeout = d;
        self
    }

    /// First-page size when [`crate::ExecuteOptions::rows`] is `None` on
    /// pool `execute` / `execute_with` / `execute_opts` /
    /// `execute_with_opts`. Default 100 (mapepire-js). Direct
    /// [`crate::Job::execute`] stays 100 unless the caller passes
    /// `execute_opts`. `Some(0)` on opts is still
    /// [`crate::ProtocolError::ZeroPageSize`] and is not sent.
    pub fn default_page_size(mut self, n: u32) -> Self {
        self.default_page_size = n;
        self
    }

    /// Parameter-logging policy for `tracing` spans on
    /// [`crate::Pool::execute_with`]. Default [`ParameterLogging::None`].
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
        use deadpool::Runtime;
        use deadpool::managed::{Pool as DeadPool, Timeouts};

        let acquire_timeout = self.acquire_timeout;
        let starting_size = self.starting_size;
        let idle_timeout = self.idle_timeout;

        // ONE registry Arc shared between Pool and JobManager — the manager
        // clones it on `create()` to register new Jobs, and the Pool reads
        // from it during the §7.3 routing scan (Task 24 / PRO-454).
        let registry = Arc::new(crate::pool::routing::Registry::default());
        let mgr = crate::pool::manager::JobManager::new(self.server, Arc::clone(&registry));

        // `.runtime(Runtime::Tokio1)` is REQUIRED whenever any timeout is set —
        // deadpool's builder errors with `NoRuntimeSpecified` otherwise (the
        // wait timer is driven by the runtime's sleep impl). The crate enables
        // deadpool's `rt_tokio_1` feature for exactly this reason. We always
        // set it because the default `acquire_timeout` is `Some(5s)` and
        // callers who pass `None` still pay nothing for the registration.
        //
        // `Timeouts.recycle` is NOT the idle-timeout — it's the deadline
        // applied to `Manager::recycle()` itself. deadpool 0.13 has no native
        // idle-timeout knob; we enforce it via a periodic reaper task spawned
        // below (Task 15 / PRO-593).
        let inner = DeadPool::builder(mgr)
            .max_size(self.max_size)
            .runtime(Runtime::Tokio1)
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

        // Spawn the idle-connection reaper if `idle_timeout` is set. The
        // returned `JoinHandle` is owned by an `Arc<ReaperGuard>` on `Pool`
        // so it gets aborted when the last clone drops — see `ReaperGuard`.
        let reaper = idle_timeout
            .and_then(|d| crate::pool::runtime::reaper_period(d).map(|p| (d, p)))
            .map(|(timeout, period)| {
                let handle = crate::pool::runtime::spawn_idle_reaper(&inner, timeout, period);
                Arc::new(crate::pool::runtime::ReaperGuard { handle })
            });

        Ok(crate::Pool {
            inner,
            registry,
            acquire_timeout,
            default_page_size: self.default_page_size,
            parameter_logging: self.parameter_logging,
            _reaper: reaper,
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
                .expect("DaemonServer builds with all required fields set"),
        )
    }

    #[test]
    fn defaults_match_spec() {
        let b = PoolBuilder::new(server());
        assert_eq!(b.max_size, 16);
        assert_eq!(b.starting_size, 0);
        assert_eq!(b.acquire_timeout, Some(Duration::from_secs(5)));
        assert_eq!(b.idle_timeout, Some(Duration::from_secs(300)));
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
            .default_page_size(50)
            .parameter_logging(ParameterLogging::TypesAndCount);
        assert_eq!(b.max_size, 32);
        assert_eq!(b.starting_size, 2);
        assert_eq!(b.acquire_timeout, None);
        assert_eq!(b.idle_timeout, Some(Duration::from_secs(60)));
        assert_eq!(b.default_page_size, 50);
        assert_eq!(b.parameter_logging, ParameterLogging::TypesAndCount);
    }
}
