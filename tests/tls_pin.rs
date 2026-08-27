//! rustls `TlsConfig::Ca` leaf pin: CN-only (no SAN) certificates.
//!
//! IBM i Mapepire certs are often CN-only. rustls 0.23/webpki rejects them
//! even when the leaf is pinned as a trust anchor. Byte-equal pin skips
//! name checks; a non-matching pin still uses `WebPkiServerVerifier`.
//! `TlsConfig::Verified` must not skip name checks.
//!
//! Gated `#[cfg(feature = "rustls-tls")]` because the mock harness is
//! rustls-only. Do **not** use `TlsConfig::Insecure` here.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use mapepire::{DaemonServer, Job, TlsConfig};
#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_ne;

/// id-ce-subjectAltName (2.5.29.17) as a DER OID encoding.
#[cfg(feature = "rustls-tls")]
const SAN_OID_DER: &[u8] = &[0x06, 0x03, 0x55, 0x1d, 0x11];

#[cfg(feature = "rustls-tls")]
fn der_has_san(der: &[u8]) -> bool {
    der.windows(SAN_OID_DER.len()).any(|w| w == SAN_OID_DER)
}

#[cfg(feature = "rustls-tls")]
fn server(port: u16, tls: TlsConfig) -> DaemonServer {
    DaemonServer::builder()
        .host("127.0.0.1")
        .port(port)
        .user("USER")
        .password("s3cret".to_string())
        .tls(tls)
        .build()
        .expect("DaemonServer builder fields all set")
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_ca_pin_accepts_cn_only_leaf() {
    let (addr, cert_der) =
        common::spawn_mock_cn_only("127.0.0.1", common::MockBehavior::AcceptAndConnect);
    assert!(
        !der_has_san(&cert_der),
        "fixture must be CN-only (no SAN); generate_simple_self_signed hides this bug"
    );

    let server = server(addr.port(), TlsConfig::Ca(cert_der));
    Job::connect(&server)
        .await
        .expect("TlsConfig::Ca pin must accept a CN-only leaf");
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_ca_pin_rejects_mismatched_leaf() {
    let (presented_der, presented_key) = common::mint_cn_only("127.0.0.1");
    let (wrong_pin, _) = common::mint_cn_only("127.0.0.1");
    assert_ne!(
        presented_der, wrong_pin,
        "two mint_cn_only calls must produce distinct leaves"
    );
    assert!(!der_has_san(&presented_der), "fixture must be CN-only");

    let addr = common::spawn_mock_with_cert(
        common::MockBehavior::AcceptAndConnect,
        &presented_der,
        presented_key,
    );
    let server = server(addr.port(), TlsConfig::Ca(wrong_pin));
    Job::connect(&server)
        .await
        .expect_err("mismatched Ca pin must fail closed");
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_verified_rejects_cn_only_leaf() {
    let (addr, cert_der) =
        common::spawn_mock_cn_only("127.0.0.1", common::MockBehavior::AcceptAndConnect);
    assert!(!der_has_san(&cert_der), "fixture must be CN-only (no SAN)");

    let server = server(addr.port(), TlsConfig::Verified);
    Job::connect(&server)
        .await
        .expect_err("TlsConfig::Verified must not accept a CN-only self-signed leaf");
}
