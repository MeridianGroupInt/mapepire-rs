//! Daemon connection configuration.

use crate::password::Password;

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

/// Connection settings for a Mapepire daemon.
///
/// Construct via [`DaemonServer::builder`]. The struct is intentionally
/// **not** `Clone` because [`Password`] is not `Clone`. Wrap in
/// [`std::sync::Arc`] to share across multiple pools.
#[derive(Debug)]
pub struct DaemonServer {
    /// Hostname or IP of the IBM i system.
    pub host: String,
    /// TCP port; default `8076`.
    pub port: u16,
    /// IBM i user profile.
    pub user: String,
    /// IBM i user password.
    pub password: Password,
    /// TLS verification mode.
    pub tls: TlsConfig,
}

impl DaemonServer {
    /// Default Mapepire daemon TCP port.
    pub const DEFAULT_PORT: u16 = 8076;

    /// Begin building a [`DaemonServer`] with required fields collected
    /// fluently.
    #[must_use]
    pub fn builder() -> DaemonServerBuilder {
        DaemonServerBuilder::default()
    }
}

/// Fluent builder for [`DaemonServer`].
#[derive(Debug, Default)]
pub struct DaemonServerBuilder {
    host: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<Password>,
    tls: Option<TlsConfig>,
}

impl DaemonServerBuilder {
    /// Set the hostname or IP.
    #[must_use]
    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    /// Override the default port (8076).
    #[must_use]
    pub fn port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Set the IBM i user profile.
    #[must_use]
    pub fn user(mut self, user: impl Into<String>) -> Self {
        self.user = Some(user.into());
        self
    }

    /// Set the password. Takes ownership; the original `String` heap
    /// buffer moves into a zeroizing buffer on construction.
    #[must_use]
    pub fn password(mut self, password: String) -> Self {
        self.password = Some(Password::new(password));
        self
    }

    /// Override the default TLS configuration ([`TlsConfig::Verified`]).
    #[must_use]
    pub fn tls(mut self, tls: TlsConfig) -> Self {
        self.tls = Some(tls);
        self
    }

    /// Finalize the builder.
    ///
    /// # Errors
    ///
    /// Returns [`BuilderError`] if any required field (`host`, `user`,
    /// `password`) is missing.
    pub fn build(self) -> Result<DaemonServer, BuilderError> {
        Ok(DaemonServer {
            host: self.host.ok_or(BuilderError::MissingField("host"))?,
            port: self.port.unwrap_or(DaemonServer::DEFAULT_PORT),
            user: self.user.ok_or(BuilderError::MissingField("user"))?,
            password: self
                .password
                .ok_or(BuilderError::MissingField("password"))?,
            tls: self.tls.unwrap_or_default(),
        })
    }
}

/// Errors returned by [`DaemonServerBuilder::build`].
#[derive(Debug, thiserror::Error)]
pub enum BuilderError {
    /// A required field was not set before calling `build()`.
    #[error("missing required field: {0}")]
    MissingField(&'static str),
}

// NOTE: `From<DaemonServer> for Arc<DaemonServer>` is provided by the
// standard library's blanket `impl<T> From<T> for Arc<T>` (stable since
// Rust 1.21). An explicit impl would conflict (E0119). Callers can use
// `Arc::new(server)` or `Into::<Arc<DaemonServer>>::into(server)` directly.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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

    #[test]
    fn builder_defaults_port_and_tls() {
        let s = DaemonServer::builder()
            .host("ibmi.example.com")
            .user("DCURTIS")
            .password("hunter2".to_string())
            .build()
            .expect("build");

        assert_eq!(s.host, "ibmi.example.com");
        assert_eq!(s.port, DaemonServer::DEFAULT_PORT);
        assert_eq!(s.user, "DCURTIS");
        assert!(matches!(s.tls, TlsConfig::Verified));
    }

    #[test]
    fn builder_missing_host_is_error() {
        let err = DaemonServer::builder()
            .user("DCURTIS")
            .password("x".to_string())
            .build()
            .unwrap_err();
        assert!(matches!(err, BuilderError::MissingField("host")));
    }

    #[test]
    fn builder_missing_user_is_error() {
        let err = DaemonServer::builder()
            .host("h")
            .password("x".to_string())
            .build()
            .unwrap_err();
        assert!(matches!(err, BuilderError::MissingField("user")));
    }

