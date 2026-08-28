//! TLS connection helper.
//!
//! Returns a typed `TlsStream` ready for the WebSocket layer to wrap.
//! Backend selection is compile-time via the `rustls-tls` (default) /
//! `native-tls` feature flags.

#[cfg(feature = "rustls-tls")]
use std::sync::Arc;

use tokio::net::TcpStream;

use crate::config::{DaemonServer, TlsConfig};
use crate::error::{Error, TransportError};

/// Stream type returned by `connect`. The concrete type varies per TLS
/// backend; callers see only the trait bounds the WebSocket layer needs
/// (`AsyncRead` + `AsyncWrite` + `Unpin` + `Send`).
#[cfg(feature = "rustls-tls")]
pub(crate) type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

#[cfg(all(not(feature = "rustls-tls"), feature = "native-tls"))]
pub(crate) type TlsStream = tokio_native_tls::TlsStream<TcpStream>;

/// Install rustls' ring `CryptoProvider` if the process has none yet.
///
/// `ClientConfig::builder()` panics without a process-level provider. We
/// install ring ourselves so consumers of this crate do not have to.
/// `AlreadyInstalled` is ignored so an application that already chose a
/// provider (e.g. aws-lc-rs) keeps it.
#[cfg(feature = "rustls-tls")]
fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Establish a TCP connection then complete the TLS handshake to the
/// daemon. The returned stream is ready for HTTP/1.1 Upgrade.
///
/// TCP uses `connect_address` when set, otherwise `host`. TLS SNI /
/// native-tls hostname is always `host`.
pub(crate) async fn connect(server: &DaemonServer) -> crate::Result<TlsStream> {
    let (tcp_host, port) = server.tcp_target();
    let addr = format!("{tcp_host}:{port}");
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| Error::from(TransportError::Io(e)))?;
    tcp.set_nodelay(true).ok();
    tls_handshake(server, tcp).await
}

#[cfg(feature = "rustls-tls")]
async fn tls_handshake(server: &DaemonServer, tcp: TcpStream) -> crate::Result<TlsStream> {
    use rustls::{ClientConfig, RootCertStore};
    use rustls_pki_types::ServerName;
    use tokio_rustls::TlsConnector;

    ensure_crypto_provider();

    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let config = match &server.tls {
        TlsConfig::Verified => ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),

        TlsConfig::Ca(der) => {
            let cert = rustls_pki_types::CertificateDer::from(der.clone());
            roots
                .add(cert)
                .map_err(|e| Error::Internal(format!("invalid Ca cert: {e}")))?;
            let inner = rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
                .build()
                .map_err(|e| Error::Internal(format!("tls verifier: {e}")))?;
            ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(PinOrWebpki {
                    pin: der.clone(),
                    inner,
                }))
                .with_no_client_auth()
        }

        #[cfg(feature = "insecure-tls")]
        TlsConfig::Insecure => {
            tracing_warn_insecure_once();
            ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerify))
                .with_no_client_auth()
        }
    };

    let connector = TlsConnector::from(Arc::new(config));
    let dns = ServerName::try_from(server.host.clone())
        .map_err(|_| Error::Internal(format!("invalid hostname: {}", server.host)))?;
    connector
        .connect(dns, tcp)
        .await
        .map_err(|e| Error::from(TransportError::Io(e)))
}

/// rustls verifier for [`TlsConfig::Ca`].
///
/// If the presented leaf DER equals the pin, skip SAN/name checks — TLS
/// already proved possession of the matching private key. IBM i Mapepire
/// certs are often CN-only; rustls 0.23/webpki would otherwise reject
/// them even as a trust anchor. A non-matching leaf still goes through
/// rustls `WebPkiServerVerifier` (pin added as a root) **with** name
/// checks. [`TlsConfig::Verified`] does not use this type.
#[cfg(feature = "rustls-tls")]
#[derive(Debug)]
struct PinOrWebpki {
    pin: Vec<u8>,
    inner: Arc<rustls::client::WebPkiServerVerifier>,
}

