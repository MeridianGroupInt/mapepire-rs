//! Metrics-feature integration test: verify Task 12's emission sites
//! actually fire by snapshotting via [`metrics_util::debugging::DebuggingRecorder`].
//!
//! Per-item `#[cfg(all(feature = "rustls-tls", feature = "metrics"))]`
//! gating since the mock harness is rustls-only and the emission sites
//! only fire when `metrics` is enabled. Crate-level `#![cfg]` is
//! intentionally avoided — it would strip the `//!` doc and trip the
//! crate-wide `missing_docs = "deny"` lint.
//!
//! # Single-test design
//!
//! [`metrics::set_global_recorder`] installs a process-wide recorder
//! and rejects subsequent installs (returns `Err`). Two `#[tokio::test]`
//! functions in the same test binary compile into one process, so
//! splitting the assertions across multiple tests would race on the
//! one-shot global slot — the second installer would silently observe
//! a stale recorder from the first test, with no isolation guarantee.
//!
//! [`metrics::with_local_recorder`] / [`metrics::set_default_local_recorder`]
//! are *thread-local*. The Pool/Job code runs on a `multi_thread`
//! tokio runtime, so emissions from worker threads would not see a
//! main-thread-only local recorder. That route is unsuitable here.
//!
//! Therefore: one consolidated test exercises BOTH `Pool::execute`
//! (driving routing-tier counter, status gauges, and job-execute
//! latency histogram) AND `Pool::acquire` (driving acquire-latency
//! histogram and reserved-acquired counter). `JobManager::create`
//! fires `POOL_CREATE_TOTAL` as a side effect of pool warm-up. One
//! global recorder install, one snapshot, multi-assertion.

#[cfg(all(feature = "rustls-tls", feature = "metrics"))]
mod common;

#[cfg(all(feature = "rustls-tls", feature = "metrics"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_emits_documented_metric_names() {
    use common::spawn_mock_pool;
    use mapepire::observability;
    use metrics_util::debugging::DebuggingRecorder;

    // Arrange — install a fresh `DebuggingRecorder` as the global
    // recorder. `set_global_recorder` returns `Err` if one is already
    // installed; in this test binary there's exactly one test and one
    // recorder, so install must succeed on a clean run.
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();
    metrics::set_global_recorder(recorder).expect("install DebuggingRecorder as global");

    let (pool, _mock) = spawn_mock_pool(2).await;

    // Act — drive both dispatch entry points so every Task 12 emission
    // site has a chance to fire:
    //   * `Pool::execute`  → routing-tier counter, status gauges, and (via `Job::execute`
    //     underneath) the job-execute latency histogram.
    //   * `Pool::acquire`  → acquire-latency histogram and the reserved-acquired counter.
    // `JobManager::create` fires `POOL_CREATE_TOTAL` whenever the
    // deadpool slot opens its underlying `Job`, which happens on the
    // first call that needs a connection (lazy pool warm-up).
    //
    // `Box::pin` matches the `clippy::large_futures` precedent in
    // `tests/common::spawn_mock_pool` — the dispatch futures contain
    // the deadpool create/recycle state machines plus a TLS handshake.
    let _ = Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
        .await
        .expect("execute ok");
    let conn = Box::pin(pool.acquire()).await.expect("acquire ok");
    drop(conn);

    // Snapshot — `metrics-util` 0.20's `Snapshotter::snapshot()` returns
    // a `Snapshot`; `into_vec()` flattens to
    // `Vec<(CompositeKey, Option<Unit>, Option<SharedString>, DebugValue)>`.
    // We only need metric *names* for the existence assertions, so we
    // pull `name()` off `CompositeKey::key()` and ignore the rest.
    let data = snapshotter.snapshot().into_vec();
    let names: Vec<String> = data
        .iter()
        .map(|(ck, _unit, _desc, _value)| ck.key().name().to_string())
        .collect();

    // Assert — every Task 12 documented emission site must appear in
    // the snapshot. Each `assert!` carries the captured `names` in its
    // failure message so a regression points directly at which site
    // stopped firing.

    // `JobManager::create` fired during pool warm-up.
    assert!(
        names.iter().any(|n| n == observability::POOL_CREATE_TOTAL),
        "expected `{}` in snapshot; saw: {names:?}",
        observability::POOL_CREATE_TOTAL,
    );

    // `Pool::execute` pre-dispatch status snapshot.
    assert!(
        names.iter().any(|n| n == observability::POOL_SIZE),
        "expected `{}` gauge in snapshot; saw: {names:?}",
        observability::POOL_SIZE,
    );
    assert!(
        names.iter().any(|n| n == observability::POOL_AVAILABLE),
        "expected `{}` gauge in snapshot; saw: {names:?}",
        observability::POOL_AVAILABLE,
    );
    assert!(
        names.iter().any(|n| n == observability::POOL_WAITING),
        "expected `{}` gauge in snapshot; saw: {names:?}",
        observability::POOL_WAITING,
    );

    // `Pool::execute` routing-tier resolution.
    assert!(
        names
            .iter()
            .any(|n| n == observability::POOL_ROUTING_TIER_WINS_TOTAL),
        "expected `{}` in snapshot; saw: {names:?}",
        observability::POOL_ROUTING_TIER_WINS_TOTAL,
    );

    // `Job::execute` end-to-end latency histogram (fires from any
    // dispatch path that ultimately calls `Job::execute`, including
    // `Pool::execute`'s routing tail).
    assert!(
        names
            .iter()
            .any(|n| n == observability::JOB_EXECUTE_LATENCY_MICROS),
        "expected `{}` in snapshot; saw: {names:?}",
        observability::JOB_EXECUTE_LATENCY_MICROS,
    );

    // `Pool::acquire` checkout latency + reserved-acquired counter.
    assert!(
        names
            .iter()
            .any(|n| n == observability::POOL_ACQUIRE_LATENCY_MICROS),
        "expected `{}` in snapshot; saw: {names:?}",
        observability::POOL_ACQUIRE_LATENCY_MICROS,
    );
    assert!(
        names
            .iter()
            .any(|n| n == observability::POOL_RESERVED_ACQUIRED_TOTAL),
        "expected `{}` in snapshot; saw: {names:?}",
        observability::POOL_RESERVED_ACQUIRED_TOTAL,
    );
}
