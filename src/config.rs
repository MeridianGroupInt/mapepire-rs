//! Daemon connection configuration. `DaemonServer` and `DaemonServerBuilder`
//! are filled in Task 5.

/// TLS verification mode for the connection to the Mapepire daemon.
///
/// Mapepire is **always** TLS — there is no plaintext path. This enum only
/// chooses how the certificate is validated.
///
/// The variants exist at the type level in v0.1. Their runtime semantics
/// land with the transport layer in v0.2 — the active TLS backend is
/// selected at compile time via the `rustls-tls` (default) and
/// `native-tls` Cargo features.
#[derive(Debug, Clone, Default)]
pub enum TlsConfig {
    /// Verify the server certificate against system / `webpki` roots (default).
    ///
    /// In v0.2 this requires the `rustls-tls` or `native-tls` feature; v0.1
    /// only declares the type.
    #[default]
    Verified,

    /// Pin a specific CA certificate (DER-encoded bytes).
    ///
    /// In v0.2, use this with the bytes returned by
    /// `DaemonServer::fetch_certificate` to bootstrap trust on a self-signed
    /// daemon. v0.1 only declares the variant.
    Ca(Vec<u8>),

    /// Skip server-cert verification entirely. Available only when the crate
    /// is built with the `insecure-tls` feature (the runtime gate lands with
    /// the transport layer in v0.2).
    ///
    /// **Never** use this in production.
    Insecure,
}

// --- placeholders below: filled in Task 5 ---

/// Placeholder. Replaced in Task 5.
#[derive(Debug)]
pub struct DaemonServer;

/// Placeholder. Replaced in Task 5.
#[derive(Debug)]
pub struct DaemonServerBuilder;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_verified() {
        assert!(matches!(TlsConfig::default(), TlsConfig::Verified));
    }

    #[test]
    fn ca_holds_bytes() {
        let bytes = vec![0xAA, 0xBB, 0xCC];
        let cfg = TlsConfig::Ca(bytes.clone());
        match cfg {
            TlsConfig::Ca(b) => assert_eq!(b, bytes),
            _ => panic!("expected Ca variant"),
        }
    }
}
