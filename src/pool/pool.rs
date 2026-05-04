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
use crate::pool::builder::{ParameterLogging, PoolBuilder};
use crate::pool::manager::JobManager;
use crate::pool::routing::Registry;

/// Owns a [`tokio::task::JoinHandle`] and aborts it on drop.
///
/// `Pool` is `Clone` and the idle reaper task must outlive *every* clone — but
/// no longer. Wrapping the join handle in an `Arc<ReaperGuard>` (added Task 15
/// / PRO-593) gives reference-count semantics: the inner `Drop` runs exactly
/// once, when the last `Pool` clone is dropped, and aborts the spawned task.
/// Without this, either the reaper would leak (no abort) or it would die after
/// the first clone disappeared (abort-on-clone-drop) — both are wrong.
pub(crate) struct ReaperGuard {
    pub(crate) handle: tokio::task::JoinHandle<()>,
}

impl Drop for ReaperGuard {
    fn drop(&mut self) {
        // Abort is fire-and-forget; we deliberately don't await the handle to
        // avoid blocking `Drop`. The reaper task itself owns only a `WeakPool`
        // so its body cannot extend `PoolInner`'s lifetime past this point.
        self.handle.abort();
    }
}

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
    /// Per-pool parameter-logging policy for `tracing` span fields.
    /// Read by `Pool::execute_with` to gate `param_types` / `params` emission
    /// (Task 9 / PRO-587). Effective only when the `tracing` feature is
    /// enabled — when it isn't, this field is dead-store but kept on the
    /// struct so the public builder API stays stable across feature combos.
    #[cfg_attr(not(feature = "tracing"), allow(dead_code))]
    pub(crate) parameter_logging: ParameterLogging,
    /// Idle-connection reaper task, alive iff `idle_timeout` was `Some(..)`.
    /// Wrapped in an `Arc` so cloning `Pool` is cheap and the abort fires only
    /// when the **last** clone drops (Task 15 / PRO-593). The field exists in
    /// every build but holds `None` when no reaper was spawned (`idle_timeout`
    /// disabled).
    pub(crate) _reaper: Option<Arc<ReaperGuard>>,
}

/// Per-job saturation threshold beyond which the routing scan stops
/// preferring the pinned Job and waits for an idle one instead (spec §7.3
/// step 3).
///
/// Picked at 32 outstanding requests per dispatcher: below that, the v0.2
/// dispatcher's mpsc(64) outbound queue + per-Job `pending` `HashMap` absorb
/// concurrent multiplexing without back-pressure stalls; above it, queue
/// depth and per-response demux overhead start to dominate. The threshold
/// is intentionally a hard-coded constant for v0.3 — making it
/// configurable adds API surface without strong evidence the right value
/// is workload-dependent. v0.4 may revisit if real traffic shows a need.
const SATURATION_THRESHOLD: u32 = 32;

