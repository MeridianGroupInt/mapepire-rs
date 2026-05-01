//! Smoke test for the mock TLS+WebSocket server harness (Task 21 / PRO-417).
//!
//! Verifies end-to-end: [`common::spawn_mock_and_connect`] produces a [`mapepire::Job`] whose
//! `version` and `initial_job` fields are populated. This proves the full
//! TCP → TLS → WebSocket → Connect handshake works against the mock before
//! Phase 6 integration tests build on it (Tasks 22–30).
//!
//! The `mod common;` declaration and the test fn are gated by
//! `#[cfg(feature = "rustls-tls")]` because the mock harness uses rustls
//! server primitives. Under native-tls the file compiles to an empty test
//! binary (the crate-level doc above is unconditional and satisfies
//! `missing_docs`).

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn smoke_mock_connect_populates_job_fields() {
    let job = common::spawn_mock_and_connect().await;
    assert!(
        !job.version.is_empty(),
        "expected non-empty version, got empty string"
    );
    assert!(
        !job.initial_job.is_empty(),
        "expected non-empty initial_job, got empty string"
    );
    // The mock reports known sentinel values — pin them so this test catches
    // any accidental change to the mock's Connected response.
    assert_eq!(
        job.version, "0.0.0-mock",
        "mock version mismatch: {}",
        job.version
    );
    assert_eq!(
        job.initial_job, "MOCK/QUSER/000001",
        "mock initial_job mismatch: {}",
        job.initial_job
    );
}
