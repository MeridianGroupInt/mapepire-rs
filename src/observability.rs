//! Metric name constants emitted by the `metrics` feature.
//!
//! With the `metrics` feature enabled, `mapepire` emits a documented set of
//! counters, gauges, and histograms via the [`metrics`](https://docs.rs/metrics)
//! facade. Downstream callers register a recorder
//! ([`metrics-exporter-prometheus`](https://docs.rs/metrics-exporter-prometheus),
//! [`metrics-util::DebuggingRecorder`](https://docs.rs/metrics-util) for tests, etc.)
//! and the metric values flow through.
//!
//! ## Stable contract
//!
//! Changing any constant in this file is a SemVer-breaking change for users
//! who scrape these metrics by name. **Add new names — don't rename.** If a
//! metric's semantics need to change, deprecate the old constant (`#[deprecated]`)
//! and add a new one.
//!
//! ## Cardinality
//!
//! Most metrics are 0-label or 1-label (`tier` on the routing counter). Per-pool
//! cardinality is the responsibility of the recorder (e.g., add a label at
//! recorder-registration time).

/// Counter — number of [`crate::Job::connect`]-equivalent calls completed
/// inside `JobManager::create` (per-pool incrementing).
///
/// **Cardinality:** 0 labels.
pub const POOL_CREATE_TOTAL: &str = "mapepire_pool_create_total";

/// Counter — number of `JobManager::recycle()` ping successes (the connection
/// was alive and reused).
///
/// **Cardinality:** 0 labels.
pub const POOL_RECYCLE_SUCCESS_TOTAL: &str = "mapepire_pool_recycle_success_total";

/// Counter — number of `JobManager::recycle()` ping failures. Deadpool
/// detaches the connection and the next caller triggers a fresh `create()`.
/// A non-zero rate here on a healthy daemon usually points at network
/// flakiness or the IBM i firewall's idle-kill behavior.
///
/// **Cardinality:** 0 labels.
pub const POOL_RECYCLE_FAIL_TOTAL: &str = "mapepire_pool_recycle_fail_total";

/// Histogram (microseconds) — [`crate::Pool::acquire`] checkout latency,
/// measured from method entry to the moment the [`crate::Reserved`] is
/// constructed.
///
/// **Cardinality:** 0 labels.
pub const POOL_ACQUIRE_LATENCY_MICROS: &str = "mapepire_pool_acquire_latency_micros";

/// Histogram (microseconds) — [`crate::Job::execute`] / [`crate::Job::execute_with`]
/// end-to-end latency, measured from method entry to result return.
///
/// **Cardinality:** 0 labels.
pub const JOB_EXECUTE_LATENCY_MICROS: &str = "mapepire_job_execute_latency_micros";

/// Gauge — current pool size (deadpool's [`Status::size`] passed through
/// [`crate::Pool::status`]).
///
/// **Cardinality:** 0 labels.
///
/// [`Status::size`]: https://docs.rs/deadpool/0.13/deadpool/managed/struct.Status.html#structfield.size
pub const POOL_SIZE: &str = "mapepire_pool_size";

/// Gauge — number of currently-idle (available) connections in the pool.
///
/// **Cardinality:** 0 labels.
pub const POOL_AVAILABLE: &str = "mapepire_pool_available";

/// Gauge — number of callers currently blocked in `pool.get()` waiting for
/// a connection.
///
/// **Cardinality:** 0 labels.
pub const POOL_WAITING: &str = "mapepire_pool_waiting";

/// Counter — routing-tier wins. Labelled `tier` ∈ {`try_idle`,
/// `least_busy_scan`, `fair_queue`} to distinguish §7.3 step 1 / 2 / 3 hits.
/// Useful for graphing routing distribution.
///
/// **Cardinality:** 1 label, 3 values. Bounded.
pub const POOL_ROUTING_TIER_WINS_TOTAL: &str = "mapepire_pool_routing_tier_wins_total";

/// Counter — number of [`crate::Pool::acquire`]-completed [`crate::Reserved`]
/// handles. Useful for transaction-rate observability.
///
/// **Cardinality:** 0 labels.
pub const POOL_RESERVED_ACQUIRED_TOTAL: &str = "mapepire_pool_reserved_acquired_total";

/// Counter — number of `Reserved` Drop firings that emitted a `ROLLBACK`.
/// Post-Task 18 (PRO-596) tightening, only fires when the connection is
/// in-tx (BEGIN observed without matching COMMIT).
///
/// **Cardinality:** 0 labels.
pub const POOL_RESERVED_ROLLBACK_TOTAL: &str = "mapepire_pool_reserved_rollback_total";
