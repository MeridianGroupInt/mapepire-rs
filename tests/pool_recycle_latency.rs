//! Latency-injection regression test for the v0.4 / Task 22 (PRO-600)
//! registry-backed fast path.
//!
//! v0.3 Task 24 documented a real-network fragility — `Pool::execute`'s
//! step 1 called `timeout_get(recycle: ZERO)` which was
//! `tokio::time::timeout(ZERO, ping)`, allowing only ~1 timer-tick of grace
//! before canceling the recycle ping. On real IBM i deployments where
//! ping RTT exceeded that, every step 1 attempt timed out → deadpool
//! detached the connection → step 3 fallback opened a fresh socket →
//! connection thrash.
//!
//! Task 22 replaced that path with `Registry::peek_idle()` which
//! dispatches directly via `Job::execute` without going through deadpool's
//! checkout, so no recycle ping fires on the fast path.
//!
//! This test pins that fix: with `mock.pause_responses(100ms)` held
//! across two `pool.execute()` calls on a single-slot pool, the second
//! call must reuse the warmed socket — `open_socket_count` stays at 1.
//! Under the v0.3 contract, the second call would have triggered a
//! timed-out recycle ping → fresh checkout → 2 sockets.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fast_path_does_not_thrash_under_slow_response() {
    use std::time::Duration;

    use common::spawn_mock_pool;

    // Single-slot pool: there's exactly one connection to either reuse or
    // thrash, so the assertion is unambiguous.
    let (pool, mock) = spawn_mock_pool(1).await;

    // Warm the pool: the first execute opens 1 socket via step 3
    // (fair-queue), registers the Job in the routing registry, and
    // returns. After the await, the Object is back in deadpool's slot.
    drop(
        Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
            .await
            .expect("warmup execute"),
    );

    let warmup_sockets = mock.open_socket_count();
    assert_eq!(
        warmup_sockets, 1,
        "warmup should have opened exactly 1 socket, got {warmup_sockets}"
    );

    // Inject 100ms of per-response latency. Under v0.3, this would have
    // delayed any recycle ping past `recycle: ZERO`'s timer-tick budget,
    // causing deadpool to detach the connection.
    let pause = mock.pause_responses(Duration::from_millis(100));

    // Second execute. Under v0.4 the registry-backed fast path skips
    // deadpool's checkout entirely — no recycle ping, no detach. The SQL
    // request rides the existing socket; the response is delayed by the
    // pause but eventually returns successfully.
    drop(
        Box::pin(pool.execute("SELECT 2 FROM SYSIBM.SYSDUMMY1"))
            .await
            .expect("post-pause execute reuses warmed socket"),
    );

    drop(pause);

    // The critical assertion: only one socket was ever opened. Under the
    // v0.3 contract this would be 2 (warmup + post-thrash fresh open).
    let final_sockets = mock.open_socket_count();
    assert_eq!(
        final_sockets, 1,
        "fast path must reuse the warmed socket under slow response; got {final_sockets} sockets (v0.3 fragility regressed?)"
    );

    // Belt-and-suspenders: confirm no `Ping` requests fired. The
    // registry-backed fast path bypasses `JobManager::recycle()` (which
    // is where the ping originates), so the wire trace should be exactly
    // two SQL requests and zero pings.
    let request_types = mock.observed_request_types();
    let ping_count = request_types.iter().filter(|t| *t == "ping").count();
    assert_eq!(
        ping_count, 0,
        "fast path must not fire a recycle ping; got {ping_count} pings in {request_types:?}"
    );
}
