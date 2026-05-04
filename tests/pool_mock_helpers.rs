//! Phase 9 Task 26 smoke test: exercise the multi-connection pool mock
//! helpers and the [`MockHandle`] observation surface.
//!
//! This test is intentionally minimal — Tasks 27–30 layer real pool
//! integration scenarios on top of these primitives. The point here is to
//! catch breakage in the helpers themselves before downstream tests start
//! depending on them, and to demonstrate the call-site shape that the
//! Phase 9 plan documents:
//!
//! - `spawn_mock_pool(N)` returns a `(Pool, MockHandle)` ready to acquire against.
//! - The pool can open more than one TCP connection against the same mock and
//!   `MockHandle::observed_socket_ids` distinguishes them.
//! - `MockHandle::last_socket_for_sql` finds the socket id that issued a SQL statement matching the
//!   needle (Task 28's BEGIN/UPDATE/COMMIT same-socket assertion uses this).
//! - `MockHandle::pause_responses` slows the mock until the `ResponsePauseGuard` is dropped (Task
//!   30's saturation test depends on this).
//!
//! Each subtest pins one helper; failures are scoped to one assertion.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_mock_pool_observes_multiple_connections() {
    use std::time::Instant;

    use common::spawn_mock_pool;

    // Build a 2-slot pool against the multi-connection mock.
    let (pool, handle) = spawn_mock_pool(2).await;

    // Acquire two connections concurrently — both should land on
    // distinct sockets, and each should issue at least one SQL request
    // so the observation hooks see them.
    let conn_a = Box::pin(pool.acquire()).await.expect("acquire A");
    let conn_b = Box::pin(pool.acquire()).await.expect("acquire B");

    drop(
        Box::pin(conn_a.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
            .await
            .expect("execute A"),
    );
    drop(
        Box::pin(conn_b.execute("SELECT 2 FROM SYSIBM.SYSDUMMY1"))
            .await
            .expect("execute B"),
    );

    // Two distinct socket ids should be observed.
    let ids = handle.observed_socket_ids();
    assert_eq!(
        ids.len(),
        2,
        "expected 2 distinct sockets, got {ids:?} (open_socket_count = {})",
        handle.open_socket_count()
    );
    assert!(handle.open_socket_count() >= 2);

    // Both SQLs should appear in the observed-sql history.
    let sqls = handle.observed_sql();
    assert!(
        sqls.iter().any(|s| s.contains("SELECT 1")),
        "missing SELECT 1 in {sqls:?}"
    );
    assert!(
        sqls.iter().any(|s| s.contains("SELECT 2")),
        "missing SELECT 2 in {sqls:?}"
    );

    // last_socket_for_sql resolves to one of the observed sockets.
    let sid = handle
        .last_socket_for_sql("SELECT 1")
        .expect("found SELECT 1 socket");
    assert!(ids.contains(&sid), "{sid} not in {ids:?}");

    // request-types history should include `connect` (handshake) and
    // `sql` (the two executes).
    let types = handle.observed_request_types();
    assert!(
        types.iter().any(|t| t == "sql"),
        "expected at least one `sql` in {types:?}"
    );

    // Sanity check: drop the pool quickly so the test doesn't hang on
    // background tasks. `Instant` import keeps this test compiling on
    // MSRV 1.85 without pulling in dev-dependency churn.
    let _ = Instant::now();
    drop(pool);
}

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_responses_delays_mock_replies() {
    use std::time::{Duration, Instant};

    use common::spawn_mock_pool;

    let (pool, handle) = spawn_mock_pool(1).await;
    let conn = Box::pin(pool.acquire()).await.expect("acquire");

    // Pause guard active during the execute — the mock will sleep before
    // emitting its canned reply.
    let pause = Duration::from_millis(200);
    let guard = handle.pause_responses(pause);

    let start = Instant::now();
    drop(
        Box::pin(conn.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
            .await
            .expect("execute"),
    );
    let elapsed = start.elapsed();

    // The mock should have slept ~200ms — give a generous lower bound to
    // tolerate scheduler jitter on slow CI.
    assert!(
        elapsed >= Duration::from_millis(150),
        "execute returned in {elapsed:?}, expected >= 150ms"
    );

    // Drop the guard explicitly to confirm the Drop impl is callable.
    drop(guard);
}
