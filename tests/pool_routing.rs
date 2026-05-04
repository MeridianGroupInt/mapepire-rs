//! Routing tests for the §7.3 least-busy-job scan.
//!
//! Per-item `#[cfg(feature = "rustls-tls")]` gating since the mock harness
//! is rustls-only test infrastructure.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_job_preferred_over_busy() {
    use common::spawn_mock_pool;

    // Build a 2-slot pool. We make Job A "busy" by holding a `Reserved`
    // on it — `Reserved` sets the underlying Job's `in_flight` to the
    // `u32::MAX` routing-skip sentinel, so the §7.3 routing scan must
    // skip that Job. The follow-up `pool.execute()` is forced to land on
    // Job B's socket (a different connection).
    //
    // Why not use `pause_responses` to keep Job A busy? Because the §7.3
    // step 1 try-idle path calls `timeout_get` with `recycle: ZERO`,
    // which causes a paused recycle ping to time out → deadpool
    // detaches. Under pause, the warmed Jobs get destroyed and the
    // routing scan sees an empty registry — defeating the test's intent.
    // Using `Reserved` exercises the routing logic without fighting the
    // recycle-ping path.
    let (pool, mock) = spawn_mock_pool(2).await;

    // Pre-warm 2 sockets via concurrent acquires. After the inner block,
    // both sockets are recycled and the pool has 2 idle slots, with
    // both Jobs registered in the routing registry.
    {
        let r1 = Box::pin(pool.acquire()).await.expect("pre-warm reserve A");
        let r2 = Box::pin(pool.acquire()).await.expect("pre-warm reserve B");
        drop(
            Box::pin(r1.execute("SELECT 0 FROM SYSIBM.SYSDUMMY1"))
                .await
                .expect("warmup A"),
        );
        drop(
            Box::pin(r2.execute("SELECT 0 FROM SYSIBM.SYSDUMMY1"))
                .await
                .expect("warmup B"),
        );
    }

    let warmup_sockets = mock.open_socket_count();
    assert!(
        warmup_sockets >= 2,
        "warmup should have opened 2 sockets, got {warmup_sockets}"
    );

    // Pin Job A by holding a `Reserved` on it. Issue a marker SQL so we
    // can identify Job A's socket id later. While `busy_reserved` is
    // alive, Job A's `in_flight` is u32::MAX and the routing scan must
    // skip it.
    let busy_reserved = Box::pin(pool.acquire()).await.expect("acquire busy");
    drop(
        Box::pin(busy_reserved.execute("SELECT BUSY MARKER FROM SYSIBM.SYSDUMMY1"))
            .await
            .expect("busy marker"),
    );
    let busy_socket = mock
        .last_socket_for_sql("BUSY MARKER")
        .expect("BUSY MARKER should appear in mock history");

    // While Job A is pinned, the next `pool.execute()` must route to the
    // OTHER pooled Job — landing on Job B's socket.
    drop(
        Box::pin(pool.execute("SELECT IDLE PREFERRED FROM SYSIBM.SYSDUMMY1"))
            .await
            .expect("idle execute"),
    );
    let idle_socket = mock
        .last_socket_for_sql("IDLE PREFERRED")
        .expect("IDLE PREFERRED should appear in mock history");

    // The critical assertion: the idle execute did NOT pile onto the
    // busy (Reserved-pinned) Job's socket.
    assert_ne!(
        idle_socket, busy_socket,
        "idle Job preferred: pool.execute() must avoid the Reserved-pinned socket {busy_socket}, got {idle_socket}"
    );

    let socket_count = mock.open_socket_count();
    assert!(
        socket_count >= 2,
        "expected at least 2 sockets serviced (idle preferred over busy), got {socket_count}"
    );

    // Release the Reserved so the pool can drain cleanly at end of test.
    drop(busy_reserved);
}

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saturation_falls_back_to_fair_queue() {
    use std::time::Duration;

    use common::spawn_mock_pool;

    // max_size=2 — both Jobs need to be saturated (>= SATURATION_THRESHOLD = 32).
    // We launch 65 concurrent execute() calls. With the pause held, all
    // requests stack up in_flight on the available Jobs. The 66th call
    // should fall back to fair queueing (wait on get_or_timeout) rather
    // than pile on a saturated Job.
    let (pool, mock) = spawn_mock_pool(2).await;

    let pause_dur = Duration::from_millis(400);
    let pause = mock.pause_responses(pause_dur);

    let mut handles = Vec::with_capacity(65);
    for _ in 0..65 {
        let p = pool.clone();
        handles.push(tokio::spawn(async move {
            let _ = Box::pin(p.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1")).await;
        }));
    }

    // Brief pause to let the 65 requests bind to the 2 connections and stack
    // in_flight up. The mock's pause holds responses, so each request is
    // mid-flight on the dispatcher's pending HashMap.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The 66th call. With saturation logic correctly engaged, it cannot
    // route through least_busy (every candidate is at or above 32) and
    // falls back to get_or_timeout. With the pause still held, that wait
    // doesn't return until the pause releases at least one connection.
    let start = std::time::Instant::now();
    let r = Box::pin(pool.execute("SELECT 2 FROM SYSIBM.SYSDUMMY1")).await;
    let elapsed = start.elapsed();

    // The 66th call must have waited for the pause to release at least one
    // slot — typically ~pause_dur minus some scheduling slack. We accept any
    // elapsed >= 80% of pause_dur as evidence the saturation fallback fired.
    let min_wait = pause_dur.mul_f32(0.8);
    assert!(
        elapsed >= min_wait,
        "expected saturation fallback to wait >= {min_wait:?}, got {elapsed:?}; result: {r:?}"
    );

    drop(pause);
    for h in handles {
        let _ = h.await;
    }
}
