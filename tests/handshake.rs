//! Phase 6 integration test: `Job::connect` happy path against the mock.
//!
//! Verifies that connecting to a mock with [`common::MockBehavior::AcceptAndConnect`]
//! returns a [`mapepire::Job`] whose `version()` and `initial_job()` accessors expose
//! the canned `Connected` response payload.
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
        job.version(),
        "0.0.0-mock",
        "mock version mismatch: {}",
        job.version()
    );
    assert_eq!(
        job.initial_job(),
        "MOCK/QUSER/000001",
        "mock initial_job mismatch: {}",
        job.initial_job()
    );
    assert_eq!(job.in_flight(), 0, "fresh Job has 0 in-flight requests");
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_connect_upgrade_request_target_is_db_slash() {
    use mapepire::{DaemonServer, Job, TlsConfig};

    let probe = common::UpgradeProbe::new();
    let (addr, cert_der) =
        common::spawn_mock_with_probe(common::MockBehavior::AcceptAndConnect, probe.clone());
    let server = DaemonServer::builder()
        .host("127.0.0.1")
        .port(addr.port())
        .user("USER")
        .password(common::dummy_password())
        .tls(TlsConfig::Ca(cert_der))
        .build()
        .expect("DaemonServer builder fields all set");
    Job::connect(&server).await.unwrap();
    let got = probe.path().expect("mock saw upgrade");
    assert_eq!(got, "/db/");
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_connect_sends_http_basic() {
    use base64::Engine;
    use mapepire::{DaemonServer, Job, TlsConfig};

    let probe = common::UpgradeProbe::new();
    let (addr, cert_der) =
        common::spawn_mock_with_probe(common::MockBehavior::AcceptAndConnect, probe.clone());
    let server = DaemonServer::builder()
        .host("127.0.0.1")
        .port(addr.port())
        .user("USER")
        .password(common::dummy_password())
        .tls(TlsConfig::Ca(cert_der))
        .build()
        .expect("DaemonServer builder fields all set");
    Job::connect(&server).await.unwrap();
    let header = probe.authorization().expect("Authorization");
    assert!(
        header.starts_with("Basic "),
        "Authorization should be Basic, got {header:?}"
    );
    let b64 = header.trim_start_matches("Basic ");
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .expect("Basic payload is standard base64");
    assert_eq!(raw, b"USER:test-only");
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_connect_json_auth_without_error_text() {
    use mapepire::{DaemonServer, Error, Job, TlsConfig};

    let (addr, cert_der) = common::spawn_mock(common::MockBehavior::AuthFail(String::new()));
    let server = DaemonServer::builder()
        .host(addr.ip().to_string())
        .port(addr.port())
        .user("USER")
        .password(common::dummy_password())
        .tls(TlsConfig::Ca(cert_der))
        .build()
        .expect("DaemonServer builder fields all set");
    match Job::connect(&server).await {
        Err(Error::Auth(msg)) => {
            assert_eq!(msg, "connect rejected");
        }
        other => panic!("expected Auth connect rejected, got {other:?}"),
    }
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_connect_unexpected_response_is_protocol_error() {
    use mapepire::{DaemonServer, Error, Job, TlsConfig};

    let (addr, cert_der) = common::spawn_mock(common::MockBehavior::PongOnConnect);
    let server = DaemonServer::builder()
        .host(addr.ip().to_string())
        .port(addr.port())
        .user("USER")
        .password(common::dummy_password())
        .tls(TlsConfig::Ca(cert_der))
        .build()
        .expect("DaemonServer builder fields all set");
    match Job::connect(&server).await {
        Err(Error::Protocol(_)) => {}
        other => panic!("expected Protocol error, got {other:?}"),
    }
}
