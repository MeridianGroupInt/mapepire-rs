//! `DaemonServer.connect_address` splits the TCP hop from the TLS name.
//!
//! Mock listens on `127.0.0.1` with a SAN/CN of `ibmi.example`. Tunneled
//! clients set `host("ibmi.example").connect_address("127.0.0.1")`.
//! Using the loopback IP as `host` without a matching leaf pin fails closed.
//!
//! Gated `#[cfg(feature = "rustls-tls")]` because the mock harness is
//! rustls-only. Do **not** use `TlsConfig::Insecure` here.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use mapepire::{DaemonServer, Job, TlsConfig};
#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_connect_address_splits_tcp_from_sni() {
    let probe = common::UpgradeProbe::new();
    let (addr, cert_der) = common::spawn_mock_named_with_probe(
        "ibmi.example",
        common::MockBehavior::AcceptAndConnect,
        probe.clone(),
    );
    let server = DaemonServer::builder()
        .host("ibmi.example")
        .connect_address("127.0.0.1")
        .port(addr.port())
        .user("USER")
        .password(common::dummy_password())
        .tls(TlsConfig::Ca(cert_der))
        .build()
        .expect("DaemonServer builder fields all set");
    Job::connect(&server).await.expect("tunneled handshake");

    let path = probe.path().expect("mock saw upgrade");
    assert_eq!(path, "/db/");
    let host = probe.host().expect("Host header");
    assert!(
        host == "ibmi.example" || host.starts_with("ibmi.example:"),
        "Host must be the TLS name ibmi.example, got {host:?}"
    );
    assert!(
        !host.contains("127.0.0.1"),
        "Host must not be the TCP hop, got {host:?}"
    );
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_host_ip_against_dns_cert_fails_without_matching_pin_name_fallback() {
    // host=127.0.0.1, cert SAN=ibmi.example. TlsConfig::Verified cannot talk
    // to a mock self-signed (not in webpki roots). Use TlsConfig::Ca with a
    // *different* pin so pin-equality fails, then the name check against
    // host=127.0.0.1 vs SAN=ibmi.example also fails.
    let (addr, presented_der) =
        common::spawn_mock_named("ibmi.example", common::MockBehavior::AcceptAndConnect);
    let (wrong_pin, _) = common::mint_cn_only("ibmi.example");
    assert!(
        presented_der != wrong_pin,
        "wrong pin must not byte-equal the presented leaf"
    );

    let server = DaemonServer::builder()
        .host("127.0.0.1")
        .port(addr.port())
        .user("USER")
        .password(common::dummy_password())
        .tls(TlsConfig::Ca(wrong_pin))
        .build()
        .expect("DaemonServer builder fields all set");
    Job::connect(&server)
        .await
        .expect_err("host IP vs DNS SAN without matching pin must fail closed");
}