#[cfg(feature = "rustls-tls")]
impl rustls::client::danger::ServerCertVerifier for PinOrWebpki {
    fn verify_server_cert(
        &self,
        end_entity: &rustls_pki_types::CertificateDer<'_>,
        intermediates: &[rustls_pki_types::CertificateDer<'_>],
        server_name: &rustls_pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.pin.as_slice() {
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls_pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(all(not(feature = "rustls-tls"), feature = "native-tls"))]
async fn tls_handshake(server: &DaemonServer, tcp: TcpStream) -> crate::Result<TlsStream> {
    let mut builder = native_tls::TlsConnector::builder();

    match &server.tls {
        TlsConfig::Verified => {}
        TlsConfig::Ca(der) => {
            let cert = native_tls::Certificate::from_der(der)
                .map_err(|e| Error::Internal(format!("invalid Ca cert: {e}")))?;
            builder.add_root_certificate(cert);
        }

        #[cfg(feature = "insecure-tls")]
        TlsConfig::Insecure => {
            tracing_warn_insecure_once();
            builder
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true);
        }
    }

    let connector = builder
        .build()
        .map_err(|e| Error::Internal(format!("native-tls builder: {e}")))?;
    let connector = tokio_native_tls::TlsConnector::from(connector);
    connector
        .connect(&server.host, tcp)
        .await
        .map_err(|e| Error::Internal(format!("native-tls handshake: {e}")))
}

#[cfg(all(feature = "insecure-tls", feature = "rustls-tls"))]
#[derive(Debug)]
struct NoVerify;

#[cfg(all(feature = "insecure-tls", feature = "rustls-tls"))]
impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &rustls_pki_types::CertificateDer<'_>,
        _: &[rustls_pki_types::CertificateDer<'_>],
        _: &rustls_pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls_pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls_pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls_pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

#[cfg(feature = "insecure-tls")]
fn tracing_warn_insecure_once() {
    use std::sync::Once;
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        // Print to stderr; tracing integration lands in v0.4 and will
        // replace this with a tracing::warn! call.
        eprintln!(
            "WARNING: TlsConfig::Insecure is in use — TLS certificate verification \
             is disabled. NEVER use this in production."
        );
    });
}

/// Open a TLS connection with verification **disabled**, capture the server's
/// leaf certificate, and return its DER-encoded bytes.
///
/// This is the canonical bootstrap helper for self-signed Mapepire daemons.
/// Pin the returned bytes via [`crate::config::TlsConfig::Ca`] for all
/// subsequent verified connections.
///
/// **Security warning:** The connection that returns the bytes is itself
/// unverified, so a man-in-the-middle attacker could substitute their own
/// certificate. Always verify the returned DER bytes out-of-band before
/// trusting them. **Never** skip that verification step in production.
/// Concretely: compute the SHA-256 fingerprint of the returned DER bytes
/// (e.g., `openssl x509 -in <der> -inform DER -fingerprint -sha256 -noout`)
/// and compare against the value the daemon admin reports out-of-band.
///
/// Fires the once-per-process insecure-TLS warning so the verification bypass
/// is visible in the process logs.
///
/// # Errors
///
/// - [`crate::error::Error::Transport`] for TCP / TLS failures.
/// - [`crate::error::Error::Internal`] if the server presents no certificate or an empty chain.
///
/// # Example
///
/// ```no_run
/// # async fn example() -> mapepire::Result<()> {
/// use mapepire::{DaemonServer, TlsConfig};
///
/// // Bootstrap: fetch the daemon's self-signed cert (UNVERIFIED).
/// let der = DaemonServer::fetch_certificate("ibmi.example", 8076).await?;
///
/// // Pin it for subsequent verified connections.
/// let server = DaemonServer::builder()
///     .host("ibmi.example")
///     .port(8076)
///     .user("USER")
///     .password("…".to_string())
///     .tls(TlsConfig::Ca(der))
///     .build()
///     .expect("all fields set");
/// # Ok(()) }
/// ```
#[cfg(all(feature = "insecure-tls", feature = "rustls-tls"))]
pub async fn fetch_certificate(host: &str, port: u16) -> crate::Result<Vec<u8>> {
    fetch_certificate_from(host, host, port).await
}

/// Open a TLS connection with verification **disabled**, capture the server's
/// leaf certificate, and return its DER-encoded bytes.
///
/// TCP connects to `connect_address:port`; SNI uses `server_name`. See
/// [`fetch_certificate`] for the security warning and the `host == TCP`
/// shorthand.
///
/// # Errors
///
/// - [`crate::error::Error::Transport`] for TCP / TLS failures.
/// - [`crate::error::Error::Internal`] if the server presents no certificate or an empty chain.
///
/// # Example
///
/// ```no_run
/// # async fn example() -> mapepire::Result<()> {
/// use mapepire::{DaemonServer, TlsConfig};
///
/// let der = DaemonServer::fetch_certificate_from("ibmi.example", "127.0.0.1", 8076).await?;
///
/// let server = DaemonServer::builder()
///     .host("ibmi.example")
///     .connect_address("127.0.0.1")
///     .port(8076)
///     .user("USER")
///     .password("…".to_string())
///     .tls(TlsConfig::Ca(der))
///     .build()
///     .expect("all fields set");
/// # Ok(()) }
/// ```
#[cfg(all(feature = "insecure-tls", feature = "rustls-tls"))]
pub async fn fetch_certificate_from(
    server_name: &str,
    connect_address: &str,
    port: u16,
) -> crate::Result<Vec<u8>> {
    use rustls::ClientConfig;
    use rustls_pki_types::ServerName;
    use tokio_rustls::TlsConnector;

    ensure_crypto_provider();

    let addr = format!("{connect_address}:{port}");
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| Error::from(TransportError::Io(e)))?;
    tcp.set_nodelay(true).ok();

