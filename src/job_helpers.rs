//! Shared helpers used by `job` and `query` to map dispatcher responses
//! into typed [`crate::error::Error`]s.

use crate::error::{Error, ProtocolError, ServerError};
use crate::protocol::{ErrorResponse, Response};

/// Map an unexpected [`Response`] variant into a [`ProtocolError`].
pub(crate) fn unexpected(response: &Response) -> Error {
    Error::from(ProtocolError::UnknownResponseType(format!(
        "unexpected variant: {response:?}"
    )))
}

/// Map a daemon-side [`ErrorResponse`] into a [`ServerError`].
pub(crate) fn server_error(e: ErrorResponse) -> Error {
    Error::from(ServerError {
        message: e
            .error
            .unwrap_or_else(|| "daemon returned error response with no message".to_string()),
        sqlstate: e.sqlstate,
        sqlcode: e.sqlcode,
        job_name: e.job,
        diagnostics: vec![],
    })
}

/// Map a `success: false` response for `method` into a [`ServerError`].
pub(crate) fn server_failed(method: &str) -> Error {
    Error::from(ServerError {
        message: format!("daemon returned success=false for {method}"),
        sqlstate: None,
        sqlcode: None,
        job_name: None,
        diagnostics: vec![],
    })
}
