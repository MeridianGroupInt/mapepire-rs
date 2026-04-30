//! Crate-wide error types.
//!
//! All fallible operations in the crate return [`Result<T>`], which is
//! `std::result::Result<T, Error>`. The [`Error`] enum is `#[non_exhaustive]`
//! so adding a new variant is a minor-version bump.

use std::time::Duration;

/// Crate result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Top-level error type.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Network, TLS, or WebSocket transport failure.
    #[error("transport: {0}")]
    Transport(#[source] TransportError),

    /// The Mapepire server returned a structured error response.
    #[error(transparent)]
    Server(ServerError),

    /// The handshake's `connect` request was rejected.
    #[error("authentication failed: {0}")]
    Auth(String),

    /// The wire JSON could not be parsed, or a response did not match its
    /// matching request.
    #[error("protocol: {0}")]
    Protocol(#[source] ProtocolError),

    /// A row could not be decoded into the requested Rust type.
    #[error("decode column {column:?}: {source}")]
    Decode {
        /// Column name when known.
        column: Option<String>,
        /// Underlying decode failure.
        #[source]
        source: DecodeError,
    },

    /// Pool ran out of capacity within the configured acquire timeout.
    #[error("pool exhausted (timeout {timeout:?})")]
    PoolExhausted {
        /// The acquire timeout that was hit.
        timeout: Duration,
    },

    /// Operation was cancelled — the awaiting future was dropped or a
    /// timeout fired.
    #[error("operation cancelled")]
    Cancelled,

    /// An invariant was violated. Indicates a bug; please file an issue.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Network / TLS / WebSocket transport failures.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The underlying socket reported a failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The peer closed the connection.
    #[error("connection closed by peer")]
    Closed,
}

/// Errors that arise while parsing the Mapepire wire protocol.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// The bytes were not valid JSON.
    #[error("malformed JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// A response was received that did not match the corresponding request
    /// `id`.
    #[error("response correlation mismatch: expected id {expected}, got {got}")]
    CorrelationMismatch {
        /// The id the caller was waiting for.
        expected: String,
        /// The id the server actually sent.
        got: String,
    },

    /// The response `type` field was not one of the known variants.
    #[error("unknown response type: {0}")]
    UnknownResponseType(String),
}

/// Failures while decoding a row into a Rust type.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// `serde` rejected the column value.
    #[error("serde: {0}")]
    Serde(String),

    /// The requested column did not exist in the row.
    #[error("column not found: {0}")]
    MissingColumn(String),
}

/// A structured error response from the Mapepire daemon.
#[derive(Debug, Clone)]
pub struct ServerError {
    /// Human-readable error message.
    pub message: String,
    /// Five-character SQLSTATE.
    pub sqlstate: Option<String>,
    /// Db2-native SQLCODE.
    pub sqlcode: Option<i32>,
    /// IBM i job that produced the error.
    pub job_name: Option<String>,
    /// Additional diagnostic items.
    pub diagnostics: Vec<DiagnosticItem>,
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Include `job_name` when present — it's the single most useful field
        // for an IBM i operator chasing the failure in joblog. `diagnostics`
        // stays out of `Display`; render it via `Debug` or a dedicated path.
        match &self.job_name {
            Some(job) => write!(
                f,
                "server error [sqlstate={:?} sqlcode={:?} job={job}]: {}",
                self.sqlstate, self.sqlcode, self.message
            ),
            None => write!(
                f,
                "server error [sqlstate={:?} sqlcode={:?}]: {}",
                self.sqlstate, self.sqlcode, self.message
            ),
        }
    }
}

impl std::error::Error for ServerError {}

impl ServerError {
    /// Returns true for SQLSTATE classes typically considered transient
    /// (`08xxx` connection failure, `40001` deadlock, `57033` timeout).
    #[must_use]
    pub fn is_transient(&self) -> bool {
        match self.sqlstate.as_deref() {
            Some(s) if s.starts_with("08") => true,
            // `40001` / `57033` are individually transient codes within
            // classes that are NOT wholly transient — `40xxx` is "transaction
            // rollback" (many of which are non-recoverable) and `57xxx` is
            // "resource not available" (`57014` "query cancelled" should not
            // be retried). Don't widen these to class matches.
            Some("40001" | "57033") => true,
            _ => false,
        }
    }

    /// Returns true for constraint violations (SQLSTATE class `23xxx`).
    #[must_use]
    pub fn is_constraint_violation(&self) -> bool {
        self.sqlstate
            .as_deref()
            .is_some_and(|s| s.starts_with("23"))
    }

    /// Returns true for authorization failures (SQLSTATE classes `28xxx`,
    /// and SQLSTATE `42501`).
    #[must_use]
    pub fn is_authorization(&self) -> bool {
        match self.sqlstate.as_deref() {
            Some(s) if s.starts_with("28") => true,
            Some("42501") => true,
            _ => false,
        }
    }

