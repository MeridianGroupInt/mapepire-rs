//! Public [`Pool`] entry point (spec §4.6).
//!
//! Wraps `deadpool::managed::Pool<JobManager>`. The pool surface itself is
//! intentionally minimal — most users construct via [`Pool::builder`], call
//! `Pool::execute` (Task 11) for one-shot work, or `Pool::acquire`
//! (Task 13) for transactional connections. Routing scan landing in Task 24
//! / PRO-454 will replace the naive checkout.

use std::sync::Arc;
use std::time::Duration;

/// Snapshot of pool state — re-exported from `deadpool::Status`.
///
/// Exposes `size` (current pool size), `available` (idle connections), and
/// `waiters` (futures blocked on `pool.get()`). Re-exported here so callers
/// don't need to depend on `deadpool` directly.
pub use deadpool::Status as PoolStatus;
use deadpool::managed::Pool as DeadPool;

use crate::config::DaemonServer;
use crate::pool::builder::PoolBuilder;
use crate::pool::manager::JobManager;
use crate::pool::routing::Registry;

/// Connection pool for one or more [`crate::Job`] connections to a single
/// Mapepire daemon.
///
/// Construct via [`Pool::builder`]. `Pool` is `Clone` — clones share the
/// same underlying deadpool runtime and registry.
///
/// `Pool::acquire` (Task 13) lands in a subsequent task of v0.3 Phase 5.
#[derive(Clone)]
pub struct Pool {
    pub(crate) inner: DeadPool<JobManager>,
    // Task 23 / PRO-453 wired up the registry: `JobManager::create` now
    // tracks each new `Arc<Job>` here via a shared `Arc<Registry>`. The
    // `Pool` itself doesn't yet read from the field — the §7.3 routing
    // scan in `Pool::execute` lands in Task 24 / PRO-454. Until then this
    // field-level `dead_code` allow narrows the suppression to the one
    // truly-dead read site rather than blanket-allowing the struct.
    #[allow(dead_code)]
    pub(crate) registry: Arc<Registry>,
    pub(crate) acquire_timeout: Option<Duration>,
}

impl Pool {
    /// Begin building a `Pool` from a [`DaemonServer`] (or `Arc<DaemonServer>`).
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
    /// let pool = Pool::builder(server).max_size(8).build().await?;
    /// # let _ = pool;
    /// # Ok(())
    /// # }
    /// ```
    pub fn builder(server: impl Into<Arc<DaemonServer>>) -> PoolBuilder {
        PoolBuilder::new(server.into())
    }

    /// Execute a SQL statement on the next-available pooled job.
    ///
    /// Naive checkout: `pool.get()` → run on `&Job` → return to pool on
    /// drop. The least-busy-job routing scan lands in Task 24 / PRO-454.
    ///
    /// # Errors
    ///
    /// As [`crate::Job::execute`], plus [`crate::Error::PoolExhausted`]
    /// if the pool's `acquire_timeout` elapses before a connection is
    /// free. Backend errors during checkout (e.g., a failed
    /// `JobManager::create` handshake) propagate as the original
    /// [`crate::Error`] variant.
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
    /// let pool = Pool::builder(server).max_size(2).build().await?;
    /// let rows = pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1").await?;
    /// # let _ = rows;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute(&self, sql: &str) -> crate::Result<crate::query::Rows> {
        let obj = self.get_or_timeout().await?;
        crate::Job::execute(&obj, sql).await
    }

    /// Execute a parameterized SQL statement on the next-available pooled job.
    ///
    /// # Errors
    ///
    /// As [`Pool::execute`].
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
    /// let pool = Pool::builder(server).max_size(2).build().await?;
    /// let rows = pool
    ///     .execute_with(
    ///         "SELECT * FROM ORDERS WHERE CUSTNO = ?",
    ///         &[serde_json::json!(42)],
    ///     )
    ///     .await?;
    /// # let _ = rows;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_with(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> crate::Result<crate::query::Rows> {
        let obj = self.get_or_timeout().await?;
        crate::Job::execute_with(&obj, sql, params).await
    }

    /// Reserve a single connection. The returned [`crate::Reserved`] holds the
    /// connection until drop — `BEGIN`/`COMMIT` are guaranteed to land on
    /// the same Db2 job (spec §7.4).
    ///
    /// While reserved, the underlying `Job`'s `in_flight` counter is set
    /// to `u32::MAX` (a routing-skip sentinel) so the pool's least-busy-job
    /// scan never picks this connection for one-shot work.
    ///
    /// # Errors
    ///
    /// As [`Pool::execute`].
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
    /// let pool = Pool::builder(server).max_size(2).build().await?;
    /// let conn = pool.acquire().await?;
    /// conn.execute("BEGIN").await?;
    /// conn.execute("COMMIT").await?;
    /// # Ok(()) }
    /// ```
    pub async fn acquire(&self) -> crate::Result<crate::pool::reserved::Reserved> {
        let obj = self.get_or_timeout().await?;
        Ok(crate::pool::reserved::Reserved::new(obj))
    }

    /// Snapshot of pool size, idle, and waiter counts (spec §7.5).
    ///
    /// `PoolStatus` is `Copy + Debug`; cheap to call repeatedly. The
    /// invariant `status().size <= max_size` always holds.
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
    /// let pool = Pool::builder(server).max_size(8).build().await?;
    /// let s = pool.status();
    /// assert!(s.size <= 8);
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn status(&self) -> PoolStatus {
        self.inner.status()
    }

    /// Check out an `Object<JobManager>` from the underlying deadpool, mapping
    /// `PoolError` into the crate's [`crate::Error`] type.
    ///
    /// `Box::pin` matches the `clippy::large_futures` precedent from Task 8 —
    /// `inner.get()`'s state machine contains the manager's `create()` future,
    /// which contains a full TLS handshake + first request/response cycle.
    ///
    /// When `acquire_timeout` is `None`, deadpool blocks indefinitely so the
    /// `PoolError::Timeout` arm is unreachable; the `unwrap_or_default()` →
    /// `Duration::ZERO` only ever surfaces in the error message itself, never
    /// as a real elapsed timeout.
    async fn get_or_timeout(
        &self,
    ) -> crate::Result<deadpool::managed::Object<crate::pool::manager::JobManager>> {
        use deadpool::managed::PoolError;
        Box::pin(self.inner.get()).await.map_err(|e| match e {
            PoolError::Timeout(_) => crate::Error::PoolExhausted {
                timeout: self.acquire_timeout.unwrap_or_default(),
            },
            PoolError::Backend(b) => b,
            other => crate::Error::Internal(format!("pool: {other}")),
        })
    }
}
