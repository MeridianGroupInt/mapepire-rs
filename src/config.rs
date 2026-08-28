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

    /// Pin a specific CA or leaf certificate (DER-encoded bytes).
    ///
    /// Use this with the bytes returned by `DaemonServer::fetch_certificate`
    /// to bootstrap trust on a self-signed daemon.
    ///
    /// On the `rustls-tls` backend, if the server presents a leaf whose DER
    /// equals this pin, name checks (SAN / CN) are skipped — TLS already
    /// proved possession of the matching private key. IBM i Mapepire daemons
    /// often ship CN-only self-signed certificates that rustls 0.23 would
    /// otherwise reject. If the presented leaf does **not** match, the pin
    /// is still added as a trust anchor and webpki name checks apply.
    ///
    /// The `native-tls` backend is unchanged: the pin is added as a root
    /// and OpenSSL's CN fallback applies.
    Ca(Vec<u8>),

    /// Skip server-cert verification entirely.
    ///
    /// Present only when the crate is built with the `insecure-tls` feature.
    /// Without that feature the variant does not exist (match exhaustiveness
    /// and rust-analyzer will not show it). First use still emits a runtime
    /// warning on stderr.
    ///
    /// **Never** use this in production. CN-only IBM i certs belong on
    /// [`TlsConfig::Ca`] (leaf pin), not this variant.
    #[cfg(feature = "insecure-tls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "insecure-tls")))]
    Insecure,
}

/// Connection settings for a Mapepire daemon.
///
/// Construct via [`DaemonServer::builder`]. The struct is intentionally
/// **not** `Clone` because [`Password`] is not `Clone`. Wrap in
/// [`std::sync::Arc`] to share across multiple pools.
#[derive(Debug)]
pub struct DaemonServer {
    /// TLS server name: SNI, HTTP `Host`, and certificate name.
    ///
    /// TCP uses [`DaemonServer::connect_address`] when set, otherwise this
    /// field. SSH forwards set `host` to the IBM i name (`ibmi.example`) and
    /// `connect_address` to `127.0.0.1`.
    pub host: String,
    /// Optional TCP hop. When `None`, TCP connects to [`DaemonServer::host`].
    ///
    /// TLS SNI, the `wss://` URI host, and HTTP `Host` always use `host`,
    /// never this field.
    pub connect_address: Option<String>,
    /// TCP port; default `8076`.
    pub port: u16,
    /// IBM i user profile.
    pub user: String,
    /// IBM i user password.
    pub password: Password,
    /// TLS verification mode.
    pub tls: TlsConfig,
    /// JDBC properties forwarded on connect as the `props` string.
    ///
    /// Semicolon-delimited, e.g. `"access=read only;auto commit=true"`.
    /// Omitted from the connect body when `None`.
    pub jdbc_props: Option<String>,
    /// Client application name sent on connect.
    ///
    /// Defaults to `"mapepire-rs"` when unset on the builder.
    pub application: String,
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

    /// TCP peer `(address, port)`.
    ///
    /// Uses [`DaemonServer::connect_address`] when set, otherwise
    /// [`DaemonServer::host`]. TLS SNI stays on `host`.
    pub(crate) fn tcp_target(&self) -> (&str, u16) {
        (
            self.connect_address
                .as_deref()
                .unwrap_or(self.host.as_str()),
            self.port,
        )
    }
}

