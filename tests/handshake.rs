//! Phase 6 integration test: `Job::connect` happy path against the mock.
//!
//! Verifies that connecting to a mock with [`common::MockBehavior::AcceptAndConnect`]
//! returns a [`mapepire::Job`] whose `version` and `initial_job` fields are populated
//! from the canned `Connected` response.
//!
//! The `mod common;` declaration and the test fn are gated by
//! `#[cfg(feature = "rustls-tls")]` because the mock harness uses rustls
//! server primitives. Under native-tls the file compiles to an empty test
//! binary (the crate-level doc above is unconditional and satisfies
//! `missing_docs`).

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_connect_returns_version_and_job() {
    let job = common::spawn_mock_and_connect().await;

    // Pin the canned values so a future change to the mock surface gets caught.
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
