//! Shared test infrastructure for mapepire integration tests.
//!
//! Each integration test binary pulls this in with `mod common;`.
//!
//! # Cargo convention
//!
//! `tests/common/mod.rs` is a *module file*, not a Cargo test binary.
//! Cargo only auto-discovers top-level `tests/*.rs` files as test binaries;
//! a `tests/common/mod.rs` is silently ignored by the test harness and only
//! compiled when another test binary does `mod common;`.
//!
//! # Feature gate requirement
// Test binaries pulling `mod common;` should gate the `mod common;`
// declaration AND any items using common's exports with
// `#[cfg(feature = "rustls-tls")]` — the mock harness is rustls-only
// test infrastructure. (Use per-item cfg, NOT crate-level
// `#![cfg]`, since that would also exclude the crate-level `//!` doc
// and trigger `missing_docs`.)

pub mod mock_server;

use std::sync::{Arc, Mutex};

use mapepire::protocol::{QueryResult, Request};
use mapepire::{DaemonServer, Job, TlsConfig};
pub use mock_server::{MockBehavior, RequestRecorder, spawn_mock};
use tokio::sync::oneshot;

/// Spawn a mock with [`MockBehavior::AcceptAndConnect`], build a
/// [`DaemonServer`] pointing at the bound address (with
/// [`TlsConfig::Ca`] pinning the mock's self-signed cert), call
/// [`Job::connect`], and return the connected [`Job`].
///
/// This is the convenience entry-point for the common case: most Phase 6
/// integration tests want a fully-connected [`Job`] backed by a mock that
/// speaks the happy-path protocol.
///
/// Uses `TlsConfig::Ca` so this works without the `insecure-tls` feature,
/// mirroring the production pattern of calling `fetch_certificate` then
/// pinning the returned DER bytes.
///
/// # Note on dead-code lint
///
/// Each test binary compiles `common` independently. Test binaries that call
/// [`spawn_mock`] directly (e.g. `auth_failure.rs`) don't use this helper, so
/// the lint fires for those compilation units. The allow suppresses that noise.
#[allow(dead_code)]
pub async fn spawn_mock_and_connect() -> Job {
    let (addr, cert_der) = spawn_mock(MockBehavior::AcceptAndConnect);
    let server = DaemonServer::builder()
        .host("127.0.0.1")
        .port(addr.port())
        .user("TESTUSER")
        .password("testpass".to_string())
        .tls(TlsConfig::Ca(cert_der))    // pin the mock's self-signed cert
        .build()
        .expect("build DaemonServer");
    Job::connect(&server)
        .await
        .expect("Job::connect to mock server")
}

/// Spawn a mock and build the [`Arc<DaemonServer>`] pointing at it. Used by
/// pool tests that construct [`mapepire::pool::JobManager`] directly (where
/// the manager — not the test — owns the `Job::connect` call).
///
/// The mock handles exactly ONE TCP connection; tests that need multiple
/// connections must spawn additional mocks. This is sufficient for the
/// Task 8 smoke test because `pool.get() → drop → pool.get()` reuses the
/// same connection (the second `get()` ping-recycles via the existing TCP
/// session rather than opening a new one).
#[allow(dead_code)]
pub fn spawn_mock_and_server(behavior: MockBehavior) -> Arc<DaemonServer> {
    let (addr, cert_der) = spawn_mock(behavior);
    Arc::new(
        DaemonServer::builder()
            .host("127.0.0.1")
            .port(addr.port())
            .user("TESTUSER")
            .password("testpass".to_string())
            .tls(TlsConfig::Ca(cert_der))
            .build()
            .expect("build DaemonServer"),
    )
}

