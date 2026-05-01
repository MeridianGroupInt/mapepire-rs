//! Prepared-statement handle (`Query`) + result-set types (`Rows`).
//!
//! Entry points (`Job::prepare`, `Job::execute`) land in Task 13.

use crate::protocol::QueryResult;
use crate::transport::DispatcherHandle;

/// Server-side prepared-statement handle.
///
/// Constructed by `Job::prepare` (Task 13). Holds the `cont_id` assigned by
/// the daemon so that subsequent `execute` and `execute_batch` calls can
/// reference the same server-side cursor.
///
/// Drop sends a best-effort `sqlclose` to release the server-side cursor
/// (Task 18).
///
/// `Query` is deliberately `!Clone` — ownership is exclusive. Wrap in
/// `Arc<Mutex<Query>>` or use a separate prepared statement per task if you
/// need shared access.
// NOTE: doc example deferred to Task 13 (PRO-409) when `Job::prepare` /
// `Job::execute` land. Adding a `no_run` example referencing methods
// that don't exist yet won't compile; the example slot is intentional.
#[allow(dead_code)] // NOTE: fields read first in Task 14 (execute/execute_batch/sqlclose).
#[derive(Debug)]
pub struct Query {
    /// Server-assigned continuation id for this prepared statement.
    // NOTE: first used in Task 14 (execute / execute_batch).
    cont_id: String,
    /// Cloned dispatcher handle for issuing follow-up requests.
    // NOTE: first used in Task 14 (execute / execute_batch / sqlclose).
    handle: DispatcherHandle,
}

impl Query {
    /// Create a new `Query` from a `cont_id` returned by a `PrepareSql`
    /// response and a clone of the connection's [`DispatcherHandle`].
    // NOTE: called first in Task 13 (Job::prepare).
    #[allow(dead_code)]
    pub(crate) fn new(cont_id: String, handle: DispatcherHandle) -> Self {
        Self { cont_id, handle }
    }
}

/// Result-set rows and paging cursor. Constructed by `Job::execute` and
/// `Job::execute_with` (Task 13).
///
/// The first page of rows is available immediately on the `inner` field.
/// Additional pages are fetched lazily via `sqlmore` (Task 16).
///
/// Like [`Query`], `Rows` is `!Clone` — it owns the server-side result-set
/// state. Dropping a fully-consumed `Rows` does not need to issue a close;
/// dropping a partially-consumed one issues a best-effort `sqlclose`
/// (Task 18).
// NOTE: doc example deferred to Task 13 (PRO-409) when `Job::prepare` /
// `Job::execute` land. Adding a `no_run` example referencing methods
// that don't exist yet won't compile; the example slot is intentional.
#[allow(dead_code)] // NOTE: fields read first in Task 16 (Rows paging + Row::get/try_get).
#[derive(Debug)]
pub struct Rows {
    /// The first page of rows from the initial SQL response.
    // NOTE: first used in Task 16 (paging / Row accessors).
    inner: QueryResult,
    /// Cloned dispatcher handle for issuing `sqlmore` / `sqlclose`.
    // NOTE: first used in Task 16 (sqlmore paging).
    handle: DispatcherHandle,
}

impl Rows {
    /// Create a new `Rows` from the initial [`QueryResult`] and a clone of
    /// the connection's [`DispatcherHandle`].
    // NOTE: called first in Task 13 (Job::execute / Job::execute_with).
    #[allow(dead_code)]
    pub(crate) fn new(inner: QueryResult, handle: DispatcherHandle) -> Self {
        Self { inner, handle }
    }
}
