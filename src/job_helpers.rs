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

/// Spawn a fire-and-forget task that runs to completion if (and only if)
/// a Tokio runtime is present in the calling thread.
///
/// Used by `Drop for Job` and `Drop for Query` to issue best-effort
/// `Exit` / `SqlClose` requests without panicking from a destructor.
/// `Handle::try_current()` returns `Err` when no runtime is active
/// (test teardown, blocking thread, panic unwind). In that case we
/// silently skip — a destructor panic during unwinding becomes a
/// `process::abort` and obscures the original failure.
///
/// The future runs to its first `Pending` after the spawn returns; the
/// task may or may not complete before the dispatcher (held on the
/// owning `Job`) is aborted by its own `Drop`. Callers should rely
/// neither on completion nor on the result.
pub(crate) fn spawn_best_effort<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        rt.spawn(future);
    }
}
