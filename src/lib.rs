#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc = include_str!("../README.md")]
//!
//! ---
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
//! `host` is SNI, HTTP `Host`, and the certificate name. TCP uses
//! `connect_address` when set (otherwise `host`):
//!
//! ```
//! use mapepire::DaemonServer;
//!
//! let server = DaemonServer::builder()
//!     .host("ibmi.example")
//!     .connect_address("127.0.0.1")
//!     .user("DCURTIS")
//!     .password("hunter2".to_string())
//!     .build()
//!     .expect("missing required field");
//!
//! assert_eq!(server.host, "ibmi.example");
//! assert_eq!(server.connect_address.as_deref(), Some("127.0.0.1"));
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
//!     terse: None,
//! };
//! let json = serde_json::to_string(&r).expect("Request serializes to JSON");
//! assert!(json.contains(r#""type":"sql""#));
//! ```
//!
//! On a live daemon, [`crate::Job::version`] may be empty after connect
//! (the connect frame often omits it). Call [`crate::Job::server_version`]
//! (`getversion`) for the version string.

#[cfg(not(any(feature = "rustls-tls", feature = "native-tls")))]
compile_error!(
    "mapepire requires one of: feature `rustls-tls` (default) or feature `native-tls`. \
     Disable default features only when explicitly enabling another TLS backend."
);

pub mod config;
pub mod error;
pub mod password;
pub mod protocol;

pub mod executor;
pub mod from_row;
pub mod job;
pub(crate) mod job_helpers;
#[cfg(feature = "metrics")]
#[cfg_attr(docsrs, doc(cfg(feature = "metrics")))]
pub mod observability;
pub mod pool;
pub mod query;
pub(crate) mod transport;

pub use crate::config::{BuilderError, DaemonServer, DaemonServerBuilder, TlsConfig};
#[cfg(feature = "serde-config")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde-config")))]
pub use crate::config::{DaemonServerSpec, SpecError, TlsConfigSpec};
pub use crate::error::{
    DecodeError, DiagnosticItem, Error, ProtocolError, Result, ServerError, TransportError,
};
pub use crate::executor::Executor;
pub use crate::from_row::FromRow;
pub use crate::job::{ClOutcome, Job, TraceDest, TraceLevel};
pub use crate::password::Password;
pub use crate::pool::{ParameterLogging, Pool, PoolBuilder, PoolStatus, RecyclingMethod, Reserved};
pub use crate::protocol::{
    ClMessage, Column, ErrorResponse, IdAllocator, JobLogEntry, ParameterDetail, ParameterResult,
    QueryMetaData, QueryResult, Request, RequestId, Response,
};
pub use crate::query::{ExecuteOptions, Query, Row, Rows};
