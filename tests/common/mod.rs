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
/// A small grace sleep (`tokio::time::sleep(Duration::from_millis(50))`)
/// is typically required at the assertion site, since `spawn_close` is
/// fire-and-forget and the `SqlClose` may not have transited the wire by
/// the time the test thread reaches the assertion.
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