    tracing_warn_insecure_once();

    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoVerify))
        .with_no_client_auth();

    let connector = TlsConnector::from(Arc::new(config));
    let dns = ServerName::try_from(server_name.to_string())
        .map_err(|_| Error::Internal(format!("invalid hostname: {server_name}")))?;
    let stream = connector
        .connect(dns, tcp)
        .await
        .map_err(|e| Error::from(TransportError::Io(e)))?;

    let (_io, session) = stream.get_ref();
    let chain = session
        .peer_certificates()
        .ok_or_else(|| Error::Internal("server did not present a certificate chain".into()))?;
    let leaf = chain
        .first()
        .ok_or_else(|| Error::Internal("server presented an empty certificate chain".into()))?;
    Ok(leaf.as_ref().to_vec())
}

/// Open a TLS connection with verification **disabled**, capture the server's
/// leaf certificate, and return its DER-encoded bytes.
///
/// This is the canonical bootstrap helper for self-signed Mapepire daemons.
/// Pin the returned bytes via [`crate::config::TlsConfig::Ca`] for all
/// subsequent verified connections.
///
/// **Security warning:** The connection that returns the bytes is itself
/// unverified, so a man-in-the-middle attacker could substitute their own
/// certificate. Always verify the returned DER bytes out-of-band before
/// trusting them. **Never** skip that verification step in production.
/// Concretely: compute the SHA-256 fingerprint of the returned DER bytes
/// (e.g., `openssl x509 -in <der> -inform DER -fingerprint -sha256 -noout`)
/// and compare against the value the daemon admin reports out-of-band.
///
/// Fires the once-per-process insecure-TLS warning so the verification bypass
/// is visible in the process logs.
///
/// # Errors
///
/// - [`crate::error::Error::Transport`] for TCP / TLS failures.
/// - [`crate::error::Error::Internal`] if the server presents no certificate or an empty chain.
///
/// # Example
///
/// ```no_run
/// # async fn example() -> mapepire::Result<()> {
/// use mapepire::{DaemonServer, TlsConfig};
///
/// // Bootstrap: fetch the daemon's self-signed cert (UNVERIFIED).
/// let der = DaemonServer::fetch_certificate("ibmi.example", 8076).await?;
///
/// // Pin it for subsequent verified connections.
/// let server = DaemonServer::builder()
///     .host("ibmi.example")
///     .port(8076)
///     .user("USER")
///     .password("…".to_string())
///     .tls(TlsConfig::Ca(der))
///     .build()
///     .expect("all fields set");
/// # Ok(()) }
/// ```
#[cfg(all(
    feature = "insecure-tls",
    not(feature = "rustls-tls"),
    feature = "native-tls"
))]
pub async fn fetch_certificate(host: &str, port: u16) -> crate::Result<Vec<u8>> {
    fetch_certificate_from(host, host, port).await
}

/// Open a TLS connection with verification **disabled**, capture the server's
/// leaf certificate, and return its DER-encoded bytes.
///
/// TCP connects to `connect_address:port`; the native-tls hostname is
/// `server_name`. See [`fetch_certificate`] for the security warning.
///
/// # Errors
///
/// - [`crate::error::Error::Transport`] for TCP / TLS failures.
/// - [`crate::error::Error::Internal`] if the server presents no certificate or an empty chain.
///
/// # Example
///
/// ```no_run
/// # async fn example() -> mapepire::Result<()> {
/// use mapepire::{DaemonServer, TlsConfig};
///
/// let der = DaemonServer::fetch_certificate_from("ibmi.example", "127.0.0.1", 8076).await?;
///
/// let server = DaemonServer::builder()
///     .host("ibmi.example")
///     .connect_address("127.0.0.1")
///     .port(8076)
///     .user("USER")
///     .password("…".to_string())
///     .tls(TlsConfig::Ca(der))
///     .build()
///     .expect("all fields set");
/// # Ok(()) }
/// ```
#[cfg(all(
    feature = "insecure-tls",
    not(feature = "rustls-tls"),
    feature = "native-tls"
))]
pub async fn fetch_certificate_from(
    server_name: &str,
    connect_address: &str,
    port: u16,
) -> crate::Result<Vec<u8>> {
    let addr = format!("{connect_address}:{port}");
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| Error::from(TransportError::Io(e)))?;
    tcp.set_nodelay(true).ok();

    tracing_warn_insecure_once();

    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|e| Error::Internal(format!("native-tls builder: {e}")))?;
    let connector = tokio_native_tls::TlsConnector::from(connector);
    let stream = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| Error::Internal(format!("native-tls handshake: {e}")))?;

    let cert = stream
        .get_ref()
        .peer_certificate()
        .map_err(|e| Error::from(TransportError::Io(std::io::Error::other(e))))?
        .ok_or_else(|| Error::Internal("server did not present a certificate".into()))?;
    cert.to_der()
        .map_err(|e| Error::from(TransportError::Io(std::io::Error::other(e))))
}

