//! Phase 6 integration test: auth failure path.
//!
//! Spawns a mock with `MockBehavior::AuthFail(message)` so the mock responds
//! to the client's `Connect` with an `Error` response. Verifies that
//! `Job::connect` returns `Err(Error::Auth(...))` carrying the daemon's
//! error text.
//!
//! Per-item `#[cfg(feature = "rustls-tls")]` gating — the mock harness uses
//! rustls server primitives. Crate-level `//!` doc unconditional so
//! `missing_docs` is satisfied under native-tls.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_connect_with_bad_password_returns_auth_error() {
    use mapepire::{DaemonServer, Error, Job, TlsConfig};

    const AUTH_ERROR_MSG: &str = "invalid credentials for USER@MAPEPIRE";

    let (addr, cert_der) =
        common::spawn_mock(common::MockBehavior::AuthFail(AUTH_ERROR_MSG.to_string()));

    let server = DaemonServer::builder()
        .host(addr.ip().to_string())
        .port(addr.port())
        .user("USER")
        .password("WRONGPASS".to_string())
        .tls(TlsConfig::Ca(cert_der))
        .build()
        .expect("DaemonServer builder fields all set");

    let result = Job::connect(&server).await;

    match result {
        Err(Error::Auth(msg)) => {
            assert_eq!(
                msg, AUTH_ERROR_MSG,
                "Error::Auth message should be the daemon's error text"
            );
        }
        other => panic!("expected Err(Error::Auth(...)), got {other:?}"),
    }
}