/// Spawn a mock with [`MockBehavior::SwallowFirstPing`] holding the given
/// `signal_tx`, build a [`DaemonServer`] with [`TlsConfig::Ca`] pinning, call
/// [`Job::connect`], and return the connected [`Job`].
///
/// The mock fires `signal_tx` exactly once — on the first [`Request::Ping`]
/// it observes — and *does not* send a `Pong` for that ping. The deterministic
/// cancellation test in `tests/cancellation.rs` uses this edge to drop the
/// in-flight ping future via a `biased` `tokio::select!`, exercising the
/// dispatcher's cancellation-safety path. Subsequent pings receive normal
/// [`Response::Pong`] replies, so the test can verify the connection still
/// works after the cancelled ping.
///
/// [`Request::Ping`]: mapepire::protocol::Request::Ping
/// [`Response::Pong`]: mapepire::protocol::Response::Pong
#[allow(dead_code)]
pub async fn spawn_mock_swallow_first_ping_and_connect(signal_tx: oneshot::Sender<()>) -> Job {
    let behavior = MockBehavior::SwallowFirstPing {
        signal_tx: Arc::new(Mutex::new(Some(signal_tx))),
    };
    let (addr, cert_der) = spawn_mock(behavior);
    let server = DaemonServer::builder()
        .host("127.0.0.1")
        .port(addr.port())
        .user("TESTUSER")
        .password("testpass".to_string())
        .tls(TlsConfig::Ca(cert_der))
        .build()
        .expect("build DaemonServer");
    Job::connect(&server)
        .await
        .expect("Job::connect to mock server")
}

/// Spawn a mock with the given `behavior`, build a [`DaemonServer`] pointing
/// at the bound address (with [`TlsConfig::Ca`]`(cert_der)` so the
/// verified-TLS path is exercised), call [`Job::connect`], and return the
/// connected [`Job`].
///
/// The generalized version of [`spawn_mock_and_connect`] — accepts any
/// [`MockBehavior`], not just `AcceptAndConnect`. Future Phase 6 tests that
/// need `Pages`, `ReturnError`, `HalfOpen`, etc. use this directly.
#[allow(dead_code)]
pub async fn connect_to_mock(behavior: MockBehavior) -> Job {
    let (addr, cert_der) = spawn_mock(behavior);
    let server = DaemonServer::builder()
        .host(addr.ip().to_string())
        .port(addr.port())
        .user("USER")
        .password("PASS".to_string())
        .tls(TlsConfig::Ca(cert_der))
        .build()
        .expect("test builder fields all set");
    Job::connect(&server)
        .await
        .expect("Job::connect against mock")
}

/// Spawn a mock with [`MockBehavior::Pages`] wired to a fresh
/// [`RequestRecorder`], connect a [`Job`], and hand both back to the
/// caller.
///
/// Used by Cleanup D's drop-rows tests: the test consumes the `Job`
/// (executing SQL, dropping `Rows`) and then asserts on the recorded
/// requests via the returned `Arc<Mutex<Vec<Request>>>`.
///
/// Note: `spawn_close` (the close-firing path) is fire-and-forget — the
/// `SqlClose` request may not have transited by the time the test thread
/// runs assertions. Use a bounded polling pattern (see `wait_for` in
/// `tests/drop_rows.rs`) rather than a fixed sleep, which is fragile
/// under CI scheduler jitter.
#[allow(dead_code)]
pub async fn connect_to_mock_with_recorder(pages: Vec<QueryResult>) -> (Job, RequestRecorder) {
    let recorder: RequestRecorder = Arc::new(Mutex::new(Vec::<Request>::new()));
    let behavior = MockBehavior::Pages {
        pages,
        recorder: Some(Arc::clone(&recorder)),
    };
    let job = connect_to_mock(behavior).await;
    (job, recorder)
}

/// Spawn a mock with [`MockBehavior::Pages`] wired to a fresh
/// [`RequestRecorder`] and return the [`DaemonServer`] (so a [`mapepire::Pool`]
/// can be built against it) plus the recorder.
///
/// Pool variant of [`connect_to_mock_with_recorder`]: the caller constructs
/// the pool itself (via `Pool::builder`) and asserts on the requests that
/// transit the *single* TCP connection the mock accepts. Used by Task 14's
/// reserved-transaction integration test (`tests/pool_transactions.rs`).
///
/// The mock is single-connection per spawn — the architectural guarantee that
/// every statement issued through a [`mapepire::Reserved`] lands on one
/// socket is implicit in the mock shape (one [`mapepire::Pool::acquire`]
/// triggers one [`crate::pool::JobManager::create`] which opens one TCP
/// session). The recorder lets tests verify the dispatcher actually emitted
/// the expected request sequence on that one socket.
#[allow(dead_code)]
pub fn spawn_mock_pool_with_recorder(
    pages: Vec<QueryResult>,
) -> (Arc<DaemonServer>, RequestRecorder) {
    let recorder: RequestRecorder = Arc::new(Mutex::new(Vec::<Request>::new()));
    let behavior = MockBehavior::Pages {
        pages,
        recorder: Some(Arc::clone(&recorder)),
    };
    let server = spawn_mock_and_server(behavior);
    (server, recorder)
}
