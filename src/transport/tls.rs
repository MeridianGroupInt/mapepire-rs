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
#[allow(dead_code)]
pub(crate) type TlsStream = tokio_rustls::client::TlsStream<TcpStream>;

#[cfg(all(not(feature = "rustls-tls"), feature = "native-tls"))]
#[allow(dead_code)]
pub(crate) type TlsStream = tokio_native_tls::TlsStream<TcpStream>;

/// Establish a TCP connection then complete the TLS handshake to the
/// daemon. The returned stream is ready for HTTP/1.1 Upgrade.
#[allow(dead_code)]
pub(crate) async fn connect(server: &DaemonServer) -> Result<TlsStream, Error> {
    let addr = format!("{}:{}", server.host, server.port);
    let tcp = TcpStream::connect(&addr)
        .await
        .map_err(|e| Error::from(TransportError::Io(e)))?;
    tcp.set_nodelay(true).ok();
    tls_handshake(server, tcp).await
}

#[cfg(feature = "rustls-tls")]
#[allow(dead_code)]
async fn tls_handshake(server: &DaemonServer, tcp: TcpStream) -> Result<TlsStream, Error> {
    use rustls::{ClientConfig, RootCertStore};
    use rustls_pki_types::ServerName;
    use tokio_rustls::TlsConnector;

    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    if let TlsConfig::Ca(der) = &server.tls {
        let cert = rustls_pki_types::CertificateDer::from(der.clone());
        roots
            .add(cert)
            .map_err(|e| Error::Internal(format!("invalid Ca cert: {e}")))?;
    }

    let config = match &server.tls {
        TlsConfig::Verified | TlsConfig::Ca(_) => ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),

        #[cfg(feature = "insecure-tls")]
        TlsConfig::Insecure => {
            tracing_warn_insecure_once();
            ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerify))
                .with_no_client_auth()
        }

        #[cfg(not(feature = "insecure-tls"))]
        TlsConfig::Insecure => {
            return Err(Error::Internal(
                "TlsConfig::Insecure requires the `insecure-tls` Cargo feature".into(),
            ));
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

#[cfg(all(not(feature = "rustls-tls"), feature = "native-tls"))]
#[allow(dead_code)]
async fn tls_handshake(server: &DaemonServer, tcp: TcpStream) -> Result<TlsStream, Error> {
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

        #[cfg(not(feature = "insecure-tls"))]
        TlsConfig::Insecure => {
            return Err(Error::Internal(
                "TlsConfig::Insecure requires the `insecure-tls` Cargo feature".into(),
            ));
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
