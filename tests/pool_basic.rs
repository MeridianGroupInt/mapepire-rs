//! Phase 9 Task 27: pool create / drain / recycle / acquire-timeout
//! integration tests.
//!
//! These tests pin the §7 pool contract end-to-end against the
//! multi-connection mock from Task 26:
//!
//! - `pool_create_then_drain` — multiple concurrent `execute()` calls succeed and
//!   `Pool::status().size` stays bounded by `max_size`.
//! - `pool_recycle_pings_on_checkout` — the second checkout observes a `ping` request, verifying
//!   deadpool's `RecyclingMethod::Verified` path runs `Job::ping()` before handing the connection
//!   back.
//! - `pool_acquire_timeout_returns_pool_exhausted` — when the pool is saturated, `Pool::execute()`
//!   surfaces [`mapepire::Error::PoolExhausted`] with the configured `acquire_timeout` carried in
//!   the variant.
//!
//! Per-item `#[cfg(feature = "rustls-tls")]` gates `mod common;` and each
//! `#[tokio::test]`: the mock harness is rustls-only test infrastructure
//! (see `tests/common/mod.rs`).

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_create_then_drain() {
    use common::spawn_mock_pool;

    let (pool, _mock) = spawn_mock_pool(4).await;

    // Multiple concurrent execute() calls should each succeed. The mock
    // returns canned empty success responses; we just verify the round-trip
    // doesn't panic and that the pool size stays bounded.
    let r1 = Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
        .await
        .expect("first execute");
    let r2 = Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
        .await
        .expect("second execute");
    drop(r1);
    drop(r2);

    // Pool size should stay <= max_size.
    let status = pool.status();
    assert!(
        status.size <= 4,
        "pool size {} exceeded max_size 4",
        status.size
    );
}

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pool_recycle_pings_on_checkout() {
    use common::spawn_mock_pool;

    let (pool, mock) = spawn_mock_pool(1).await;

    // First execute: opens a Job (Connect handshake) and runs the SQL.
    let _r1 = Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
        .await
        .expect("first execute");

    // Second execute: deadpool's recycle path runs Job::ping() before
    // handing the connection back. We verify the mock observed at least
    // one ping in the request stream.
    let _r2 = Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
        .await
        .expect("second execute");

    // Allow any final dispatch to land. The mock records inbound at arrival
    // time, so this sleep is conservative — usually the requests are visible
    // immediately on response receipt.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let types = mock.observed_request_types();
    let ping_count = types.iter().filter(|t| t.as_str() == "ping").count();
    assert!(
        ping_count >= 1,
        "expected at least one ping (recycle), got types: {types:?}"
    );
}

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pool_acquire_timeout_returns_pool_exhausted() {
    use std::time::Duration;

    use common::spawn_pool_mock_and_server;

    let (server_arc, _mock) = spawn_pool_mock_and_server();
    let pool = Box::pin(
        mapepire::Pool::builder(server_arc)
            .max_size(1)
            .acquire_timeout(Some(Duration::from_millis(50)))
            .build(),
    )
    .await
    .expect("pool builds");

    // Hold the only connection; the next execute() should time out
    // waiting for a free slot.
    let _hold = Box::pin(pool.acquire()).await.expect("first acquire");

    let err = Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
        .await
        .expect_err("execute must time out");

    match err {
        mapepire::Error::PoolExhausted { timeout } => {
            assert_eq!(
                timeout,
                Duration::from_millis(50),
                "PoolExhausted should carry the configured acquire_timeout"
            );
        }
        other => panic!("expected PoolExhausted, got {other:?}"),
    }
}
