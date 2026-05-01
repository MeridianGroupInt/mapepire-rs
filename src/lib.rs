//! # mapepire
//!
//! Async Rust client for [Mapepire](https://mapepire-ibmi.github.io/) — a
//! cloud-friendly access layer for **Db2 for IBM i** that exposes the database
//! over TLS-secured `WebSockets`.
//!
//! This crate is the v0.1 protocol foundation: types, error taxonomy, and
//! configuration. Transport, connection, and pooling land in subsequent
//! milestones.
//!
//! See `AGENTS.md` at the repository root for contributor and AI-assistant
//! conventions.
//!
//! ## Building a `DaemonServer`
//!
//! ```
//! use mapepire::{DaemonServer, TlsConfig};
//!
//! let server = DaemonServer::builder()
//!     .host("ibmi.example.com")
//!     .user("DCURTIS")
//!     .password("hunter2".to_string())
//!     .tls(TlsConfig::Verified)
//!     .build()
//!     .expect("missing required field");
//!
//! assert_eq!(server.port, DaemonServer::DEFAULT_PORT);
//! ```
//!
//! ## Encoding a request
//!
//! ```
//! use mapepire::protocol::request::Request;
//!
//! let r = Request::Sql {
//!     id: "1".into(),
//!     sql: "SELECT 1 FROM SYSIBM.SYSDUMMY1".into(),
//!     rows: None,
//!     parameters: None,
//! };
//! let json = serde_json::to_string(&r).expect("serialize");
//! assert!(json.contains(r#""type":"sql""#));
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(not(any(feature = "rustls-tls", feature = "native-tls")))]
compile_error!(
    "mapepire requires one of: feature `rustls-tls` (default) or feature `native-tls`. \
     Disable default features only when explicitly enabling another TLS backend."
);

pub mod config;
pub mod error;
pub mod password;
pub mod protocol;

pub(crate) mod transport;

pub use crate::config::{BuilderError, DaemonServer, DaemonServerBuilder, TlsConfig};
#[cfg(feature = "serde-config")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde-config")))]
pub use crate::config::{DaemonServerSpec, SpecError, TlsConfigSpec};
pub use crate::error::{
    DecodeError, DiagnosticItem, Error, ProtocolError, Result, ServerError, TransportError,
};
pub use crate::password::Password;
pub use crate::protocol::{
    ClMessage, Column, ErrorResponse, IdAllocator, QueryMetaData, QueryResult, Request, RequestId,
    Response,
};
