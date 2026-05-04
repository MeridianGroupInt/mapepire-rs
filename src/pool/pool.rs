//! Public [`Pool`] entry point (spec §4.6).
//!
//! Wraps `deadpool::managed::Pool<JobManager>`. The pool surface itself is
//! intentionally minimal — most users construct via [`Pool::builder`], call
//! `Pool::execute` for one-shot work, or `Pool::acquire` (Task 13) for
//! transactional connections. `Pool::execute` performs the §7.3 three-tier
//! routing scan (try-idle → least-busy multiplex → fair-queue fallback).

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
    // Task 23 / PRO-453 wired up the registry: `JobManager::create` tracks
    // each new `Arc<Job>` via this shared `Arc<Registry>`. Task 24 /
    // PRO-454 reads from it in `Pool::execute`'s §7.3 routing scan.
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

    /// Execute a SQL statement using the §7.3 three-tier routing scan.
    ///
    /// Routing order (spec §7.3):
    /// 1. **Try idle**: a non-blocking `try_get` returns immediately if a pooled `Job` is idle
    ///    (`in_flight == 0`). Run on it directly.
    /// 2. **Least-busy scan**: otherwise, peek at up to `min(status().size, 8)`
    ///    currently-checked-out jobs via the routing registry and ride the lowest-`in_flight` one —
    ///    the v0.2 dispatcher already multiplexes concurrent requests on a single connection, so
    ///    this routes additional work onto a Job another caller has out without blocking on the
    ///    fair queue.
    /// 3. **Fair-queue fallback**: if no upgradeable Jobs are eligible (e.g., pool not yet warmed,
    ///    or every Job is `Reserved`'s `u32::MAX` sentinel), wait via `pool.get()` honoring
    ///    `acquire_timeout`.
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
        use crate::Job;

        // §7.3 step 1: try an immediately-idle job. We gate on
        // `status().available > 0` and then call `timeout_get` with all
        // timeouts at `Duration::ZERO` — `wait: ZERO` makes the semaphore
        // acquire non-blocking and `create: ZERO` ensures we don't block
        // on a TLS handshake if the pool happens not to be at full size
        // yet (we want a *currently idle* connection only). Any `Err`
        // (Timeout, Backend, …) is treated as "no idle slot available
        // right now" and we fall through to the scan / fair-queue
        // fallback — real backend errors resurface on `get_or_timeout`.
        if self.inner.status().available > 0 {
            let nb = deadpool::managed::Timeouts {
                wait: Some(Duration::ZERO),
                create: Some(Duration::ZERO),
                recycle: Some(Duration::ZERO),
            };
            if let Ok(obj) = Box::pin(self.inner.timeout_get(&nb)).await {
                if obj.in_flight() == 0 {
                    return Job::execute(&obj, sql).await;
                }
                // Idle slot returned but the underlying Job is mid-flight
                // (a caller dropped a future without awaiting; the
                // Object is still unowned). Drop the checkout — the scan
                // / fair-queue fallback picks a better target.
                drop(obj);
            }
        }

        // §7.3 step 2: scan up to min(max_size, 8) checked-out jobs and
        // route through the least-busy upgradeable one. The Arc keeps
        // the Job alive for the duration of this request even though
        // the deadpool slot belongs to whoever currently has the
        // Object<JobManager> checked out — the v0.2 dispatcher
        // multiplexes concurrent requests on a single connection.
        let limit = std::cmp::min(self.inner.status().size, 8);
        let candidates = self.registry.least_busy(limit);
        if let Some(arc) = candidates.into_iter().next() {
            return Job::execute(&arc, sql).await;
        }

        // §7.3 step 3: fall back to fair queueing (waits up to
        // `acquire_timeout`).
        let obj = self.get_or_timeout().await?;
        Job::execute(&obj, sql).await
    }

    /// Execute a parameterized SQL statement using the §7.3 three-tier
    /// routing scan.
    ///
    /// Routes identically to [`Pool::execute`] (try-idle → least-busy
    /// scan → fair-queue fallback) and forwards `params` through to
    /// [`crate::Job::execute_with`].
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
        use crate::Job;

        // §7.3 step 1: try an immediately-idle job (see `execute` for
        // the rationale on the all-zero-timeouts non-blocking checkout
        // and dropping its errors silently).
        if self.inner.status().available > 0 {
            let nb = deadpool::managed::Timeouts {
                wait: Some(Duration::ZERO),
                create: Some(Duration::ZERO),
                recycle: Some(Duration::ZERO),
            };
            if let Ok(obj) = Box::pin(self.inner.timeout_get(&nb)).await {
                if obj.in_flight() == 0 {
                    return Job::execute_with(&obj, sql, params).await;
                }
                drop(obj);
            }
        }

        // §7.3 step 2: least-busy scan over checked-out jobs.
        let limit = std::cmp::min(self.inner.status().size, 8);
        let candidates = self.registry.least_busy(limit);
        if let Some(arc) = candidates.into_iter().next() {
            return Job::execute_with(&arc, sql, params).await;
        }

        // §7.3 step 3: fall back to fair queueing.
        let obj = self.get_or_timeout().await?;
        Job::execute_with(&obj, sql, params).await
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