impl Pool {
    /// Pick the least-busy upgradeable Job from the registry that is
    /// **below** [`SATURATION_THRESHOLD`]. Returns `None` if every
    /// candidate is at or above saturation (callers fall through to
    /// fair queueing — spec §7.3 step 3).
    ///
    /// Scans up to `min(status().size, 8)` candidates per the spec's
    /// "wide enough to find a winner, narrow enough to stay cheap" heuristic.
    fn pick_unsaturated(&self) -> Option<std::sync::Arc<crate::Job>> {
        let limit = std::cmp::min(self.inner.status().size, 8);
        let mut candidates = self.registry.least_busy(limit);
        candidates.retain(|j| j.in_flight() < SATURATION_THRESHOLD);
        candidates.into_iter().next()
    }

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
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(sql = %sql, tier = tracing::field::Empty))
    )]
    pub async fn execute(&self, sql: &str) -> crate::Result<crate::query::Rows> {
        use crate::Job;

        // Pool status gauges — snapshot once per call. Pool size doesn't
        // change frequently within a single execute, so we avoid emitting
        // in the hot routing-scan loop. Downstream samplers (Prometheus
        // scrape, etc.) get a per-call data point which is plenty.
        #[cfg(feature = "metrics")]
        emit_pool_status_gauges(&self.inner.status());

        // §7.3 step 1 (v0.4 / Task 22 / PRO-600): registry-backed fast path.
        // Replaces v0.3's `timeout_get(recycle: ZERO)` which thrashed connections
        // on real IBM i ping RTT. We don't go through deadpool's checkout here,
        // so no recycle ping fires — liveness is verified at next dispatch
        // (Job::execute will surface a transport error if the socket is dead, and
        // the caller's retry / fair-queue fallback handles it).
        if let Some(arc) = self.registry.peek_idle() {
            #[cfg(feature = "tracing")]
            tracing::Span::current().record("tier", "try_idle");
            #[cfg(feature = "metrics")]
            metrics::counter!(
                crate::observability::POOL_ROUTING_TIER_WINS_TOTAL,
                "tier" => "try_idle",
            )
            .increment(1);
            return Job::execute(&arc, sql).await;
        }

        // §7.3 step 2: scan up to min(status().size, 8) checked-out jobs
        // and route through the least-busy unsaturated upgradeable one.
        // The Arc keeps the Job alive for the duration of this request
        // even though the deadpool slot belongs to whoever currently has
        // the Object<JobManager> checked out — the v0.2 dispatcher
        // multiplexes concurrent requests on a single connection.
        if let Some(arc) = self.pick_unsaturated() {
            #[cfg(feature = "tracing")]
            tracing::Span::current().record("tier", "least_busy_scan");
            #[cfg(feature = "metrics")]
            metrics::counter!(
                crate::observability::POOL_ROUTING_TIER_WINS_TOTAL,
                "tier" => "least_busy_scan",
            )
            .increment(1);
            return Job::execute(&arc, sql).await;
        }

        // §7.3 step 3: every candidate is at or above SATURATION_THRESHOLD
        // (or the registry is empty) — fall back to fair queueing (waits
        // up to `acquire_timeout`) so the caller blocks until something
        // frees up rather than piling onto an already-saturated dispatcher.
        #[cfg(feature = "tracing")]
        tracing::Span::current().record("tier", "fair_queue");
        #[cfg(feature = "metrics")]
        metrics::counter!(
            crate::observability::POOL_ROUTING_TIER_WINS_TOTAL,
            "tier" => "fair_queue",
        )
        .increment(1);
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
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, params),
            fields(
                sql = %sql,
                param_count = params.len(),
                tier = tracing::field::Empty,
                param_types = tracing::field::Empty,
                params = tracing::field::Empty,
            ),
        )
    )]
    pub async fn execute_with(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> crate::Result<crate::query::Rows> {
        use crate::Job;

        // Task 9 / PRO-587: honor the per-pool `ParameterLogging` policy by
        // populating the Empty-declared `param_types` / `params` span fields.
        // The fields MUST be declared on `instrument(...)` above as
        // `tracing::field::Empty`, otherwise `record(...)` silently no-ops.
        // The default `ParameterLogging::None` arm intentionally records
        // nothing — `param_count` (already in the span) is the only
        // param-related disclosure.
        #[cfg(feature = "tracing")]
        match self.parameter_logging {
            ParameterLogging::None => {}
            ParameterLogging::TypesAndCount => {
                let types: Vec<&'static str> = params
                    .iter()
                    .map(|v| match v {
                        serde_json::Value::String(_) => "String",
                        serde_json::Value::Number(_) => "Number",
                        serde_json::Value::Bool(_) => "Bool",
                        serde_json::Value::Null => "Null",
                        serde_json::Value::Array(_) => "Array",
                        serde_json::Value::Object(_) => "Object",
                    })
                    .collect();
                tracing::Span::current().record("param_types", tracing::field::debug(&types));
            }
            ParameterLogging::Full => {
                tracing::Span::current().record("params", tracing::field::debug(params));
            }
        }

        // Pool status gauges — same once-per-call pattern as `execute`.
        #[cfg(feature = "metrics")]
        emit_pool_status_gauges(&self.inner.status());

        // §7.3 step 1 (v0.4 / Task 22 / PRO-600): registry-backed fast path.
        // See `execute` for the full rationale.
        if let Some(arc) = self.registry.peek_idle() {
            #[cfg(feature = "tracing")]
            tracing::Span::current().record("tier", "try_idle");
            #[cfg(feature = "metrics")]
            metrics::counter!(
                crate::observability::POOL_ROUTING_TIER_WINS_TOTAL,
                "tier" => "try_idle",
            )
            .increment(1);
            return Job::execute_with(&arc, sql, params).await;
        }

        // §7.3 step 2: least-busy scan filtered by SATURATION_THRESHOLD
        // (see `execute` for the full rationale).
        if let Some(arc) = self.pick_unsaturated() {
            #[cfg(feature = "tracing")]
            tracing::Span::current().record("tier", "least_busy_scan");
            #[cfg(feature = "metrics")]
            metrics::counter!(
                crate::observability::POOL_ROUTING_TIER_WINS_TOTAL,
                "tier" => "least_busy_scan",
            )
            .increment(1);
            return Job::execute_with(&arc, sql, params).await;
        }

        // §7.3 step 3: fall back to fair queueing.
        #[cfg(feature = "tracing")]
        tracing::Span::current().record("tier", "fair_queue");
        #[cfg(feature = "metrics")]
        metrics::counter!(
            crate::observability::POOL_ROUTING_TIER_WINS_TOTAL,
            "tier" => "fair_queue",
        )
        .increment(1);
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
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(acquired_in_micros = tracing::field::Empty))
    )]
    pub async fn acquire(&self) -> crate::Result<crate::pool::reserved::Reserved> {
        #[cfg(any(feature = "tracing", feature = "metrics"))]
        let start = std::time::Instant::now();
        let obj = self.get_or_timeout().await?;
        // `Duration::as_micros()` returns u128 — saturate at u64::MAX
        // (~584 942 years) to satisfy clippy::cast_possible_truncation
        // without panicking on a hypothetically huge elapsed.
        #[cfg(any(feature = "tracing", feature = "metrics"))]
        let elapsed_micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
        #[cfg(feature = "tracing")]
        tracing::Span::current().record("acquired_in_micros", elapsed_micros);
        #[cfg(feature = "metrics")]
        {
            // `u64 as f64` is the recommended pattern for histogram::record;
            // metric resolution is microseconds and the saturated u64::MAX
            // upper bound is not representable in f64 mantissa precision, but
            // any realistic elapsed (< 2^53 µs ≈ 285 years) fits exactly.
            #[allow(clippy::cast_precision_loss)]
            let micros_f64 = elapsed_micros as f64;
            metrics::histogram!(crate::observability::POOL_ACQUIRE_LATENCY_MICROS)
                .record(micros_f64);
            metrics::counter!(crate::observability::POOL_RESERVED_ACQUIRED_TOTAL).increment(1);
        }
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

/// Emit the three pool-status gauges (`size`, `available`, `waiting`) from a
/// fresh deadpool [`PoolStatus`] snapshot. Called once per `Pool::execute*`
/// invocation; downstream samplers see one data point per call (sufficient
/// granularity for Prometheus-style scrape windows without overwhelming the
/// recorder hot path).
///
/// `usize -> f64` casts are flagged by `clippy::cast_precision_loss` because
/// values above 2^53 lose mantissa precision; for pool sizes that's
/// essentially impossible (we'd run out of file descriptors first), so the
/// allow is sound.
#[cfg(feature = "metrics")]
fn emit_pool_status_gauges(s: &PoolStatus) {
    #[allow(clippy::cast_precision_loss)]
    {
        metrics::gauge!(crate::observability::POOL_SIZE).set(s.size as f64);
        metrics::gauge!(crate::observability::POOL_AVAILABLE).set(s.available as f64);
        metrics::gauge!(crate::observability::POOL_WAITING).set(s.waiting as f64);
    }
}

/// Compute the reaper's wake-up cadence from a configured `idle_timeout`.
///
/// We sweep at `idle_timeout / 4` so an idle connection is reaped within
/// `idle_timeout..=idle_timeout * 1.25` of its last use — fast enough to
/// honor the contract without burning CPU on tight loops. Clamped to
/// `[1s, 60s]`: a sub-second sweep wastes wakeups and a sweep that runs
/// less than once a minute leaves stale connections holding sockets too
/// long even for very long timeouts.
///
/// Returns `None` for zero/negligible timeouts (caller should not spawn).
pub(crate) fn reaper_period(idle_timeout: Duration) -> Option<Duration> {
    if idle_timeout.is_zero() {
        return None;
    }
    let quarter = idle_timeout / 4;
    let clamped = quarter.clamp(Duration::from_secs(1), Duration::from_secs(60));
    Some(clamped)
}

/// Spawn the periodic idle-connection reaper.
///
/// The task holds a [`deadpool::managed::WeakPool`] so it never extends
/// `PoolInner`'s lifetime. On each tick it upgrades the weak ref, calls
/// `Pool::retain` with a `last_used() < idle_timeout` predicate, and lets
/// removed objects drop (which calls `JobManager::detach` → tears the dispatcher
/// down). If upgrade fails, the task exits (defensive: in steady state
/// [`ReaperGuard::Drop`] aborts it before the strong refs hit zero, but during
/// runtime shutdown the abort may race the upgrade and we want a clean exit).
pub(crate) fn spawn_idle_reaper(
    inner: &DeadPool<JobManager>,
    idle_timeout: Duration,
    period: Duration,
) -> tokio::task::JoinHandle<()> {
    let weak = inner.weak();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(period);
        // Skip the immediate-tick fired by `interval()`'s first poll — there
        // are no connections to reap right after `Pool::build` returns, and
        // it would race with `starting_size` eager opens.
        interval.tick().await;
        loop {
            interval.tick().await;
            let Some(pool) = weak.upgrade() else {
                // Last `Pool` clone dropped between ticks. Done.
                return;
            };
            // `retain` blocks the slots mutex briefly; the predicate is
            // pure-arithmetic so this stays well under any contention
            // threshold. Removed `ObjectInner`s drop here, which calls
            // `JobManager::detach` → `Job::Drop` → dispatcher shutdown.
            let result = pool.retain(|_, metrics| metrics.last_used() < idle_timeout);
            #[cfg(feature = "metrics")]
            if !result.removed.is_empty() {
                metrics::counter!(crate::observability::POOL_IDLE_REAPED_TOTAL)
                    .increment(result.removed.len() as u64);
            }
            #[cfg(not(feature = "metrics"))]
            let _ = result;
            // Drop the strong `Pool` clone immediately so we don't hold the
            // pool alive across `interval.tick().await`.
            drop(pool);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaper_period_quarters_short_timeouts_clamped_to_one_second() {
        // 2s / 4 = 500ms, clamped up to the 1s floor.
        assert_eq!(
            reaper_period(Duration::from_secs(2)),
            Some(Duration::from_secs(1))
        );
    }

    #[test]
    fn reaper_period_quarters_medium_timeouts_unclamped() {
        // 60s / 4 = 15s — sits inside [1s, 60s].
        assert_eq!(
            reaper_period(Duration::from_secs(60)),
            Some(Duration::from_secs(15))
        );
    }

    #[test]
    fn reaper_period_clamps_long_timeouts_to_one_minute() {
        // 1h / 4 = 15min — clamped down to the 60s ceiling.
        assert_eq!(
            reaper_period(Duration::from_secs(3600)),
            Some(Duration::from_secs(60))
        );
    }

    #[test]
    fn reaper_period_returns_none_for_zero() {
        // Zero idle_timeout would tight-loop; refuse to spawn.
        assert_eq!(reaper_period(Duration::ZERO), None);
    }
}