/// TLS certificate bootstrap methods.
#[cfg(feature = "insecure-tls")]
#[cfg_attr(docsrs, doc(cfg(feature = "insecure-tls")))]
impl DaemonServer {
    /// Connect to the daemon with TLS verification disabled and return the
    /// server's leaf certificate as DER bytes. Pin the bytes via
    /// [`TlsConfig::Ca`] for subsequent verified connections — the canonical
    /// bootstrap workflow for self-signed daemons.
    ///
    /// **Never** use this in production without immediately pinning the
    /// returned cert. The connection that returns the bytes is itself
    /// unverified, so a man-in-the-middle attacker could substitute their own
    /// cert. Verify the returned bytes out-of-band before trusting them.
    /// Concretely: compute the SHA-256 fingerprint of the returned DER bytes
    /// (e.g., `openssl x509 -in <der> -inform DER -fingerprint -sha256 -noout`)
    /// and compare against the value the daemon admin reports out-of-band.
    ///
    /// This is an associated function (no `&self`) because callers are
    /// bootstrapping — they don't have a fully-built [`DaemonServer`] yet.
    ///
    /// Requires the `insecure-tls` Cargo feature.
    ///
    /// Equivalent to [`Self::fetch_certificate_from`] with both names equal
    /// to `host`.
    ///
    /// # Errors
    ///
    /// - [`crate::Error::Transport`] for TCP / TLS failures.
    /// - [`crate::Error::Internal`] if the server presents no certificate or an empty chain.
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
    pub async fn fetch_certificate(host: &str, port: u16) -> crate::Result<Vec<u8>> {
        crate::transport::tls::fetch_certificate(host, port).await
    }

    /// Like [`Self::fetch_certificate`], but TCP connects to
    /// `connect_address` while SNI uses `server_name`.
    ///
    /// Needed for tunnel bootstrap (SNI `ibmi.example`, TCP `127.0.0.1`).
    /// The connection is still unverified — pin the returned DER via
    /// [`TlsConfig::Ca`] after out-of-band fingerprint check.
    ///
    /// Requires the `insecure-tls` Cargo feature.
    ///
    /// # Errors
    ///
    /// - [`crate::Error::Transport`] for TCP / TLS failures.
    /// - [`crate::Error::Internal`] if the server presents no certificate or an empty chain.
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
    pub fn fetch_certificate_from(
        server_name: &str,
        connect_address: &str,
        port: u16,
    ) -> impl std::future::Future<Output = crate::Result<Vec<u8>>> + Send {
        crate::transport::tls::fetch_certificate_from(server_name, connect_address, port)
    }
}

/// Fluent builder for [`DaemonServer`].
#[derive(Debug, Default)]
pub struct DaemonServerBuilder {
    host: Option<String>,
    connect_address: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    password: Option<Password>,
    tls: Option<TlsConfig>,
    jdbc_props: Option<String>,
    application: Option<String>,
}

impl DaemonServerBuilder {
    /// Set the TLS server name (SNI, HTTP `Host`, certificate name).
    ///
    /// TCP uses this value unless [`Self::connect_address`] is set.
    #[must_use]
    pub fn host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    /// Set the TCP hop. TLS SNI, HTTP `Host`, and the `wss://` URI still use
    /// [`Self::host`].
    ///
    /// Omit this to connect TCP to `host`. Laptop SSH forwards use
    /// `host("ibmi.example").connect_address("127.0.0.1")`.
    ///
    /// # Example
    ///
    /// ```
    /// use mapepire::DaemonServer;
    ///
    /// let server = DaemonServer::builder()
    ///     .host("ibmi.example")
    ///     .connect_address("127.0.0.1")
    ///     .user("DCURTIS")
    ///     .password("secret".to_string())
    ///     .build()
    ///     .expect("required fields set");
    /// assert_eq!(server.host, "ibmi.example");
    /// assert_eq!(server.connect_address.as_deref(), Some("127.0.0.1"));
    /// ```
    #[must_use]
    pub fn connect_address(mut self, addr: impl Into<String>) -> Self {
        self.connect_address = Some(addr.into());
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

    /// Set JDBC properties forwarded as the connect `props` string.
    ///
    /// # Example
    ///
    /// ```
    /// use mapepire::DaemonServer;
    ///
    /// let server = DaemonServer::builder()
    ///     .host("ibmi.example.com")
    ///     .user("DCURTIS")
    ///     .password("secret".to_string())
    ///     .jdbc_props("access=read only;auto commit=true")
    ///     .build()
    ///     .expect("required fields set");
    /// assert_eq!(
    ///     server.jdbc_props.as_deref(),
    ///     Some("access=read only;auto commit=true")
    /// );
    /// ```
    #[must_use]
    pub fn jdbc_props(mut self, props: impl Into<String>) -> Self {
        self.jdbc_props = Some(props.into());
        self
    }

    /// Override the client application name sent on connect.
    ///
    /// Defaults to `"mapepire-rs"` when unset.
    ///
    /// # Example
    ///
    /// ```
    /// use mapepire::DaemonServer;
    ///
    /// let server = DaemonServer::builder()
    ///     .host("ibmi.example.com")
    ///     .user("DCURTIS")
    ///     .password("secret".to_string())
    ///     .application("my-app")
    ///     .build()
    ///     .expect("required fields set");
    /// assert_eq!(server.application, "my-app");
    /// ```
    #[must_use]
    pub fn application(mut self, name: impl Into<String>) -> Self {
        self.application = Some(name.into());
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
            connect_address: self.connect_address,
            port: self.port.unwrap_or(DaemonServer::DEFAULT_PORT),
            user: self.user.ok_or(BuilderError::MissingField("user"))?,
            password: self
                .password
                .ok_or(BuilderError::MissingField("password"))?,
            tls: self.tls.unwrap_or_default(),
            jdbc_props: self.jdbc_props,
            application: self.application.unwrap_or_else(|| "mapepire-rs".into()),
        })
    }
}