    /// Returns true for "table or view not found" SQLSTATEs (`42704`
    /// Db2-native, `42S02` ODBC/IBM i flavor). Deliberately narrow —
    /// column-not-found (`42703` / `42S22`) is a separate failure class
    /// and gets its own predicate when a row layer needs to branch on it.
    #[must_use]
    pub fn is_object_not_found(&self) -> bool {
        matches!(self.sqlstate.as_deref(), Some("42704" | "42S02"))
    }

    /// Returns true for data-type / value-conversion failures
    /// (SQLSTATE class `22xxx`).
    #[must_use]
    pub fn is_data_type_mismatch(&self) -> bool {
        self.sqlstate
            .as_deref()
            .is_some_and(|s| s.starts_with("22"))
    }
}

/// One diagnostic entry from a [`ServerError`]. Mirrors the wire shape;
/// expanded as the protocol is implemented.
#[derive(Debug, Clone)]
pub struct DiagnosticItem {
    /// Vendor-specific message identifier (e.g., CPF code).
    pub message_id: Option<String>,
    /// Human-readable text.
    pub text: String,
}

// --- From impls into Error ----------------------------------------------

impl From<TransportError> for Error {
    fn from(value: TransportError) -> Self {
        Self::Transport(value)
    }
}

impl From<ProtocolError> for Error {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

impl From<ServerError> for Error {
    fn from(value: ServerError) -> Self {
        Self::Server(value)
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Transport(TransportError::Io(value))
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Protocol(ProtocolError::Json(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn srv(sqlstate: Option<&str>) -> ServerError {
        ServerError {
            message: "x".into(),
            sqlstate: sqlstate.map(String::from),
            sqlcode: None,
            job_name: None,
            diagnostics: vec![],
        }
    }

    #[test]
    fn is_transient_classifies() {
        assert!(srv(Some("08001")).is_transient());
        assert!(srv(Some("08S01")).is_transient());
        assert!(srv(Some("40001")).is_transient());
        assert!(srv(Some("57033")).is_transient());
        assert!(!srv(Some("23000")).is_transient());
        assert!(!srv(None).is_transient());
    }

    #[test]
    fn is_constraint_violation_classifies() {
        assert!(srv(Some("23000")).is_constraint_violation());
        assert!(srv(Some("23505")).is_constraint_violation());
        assert!(!srv(Some("22000")).is_constraint_violation());
    }

    #[test]
    fn is_authorization_classifies() {
        assert!(srv(Some("28000")).is_authorization());
        assert!(srv(Some("42501")).is_authorization());
        assert!(!srv(Some("23000")).is_authorization());
    }

    #[test]
    fn is_object_not_found_classifies() {
        assert!(srv(Some("42704")).is_object_not_found());
        assert!(srv(Some("42S02")).is_object_not_found());
        assert!(!srv(Some("42501")).is_object_not_found());
    }

    #[test]
    fn is_data_type_mismatch_classifies() {
        assert!(srv(Some("22001")).is_data_type_mismatch());
        assert!(srv(Some("22018")).is_data_type_mismatch());
        assert!(!srv(Some("23000")).is_data_type_mismatch());
    }

    #[test]
    fn server_error_display() {
        let e = srv(Some("23505"));
        let s = format!("{e}");
        assert!(s.contains("23505"));
        assert!(s.contains('x'));
    }

    #[test]
    fn server_error_display_includes_job_name_when_present() {
        let mut e = srv(Some("23505"));
        e.job_name = Some("QZDASOINIT/QUSER/123456".into());
        let s = format!("{e}");
        assert!(s.contains("QZDASOINIT/QUSER/123456"));
        assert!(s.contains("23505"));
    }

    #[test]
    fn error_server_display_is_transparent() {
        // Locks in the `#[error(transparent)]` wiring on `Error::Server` —
        // the top-level `Error` `Display` must surface the inner
        // `ServerError` text (incl. sqlstate). Replacing `transparent`
        // with a literal format string would compile but break this test.
        let inner = srv(Some("23505"));
        let err: Error = inner.into();
        let s = format!("{err}");
        assert!(
            s.contains("23505"),
            "expected sqlstate in transparent display, got: {s}"
        );
    }

    #[test]
    fn from_io_error_classifies_as_transport() {
        let io = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "bye");
        let err: Error = io.into();
        assert!(matches!(err, Error::Transport(TransportError::Io(_))));
    }

    #[test]
    fn from_serde_json_error_classifies_as_protocol() {
        let parse_err = serde_json::from_str::<i32>("not json").unwrap_err();
        let err: Error = parse_err.into();
        assert!(matches!(err, Error::Protocol(ProtocolError::Json(_))));
    }
}
