//! Phase 4 Task 16 integration test: `idle_timeout` enforcement.
//!
//! Configures a pool with a 500ms idle timeout, opens a connection via
//! `pool.execute()`, then waits past the reap window and verifies the
//! connection was actually reaped (`pool.status().size == 0`).
//!
//! ## Reaper timing primer (Task 15 contract)
//!
//! Task 15 wires deadpool's runtime hooks so a periodic reaper drops
//! connections whose `Metrics::last_used()` exceeds `idle_timeout`. The
//! reap period is `idle_timeout / 4`, **clamped to a 1-second floor**.
//! With `idle_timeout = 500ms` the reap period is therefore `max(125ms,
//! 1s) = 1s`, and the minimum observable reap latency is ~1.0–1.25s after
//! the connection becomes idle.
//!
//! Total wallclock budget is ~2s of sleep + a short post-reap retry loop.
//!
//! ## Wallclock vs virtual time
//!
//! This test uses `tokio::time::sleep` (wallclock) deliberately. Pausing
//! virtual time would interact unpredictably with the reaper task's
//! `tokio::time::interval`, since the reaper runs on the same runtime.
//! Wallclock is the predictable choice even though it costs ~2s per run.
//!
//! ## Mock harness limitation
//!
//! `MockHandle::open_socket_count()` is monotonic — it counts TCP accepts
//! and never decrements on close (see `tests/common/mock_server.rs`).
//! That means we cannot assert "mock observed socket close after reap"
//! against the current mock; we rely instead on `pool.status().size`
//! dropping to 0 to prove the reaper actually evicted the slot. The
//! post-reap `execute()` then drives `open_socket_count` from 1 to 2,
//! which is independent evidence that the reaped slot was replaced by a
//! fresh TCP accept rather than reusing the previous connection.
//!
//! Per-item `#[cfg(feature = "rustls-tls")]` gating since the mock harness
//! is rustls-only test infrastructure.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_timeout_reaps_connections() {
    use std::time::Duration;

    use common::spawn_pool_mock_and_server;

    let (server_arc, mock) = spawn_pool_mock_and_server();

    let pool = Box::pin(
        mapepire::Pool::builder(server_arc)
            .max_size(2)
            .idle_timeout(Some(Duration::from_millis(500)))
            .build(),
    )
    .await
    .expect("pool builds");

    // Open exactly one connection by running a single execute.
    let _ = Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
        .await
        .expect("execute ok");

    // Sanity: pool now has 1 connection in its slot vec, and the mock saw
    // exactly one TCP accept.
    assert_eq!(
        pool.status().size,
        1,
        "expected 1 connection after first execute; status: {:?}",
        pool.status()
    );
    assert_eq!(
        mock.open_socket_count(),
        1,
        "expected 1 mock socket after first execute"
    );

    // Wait past the reap window: idle_timeout (500ms) + reap period
    // (clamped to 1s) + slack (500ms). The reaper should observe the
    // connection's last_used > 500ms ago and reap it.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Pool size dropped to 0 after reap.
    //
    // Allow a short polling window in case the reaper just missed this
    // tick — its period is 1s, so up to ~1s of additional wait is the
    // worst legitimate case under CI scheduler jitter.
    let mut grace_remaining = 50u32;
    while pool.status().size > 0 && grace_remaining > 0 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        grace_remaining -= 1;
    }
    assert_eq!(
        pool.status().size,
        0,
        "expected pool drained by idle reaper; status: {:?}",
        pool.status()
    );

    // Subsequent execute can still open a fresh connection — verifies the
    // pool isn't poisoned by the reap and that a NEW socket is opened
    // (open_socket_count goes 1 → 2).
    let _ = Box::pin(pool.execute("SELECT 2 FROM SYSIBM.SYSDUMMY1"))
        .await
        .expect("post-reap execute ok");

    assert_eq!(
        mock.open_socket_count(),
        2,
        "expected post-reap execute to open a fresh socket; \
         open_socket_count={}",
        mock.open_socket_count()
    );

    drop(pool);
}