    #[test]
    fn builder_missing_password_is_error() {
        let err = DaemonServer::builder()
            .host("h")
            .user("u")
            .build()
            .unwrap_err();
        assert!(matches!(err, BuilderError::MissingField("password")));
    }

    #[test]
    fn into_arc_works() {
        let s = DaemonServer::builder()
            .host("h")
            .user("u")
            .password("p".to_string())
            .build()
            .unwrap();
        let a: Arc<DaemonServer> = s.into();
        assert_eq!(a.host, "h");
    }

    #[test]
    fn builder_overrides_port_and_tls() {
        let s = DaemonServer::builder()
            .host("h")
            .user("u")
            .password("p".to_string())
            .port(9999)
            .tls(TlsConfig::Insecure)
            .build()
            .expect("build");
        assert_eq!(s.port, 9999);
        assert!(matches!(s.tls, TlsConfig::Insecure));
    }
}

/// Serializable counterpart to [`DaemonServer`] for loading config from files.
///
/// Available only with the `serde-config` feature. Convert into the runtime
/// type via [`DaemonServerSpec::try_into_server`].
#[cfg(feature = "serde-config")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde-config")))]
#[derive(Debug, serde::Deserialize)]
pub struct DaemonServerSpec {
    /// Hostname or IP of the IBM i system.
    pub host: String,
    /// TCP port; defaults to [`DaemonServer::DEFAULT_PORT`] when absent.
    #[serde(default)]
    pub port: Option<u16>,
    /// IBM i user profile.
    pub user: String,
    /// IBM i user password (plain text in config — handle the file accordingly).
    pub password: String,
    /// TLS mode. `"verified"`, `"insecure"`, or `{ "ca": "<base64-DER>" }`
    /// in the config file.
    #[serde(default)]
    pub tls: TlsConfigSpec,
}

/// TLS configuration as it appears in serialized config.
#[cfg(feature = "serde-config")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde-config")))]
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TlsConfigSpec {
    /// Verify against system roots.
    #[default]
    Verified,
    /// Pin a CA from the given DER bytes (base64-encoded in the config).
    Ca(String),
    /// Skip verification.
    Insecure,
}

#[cfg(feature = "serde-config")]
impl DaemonServerSpec {
    /// Convert into a runtime [`DaemonServer`].
    ///
    /// # Errors
    ///
    /// Returns a [`SpecError`] if the TLS CA bytes fail to decode from base64.
    pub fn try_into_server(self) -> Result<DaemonServer, SpecError> {
        use base64::Engine;
        let tls = match self.tls {
            TlsConfigSpec::Verified => TlsConfig::Verified,
            TlsConfigSpec::Insecure => TlsConfig::Insecure,
            TlsConfigSpec::Ca(b64) => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&b64)
                    .map_err(SpecError::InvalidCaBase64)?;
                TlsConfig::Ca(bytes)
            }
        };
        Ok(DaemonServer {
            host: self.host,
            port: self.port.unwrap_or(DaemonServer::DEFAULT_PORT),
            user: self.user,
            password: Password::new(self.password),
            tls,
        })
    }
}

/// Errors returned by [`DaemonServerSpec::try_into_server`].
#[cfg(feature = "serde-config")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde-config")))]
#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    /// The base64-encoded CA bytes failed to decode.
    #[error("invalid base64 in tls.ca: {0}")]
    InvalidCaBase64(#[source] base64::DecodeError),
}

#[cfg(all(test, feature = "serde-config"))]
mod spec_tests {
    use super::*;

    #[test]
    fn parses_minimal_toml() {
        let toml_str = r#"
            host = "ibmi.example.com"
            user = "DCURTIS"
            password = "hunter2"
        "#;
        let spec: DaemonServerSpec = toml::from_str(toml_str).expect("parse");
        let server = spec.try_into_server().expect("convert");
        assert_eq!(server.host, "ibmi.example.com");
        assert_eq!(server.port, DaemonServer::DEFAULT_PORT);
    }

    #[test]
    fn parses_with_explicit_port_and_insecure_tls() {
        let toml_str = r#"
            host = "h"
            port = 9000
            user = "u"
            password = "p"
            tls = "insecure"
        "#;
        let spec: DaemonServerSpec = toml::from_str(toml_str).expect("parse");
        let server = spec.try_into_server().expect("convert");
        assert_eq!(server.port, 9000);
        assert!(matches!(server.tls, TlsConfig::Insecure));
    }
}