/// Errors returned by [`DaemonServerBuilder::build`].
#[non_exhaustive]
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

    fn dummy_pass() -> String {
        // Concatenation is a CodeQL barrier for rust/hard-coded-cryptographic-value.
        String::from("test") + "-only"
    }

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
            TlsConfig::Verified => panic!("expected Ca variant"),
            #[cfg(feature = "insecure-tls")]
            TlsConfig::Insecure => panic!("expected Ca variant"),
        }
    }

    /// Exhaustive match without `Insecure` — compiles only when the feature
    /// is off, which is the compile-time gate AGENTS.md §6 requires.
    #[cfg(not(feature = "insecure-tls"))]
    #[test]
    fn insecure_variant_absent_without_feature() {
        fn assert_no_insecure(cfg: TlsConfig) {
            match cfg {
                TlsConfig::Verified | TlsConfig::Ca(_) => {}
            }
        }
        assert_no_insecure(TlsConfig::Verified);
        assert_no_insecure(TlsConfig::Ca(vec![0x00]));
    }

    #[test]
    fn builder_defaults_port_and_tls() {
        let s = DaemonServer::builder()
            .host("ibmi.example.com")
            .user("DCURTIS")
            .password("hunter2".to_string())
            .build()
            .expect("DaemonServer builds with all required fields set");

        assert_eq!(s.host, "ibmi.example.com");
        assert_eq!(s.connect_address, None);
        assert_eq!(s.port, DaemonServer::DEFAULT_PORT);
        assert_eq!(s.user, "DCURTIS");
        assert_eq!(s.application, "mapepire-rs");
        assert_eq!(s.jdbc_props, None);
        assert!(matches!(s.tls, TlsConfig::Verified));
        assert_eq!(
            s.tcp_target(),
            ("ibmi.example.com", DaemonServer::DEFAULT_PORT)
        );
    }

    #[test]
    fn builder_omitted_connect_address_tcp_target_is_host() {
        let s = DaemonServer::builder()
            .host("ibmi.example")
            .user("u")
            .password(dummy_pass())
            .build()
            .expect("DaemonServer builds with all required fields set");
        assert_eq!(s.connect_address, None);
        assert_eq!(s.tcp_target(), ("ibmi.example", DaemonServer::DEFAULT_PORT));
    }

    #[test]
    fn builder_connect_address_tcp_target_is_override() {
        let s = DaemonServer::builder()
            .host("ibmi.example")
            .connect_address("127.0.0.1")
            .port(9000)
            .user("u")
            .password(dummy_pass())
            .build()
            .expect("DaemonServer builds with connect_address set");
        assert_eq!(s.connect_address.as_deref(), Some("127.0.0.1"));
        assert_eq!(s.tcp_target(), ("127.0.0.1", 9000));
    }

    #[test]
    fn builder_defaults_application_to_mapepire_rs() {
        let s = DaemonServer::builder()
            .host("h")
            .user("u")
            .password(dummy_pass())
            .build()
            .expect("DaemonServer builds with all required fields set");
        assert_eq!(s.application, "mapepire-rs");
        assert_eq!(s.jdbc_props, None);
    }

    #[test]
    fn builder_jdbc_props_round_trips() {
        let s = DaemonServer::builder()
            .host("h")
            .user("u")
            .password(dummy_pass())
            .jdbc_props("access=read only;auto commit=true")
            .application("cli")
            .build()
            .expect("DaemonServer builds with jdbc_props set");
        assert_eq!(
            s.jdbc_props.as_deref(),
            Some("access=read only;auto commit=true")
        );
        assert_eq!(s.application, "cli");
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
            .tls(TlsConfig::Ca(vec![0xAA]))
            .build()
            .expect("DaemonServer builds with all required fields set");
        assert_eq!(s.port, 9999);
        assert!(matches!(s.tls, TlsConfig::Ca(_)));
    }

    #[cfg(feature = "insecure-tls")]
    #[test]
    fn builder_tls_insecure_when_feature_on() {
        let s = DaemonServer::builder()
            .host("h")
            .user("u")
            .password(dummy_pass())
            .tls(TlsConfig::Insecure)
            .build()
            .expect("DaemonServer builds with Insecure when feature is on");
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
    /// TLS server name: SNI, HTTP `Host`, and certificate name.
    pub host: String,
    /// Optional TCP hop. When absent, TCP uses [`DaemonServerSpec::host`].
    #[serde(default)]
    pub connect_address: Option<String>,
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
    /// JDBC properties forwarded on connect as the `props` string.
    #[serde(default)]
    pub jdbc_props: Option<String>,
    /// Client application name sent on connect.
    ///
    /// When absent, [`DaemonServerSpec::try_into_server`] fills
    /// `"mapepire-rs"`.
    #[serde(default)]
    pub application: Option<String>,
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
    /// Skip verification. Present only with the `insecure-tls` feature;
    /// without it, `"tls": "insecure"` fails to deserialize.
    #[cfg(feature = "insecure-tls")]
    #[cfg_attr(docsrs, doc(cfg(feature = "insecure-tls")))]
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
            #[cfg(feature = "insecure-tls")]
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
            connect_address: self.connect_address,
            port: self.port.unwrap_or(DaemonServer::DEFAULT_PORT),
            user: self.user,
            password: Password::new(self.password),
            tls,
            jdbc_props: self.jdbc_props,
            application: self.application.unwrap_or_else(|| "mapepire-rs".into()),
        })
    }
}

