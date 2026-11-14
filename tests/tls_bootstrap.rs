//! Phase 7 integration test: TLS bootstrap workflow (cert-fetch portion).
//!
//! Exercises `DaemonServer::fetch_certificate(host, port)` end-to-end against a
//! self-signed mock cert. Verifies that the fetched DER bytes byte-equal the
//! cert the mock minted — proving the bootstrap path actually retrieves the
//! server's leaf certificate.
//!
//! The full bootstrap workflow continues with `TlsConfig::Ca(fetched_der)` +
//! `Job::connect`. That verified-connection step is exercised implicitly by
//! every Phase 6 test (handshake, sql, prepared, paging, etc.) which uses
//! `TlsConfig::Ca(spawn_mock_returned_cert)` via `common::connect_to_mock`.
//! Direct fetch-then-connect-on-same-mock requires multi-connection mock
//! support; deferred to v0.3 if needed.
//!
//! Gated `#[cfg(all(feature = "insecure-tls", any(feature = "rustls-ring", feature =
//! "rustls-aws-lc-rs")))]` because `fetch_certificate` requires `insecure-tls`, and `mod common`
//! pulls in rustls types that require a rustls backend feature. Both must be enabled.

#[cfg(all(
    feature = "insecure-tls",
    any(feature = "rustls-ring", feature = "rustls-aws-lc-rs")
))]
mod common;

#[cfg(all(
    feature = "insecure-tls",
    any(feature = "rustls-ring", feature = "rustls-aws-lc-rs")
))]
use pretty_assertions::assert_eq;

#[cfg(all(
    feature = "insecure-tls",
    any(feature = "rustls-ring", feature = "rustls-aws-lc-rs")
))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_fetch_certificate_returns_mock_cert_bytes() {
    use mapepire::DaemonServer;

    let (addr, mock_cert_der) = common::spawn_mock(common::MockBehavior::AcceptAndConnect);

    let host = addr.ip().to_string();
    let port = addr.port();
    let fetched_der = DaemonServer::fetch_certificate(&host, port)
        .await
        .expect("fetch_certificate against mock");

    assert_eq!(
        fetched_der, mock_cert_der,
        "fetch_certificate should return the mock's leaf cert DER bytes byte-for-byte"
    );
}

#[cfg(all(
    feature = "insecure-tls",
    any(feature = "rustls-ring", feature = "rustls-aws-lc-rs")
))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_fetch_certificate_from_splits_sni_from_tcp() {
    use mapepire::DaemonServer;

    let (addr, mock_cert_der) =
        common::spawn_mock_named("ibmi.example", common::MockBehavior::AcceptAndConnect);

    let fetched_der =
        DaemonServer::fetch_certificate_from("ibmi.example", "127.0.0.1", addr.port())
            .await
            .expect("fetch_certificate_from against tunneled mock");

    assert_eq!(
        fetched_der, mock_cert_der,
        "fetch_certificate_from should return the mock's leaf cert DER bytes byte-for-byte"
    );
}
