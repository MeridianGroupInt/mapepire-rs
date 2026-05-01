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

use mapepire::{DaemonServer, Job, TlsConfig};
pub use mock_server::{MockBehavior, spawn_mock};

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