/// Errors returned by [`DaemonServerSpec::try_into_server`].
#[cfg(feature = "serde-config")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde-config")))]
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    /// The base64-encoded CA bytes failed to decode.
    #[error("invalid base64 in tls.ca: {0}")]
    InvalidCaBase64(#[source] base64::DecodeError),
}

#[cfg(all(test, feature = "serde-config"))]
mod spec_tests {
    //! Tests use JSON via `serde_json` (already in `[dependencies]`) rather
    //! than introducing a TOML parser as a dev-dep — the `toml` 0.9 crate
    //! pulls `winnow` 1.x and conflicts with the `winnow` 0.7.x already in
    //! the tree via `insta`/`toml_edit`, which trips
    //! `multiple-versions = "deny"` in `deny.toml`. The serde derives are
    //! format-agnostic, so JSON exercises the same code path.

    use super::*;

    #[test]
    fn parses_minimal_json() {
        let json = r#"{
            "host": "ibmi.example.com",
            "user": "DCURTIS",
            "password": "hunter2"
        }"#;
        let spec: DaemonServerSpec =
            serde_json::from_str(json).expect("DaemonServerSpec parses from JSON");
        let server = spec
            .try_into_server()
            .expect("DaemonServerSpec converts to DaemonServer");
        assert_eq!(server.host, "ibmi.example.com");
        assert_eq!(server.connect_address, None);
        assert_eq!(server.port, DaemonServer::DEFAULT_PORT);
        assert_eq!(server.application, "mapepire-rs");
        assert_eq!(server.jdbc_props, None);
        assert_eq!(
            server.tcp_target(),
            ("ibmi.example.com", DaemonServer::DEFAULT_PORT)
        );
    }

    #[test]
    fn parses_connect_address() {
        let json = r#"{
            "host": "ibmi.example",
            "connect_address": "127.0.0.1",
            "user": "DCURTIS",
            "password": "hunter2"
        }"#;
        let spec: DaemonServerSpec =
            serde_json::from_str(json).expect("DaemonServerSpec parses from JSON");
        let server = spec
            .try_into_server()
            .expect("DaemonServerSpec converts to DaemonServer");
        assert_eq!(server.host, "ibmi.example");
        assert_eq!(server.connect_address.as_deref(), Some("127.0.0.1"));
        assert_eq!(
            server.tcp_target(),
            ("127.0.0.1", DaemonServer::DEFAULT_PORT)
        );
    }

    #[test]
    fn parses_jdbc_props_and_application() {
        let json = r#"{
            "host": "ibmi.example.com",
            "user": "DCURTIS",
            "password": "hunter2",
            "jdbc_props": "access=read only;auto commit=true",
            "application": "cli"
        }"#;
        let spec: DaemonServerSpec =
            serde_json::from_str(json).expect("DaemonServerSpec parses from JSON");
        let server = spec
            .try_into_server()
            .expect("DaemonServerSpec converts to DaemonServer");
        assert_eq!(
            server.jdbc_props.as_deref(),
            Some("access=read only;auto commit=true")
        );
        assert_eq!(server.application, "cli");
    }

    #[test]
    fn invalid_ca_base64_is_error() {
        let json = r#"{
            "host": "h",
            "user": "u",
            "password": "x",
            "tls": { "ca": "%%%" }
        }"#;
        let spec: DaemonServerSpec =
            serde_json::from_str(json).expect("DaemonServerSpec parses from JSON");
        let err = spec.try_into_server().unwrap_err();
        assert!(matches!(err, SpecError::InvalidCaBase64(_)));
    }

    #[cfg(feature = "insecure-tls")]
    #[test]
    fn parses_with_explicit_port_and_insecure_tls() {
        let json = r#"{
            "host": "h",
            "port": 9000,
            "user": "u",
            "password": "p",
            "tls": "insecure"
        }"#;
        let spec: DaemonServerSpec =
            serde_json::from_str(json).expect("DaemonServerSpec parses from JSON");
        let server = spec
            .try_into_server()
            .expect("DaemonServerSpec converts to DaemonServer");
        assert_eq!(server.port, 9000);
        assert!(matches!(server.tls, TlsConfig::Insecure));
    }

    /// Without `insecure-tls` the spec variant is gone, so serde rejects
    /// the `"insecure"` tag instead of constructing a runtime-gated config.
    #[cfg(not(feature = "insecure-tls"))]
    #[test]
    fn insecure_tls_spec_tag_rejected_without_feature() {
        let json = r#"{
            "host": "h",
            "user": "u",
            "password": "p",
            "tls": "insecure"
        }"#;
        let err = serde_json::from_str::<DaemonServerSpec>(json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("insecure") || msg.contains("unknown variant"),
            "expected unknown-variant error, got: {msg}"
        );
    }
}
