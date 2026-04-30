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

pub use crate::config::{BuilderError, DaemonServer, DaemonServerBuilder, TlsConfig};
#[cfg(feature = "serde-config")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde-config")))]
pub use crate::config::{DaemonServerSpec, SpecError, TlsConfigSpec};
pub use crate::error::{
    DecodeError, DiagnosticItem, Error, ProtocolError, Result, ServerError, TransportError,
};
pub use crate::password::Password;
