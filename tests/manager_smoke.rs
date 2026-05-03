//! Smoke: `deadpool::managed::Pool<JobManager>::get()` returns a usable
//! `Arc<Job>`; recycle on return-to-pool succeeds via ping.
//!
//! Verifies the round-trip plumbed by Tasks 6 + 7:
//! - First `pool.get()` triggers `JobManager::create()` (which calls `Job::connect`) and hands back
//!   an `Arc<Job>`.
//! - Dropping the wrapper returns the connection to the pool.
//! - Second `pool.get()` exercises `JobManager::recycle()` (which pings via the existing TCP
//!   session) and yields the same connection again.
//!
//! The mock harness is rustls-only test infrastructure, so the `mod common;`
//! declaration and the test fn are gated by `#[cfg(feature = "rustls-tls")]`
//! at item level (NOT crate-level `#![cfg]` — that would also exclude the
//! crate-level `//!` doc comment and trip `missing_docs = "deny"`; see the
//! note in `tests/common/mod.rs`).

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manager_create_and_recycle() {
    use common::{MockBehavior, spawn_mock_and_server};
    use deadpool::managed::Pool;
    use mapepire::pool::JobManager;

    let server_arc = spawn_mock_and_server(MockBehavior::AcceptAndConnect);
    let mgr = JobManager::new(server_arc);
    let pool: Pool<JobManager> = Pool::builder(mgr).max_size(2).build().expect("pool builds");

    // `Box::pin` on `pool.get()` to satisfy `clippy::large_futures` — the
    // pool's `get` future contains the manager's `create()` state machine
    // (which contains a full TLS handshake + first request/response cycle),
    // so on the stack it's ~30 KB. Boxing matches `Dispatcher::spawn`'s
    // treatment of the transport future in `src/transport/handshake.rs`.
    // Task 10 / PRO-440 will decide whether `Pool::execute` boxes internally
    // or pushes this responsibility onto callers.
    let obj = Box::pin(pool.get()).await.expect("get");
    assert_eq!(obj.in_flight(), 0);

    // Drop returns the connection to the pool.
    drop(obj);

    // Second get() exercises the recycle() path (which pings via the existing
    // connection — same TCP session as the first get(), which is why a
    // single-connection mock is sufficient here).
    let obj2 = Box::pin(pool.get()).await.expect("get-after-recycle");
    let _rtt = obj2.ping().await.expect("ping after recycle");

    // Pool stats: exactly one connection should exist (recycle reuses; it must
    // not have dropped + re-created). max_size=2 is enforced by deadpool itself,
    // so the strong shape (== 1) catches a regression where recycle accidentally
    // closes the connection.
    let status = pool.status();
    assert_eq!(
        status.size, 1,
        "recycle should not have spawned a second connection (got size {})",
        status.size
    );
}

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pool_execute_one_shot() {
    use common::{MockBehavior, spawn_mock_and_server};

    let server_arc = spawn_mock_and_server(MockBehavior::AcceptAndConnect);
    let pool = Box::pin(mapepire::Pool::builder(server_arc).max_size(2).build())
        .await
        .expect("pool builds");

    // Smoke: just verify Pool::execute makes it through the checkout +
    // dispatch path. The mock's `AcceptAndConnect` answers post-handshake
    // requests with `Pong { id }`, but `Job::execute` expects a
    // `QueryResult` — so the dispatcher will route the response by id and
    // the caller will surface a protocol mismatch error. The point of this
    // test is the pool plumbing (get → run → return on drop), not SQL
    // semantics. Full SQL coverage with a Pages-mock harness lands in
    // Task 27 / PRO-457; here we just verify the call doesn't panic.
    let _ = Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1")).await;
}