#[cfg(all(test, feature = "rustls-tls"))]
mod tests {
    use std::sync::Arc;

    use rustls::ServerConfig;
    use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
    use tokio_rustls::TlsAcceptor;

    use super::*;
    use crate::config::{DaemonServer, TlsConfig};
    use crate::error::Error;

    fn dummy_pass() -> String {
        String::from("test") + "-only"
    }

    fn accept_one() -> (u16, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local_addr").port();
        let handle = std::thread::spawn(move || {
            let _ = listener.accept();
        });
        (port, handle)
    }

    #[test]
    fn test_rustls_crypto_provider_is_installed() {
        ensure_crypto_provider();
        let _ = rustls::ClientConfig::builder();
        assert!(
            rustls::crypto::CryptoProvider::get_default().is_some(),
            "rustls has no process-level CryptoProvider; enable the ring feature"
        );
    }

    #[tokio::test]
    async fn test_connect_rejects_invalid_hostname() {
        let (port, accept) = accept_one();
        let server = DaemonServer::builder()
            .host("[")
            .connect_address("127.0.0.1")
            .port(port)
            .user("u")
            .password(dummy_pass())
            .tls(TlsConfig::Verified)
            .build()
            .expect("builder");
        let err = connect(&server).await.expect_err("invalid hostname");
        assert!(matches!(err, Error::Internal(_)));
        if let Error::Internal(msg) = err {
            assert!(
                msg.contains("invalid hostname"),
                "unexpected Internal: {msg}"
            );
        }
        let _ = accept.join();
    }

    #[tokio::test]
    async fn test_connect_rejects_invalid_ca_der() {
        let (port, accept) = accept_one();
        let server = DaemonServer::builder()
            .host("localhost")
            .connect_address("127.0.0.1")
            .port(port)
            .user("u")
            .password(dummy_pass())
            .tls(TlsConfig::Ca(vec![0xff, 0x00, 0x01]))
            .build()
            .expect("builder");
        let err = connect(&server).await.expect_err("invalid Ca");
        assert!(matches!(err, Error::Internal(_)));
        if let Error::Internal(msg) = err {
            assert!(
                msg.contains("invalid Ca cert"),
                "unexpected Internal: {msg}"
            );
        }
        let _ = accept.join();
    }

    #[cfg(feature = "insecure-tls")]
    #[tokio::test]
    async fn test_fetch_certificate_from_rejects_invalid_hostname() {
        let (port, accept) = accept_one();
        let err = fetch_certificate_from("[", "127.0.0.1", port)
            .await
            .expect_err("invalid hostname");
        assert!(matches!(err, Error::Internal(_)));
        if let Error::Internal(msg) = err {
            assert!(
                msg.contains("invalid hostname"),
                "unexpected Internal: {msg}"
            );
        }
        let _ = accept.join();
    }

    #[tokio::test]
    async fn test_ca_pin_tls12_handshake_covers_tls12_signature() {
        ensure_crypto_provider();
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["localhost".into()]).expect("rcgen");
        let cert_der: Vec<u8> = cert.der().as_ref().to_vec();
        let key_der = signing_key.serialize_der();
        let server_config =
            ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
                .with_no_client_auth()
                .with_single_cert(
                    vec![CertificateDer::from(cert_der.clone())],
                    PrivatePkcs8KeyDer::from(key_der).into(),
                )
                .expect("tls12 server");
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("local_addr").port();
        let server_task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            acceptor.accept(tcp).await
        });
        let server = DaemonServer::builder()
            .host("localhost")
            .connect_address("127.0.0.1")
            .port(port)
            .user("u")
            .password(dummy_pass())
            .tls(TlsConfig::Ca(cert_der))
            .build()
            .expect("builder");
        let client = connect(&server).await;
        assert!(client.is_ok(), "tls12 Ca pin handshake failed");
        let _ = server_task.await;
    }
}
