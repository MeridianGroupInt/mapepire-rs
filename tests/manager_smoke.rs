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

    // Task 23 / PRO-453 added a routing-registry parameter to
    // `JobManager::new`, and the registry type itself is `pub(crate)` —
    // not reachable from integration tests. Rather than broaden visibility,
    // exercise the same create+recycle round-trip through the public
    // `Pool::builder` API. `Pool::acquire` (Task 13) drives the underlying
    // `deadpool::managed::Pool::get`, which is exactly what the previous
    // form did with a hand-constructed `JobManager`.
    let server_arc = spawn_mock_and_server(MockBehavior::AcceptAndConnect);
    let pool = Box::pin(mapepire::Pool::builder(server_arc).max_size(2).build())
        .await
        .expect("pool builds");

    // First acquire exercises `JobManager::create()` (which calls
    // `Job::connect` and registers the new `Arc<Job>` with the routing
    // registry — Task 23). `Box::pin` matches the `clippy::large_futures`
    // precedent: the acquire future contains the manager's `create()` state
    // machine (TLS handshake + first request/response cycle).
    let conn = Box::pin(pool.acquire()).await.expect("acquire");
    // `Reserved::new` (Task 13) marks the Job with the `u32::MAX` routing-skip
    // sentinel so the §7.3 scan never picks an exclusively-held connection.
    // Confirming the sentinel is set verifies `pool.acquire()` returned a
    // properly-initialized Reserved on top of a freshly-created Job.
    assert_eq!(conn.in_flight(), u32::MAX);
    // Drop returns the connection to the pool (and clears the Reserved
    // sentinel so `recycle()` can ping on the next acquire).
    drop(conn);

    // Second acquire exercises `JobManager::recycle()` (which pings via the
    // existing TCP session — same connection as the first acquire, which is
    // why a single-connection mock is sufficient here).
    let conn2 = Box::pin(pool.acquire())
        .await
        .expect("acquire-after-recycle");
    let _rtt = conn2.ping().await.expect("ping after recycle");

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

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reserved_derefs_to_job() {
    use common::{MockBehavior, spawn_mock_and_server};

    let server_arc = spawn_mock_and_server(MockBehavior::AcceptAndConnect);
    let pool = Box::pin(mapepire::Pool::builder(server_arc).max_size(2).build())
        .await
        .expect("pool builds");

    let conn = Box::pin(pool.acquire()).await.expect("acquire");
    // Deref to &Job — exercises the Deref impl plus the v0.2 inherent methods.
    let _v = conn.version();
    let _ = conn.ping().await.expect("ping via Reserved");
    drop(conn);
    // After drop, the Job's in_flight reset to 0 (sentinel cleared).
    // The pool can pick up the connection on the next acquire/execute.
}
