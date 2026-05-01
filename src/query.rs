//! Prepared-statement handle (`Query`) + result-set types (`Rows`).
//!
//! Entry points (`Job::prepare`, `Job::execute`) land in Task 13.

use crate::error::Error;
use crate::protocol::{IdAllocator, QueryResult, Request, Response};
use crate::transport::DispatcherHandle;

/// Server-side prepared-statement handle.
///
/// Constructed by [`crate::Job::prepare`]. Holds the `cont_id` assigned by
/// the daemon so that subsequent `execute` and `execute_batch` calls can
/// reference the same server-side cursor.
///
/// Drop sends a best-effort `sqlclose` to release the server-side cursor
/// (Task 18).
///
/// `Query` is deliberately `!Clone` — ownership is exclusive. Wrap in
/// `Arc<Mutex<Query>>` or use a separate prepared statement per task if you
/// need shared access.
///
/// # Example
///
/// ```no_run
/// # use mapepire::{DaemonServer, Job, TlsConfig};
/// # async fn example() -> mapepire::Result<()> {
/// # let server = DaemonServer::builder()
/// #     .host("ibmi.example.com")
/// #     .user("MYUSER")
/// #     .password("s3cret".to_string())
/// #     .tls(TlsConfig::Verified)
/// #     .build()
/// #     .expect("missing required field");
/// let job = Job::connect(&server).await?;
/// let query = job.prepare("SELECT * FROM ORDERS WHERE CUSTNO = ?").await?;
/// let rows = query
///     .execute_with(job.ids(), &[serde_json::json!(42)])
///     .await?;
/// drop(rows);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Query {
    /// Server-assigned continuation id for this prepared statement.
    cont_id: String,
    /// Cloned dispatcher handle for issuing follow-up requests.
    handle: DispatcherHandle,
}

impl Query {
    /// Create a new `Query` from a `cont_id` returned by a `PrepareSql`
    /// response and a clone of the connection's [`DispatcherHandle`].
    pub(crate) fn new(cont_id: String, handle: DispatcherHandle) -> Self {
        Self { cont_id, handle }
    }

    /// Execute the prepared statement with no parameters.
    ///
    /// # Errors
    ///
    /// [`Error::Server`] for daemon-side SQL errors (with SQLSTATE);
    /// [`Error::Transport`]/[`Error::Protocol`] for connection issues.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Job, TlsConfig};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let job = Job::connect(&server).await?;
    /// let query = job.prepare("SELECT 1 FROM SYSIBM.SYSDUMMY1").await?;
    /// let rows = query.execute(job.ids()).await?;
    /// drop(rows);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute(&self, ids: &IdAllocator) -> Result<Rows, Error> {
        self.execute_inner(ids, None).await
    }

    /// Execute the prepared statement with a single parameter set.
    ///
    /// # Errors
    ///
    /// [`Error::Server`] for daemon-side SQL errors (with SQLSTATE);
    /// [`Error::Transport`]/[`Error::Protocol`] for connection issues.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Job, TlsConfig};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let job = Job::connect(&server).await?;
    /// let query = job.prepare("SELECT * FROM ORDERS WHERE CUSTNO = ?").await?;
    /// let rows = query
    ///     .execute_with(job.ids(), &[serde_json::json!(42)])
    ///     .await?;
    /// drop(rows);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_with(
        &self,
        ids: &IdAllocator,
        params: &[serde_json::Value],
    ) -> Result<Rows, Error> {
        self.execute_inner(ids, Some(params.to_vec())).await
    }

    async fn execute_inner(
        &self,
        ids: &IdAllocator,
        params: Option<Vec<serde_json::Value>>,
    ) -> Result<Rows, Error> {
        let id = ids.next();
        let request = Request::Execute {
            id: id.clone(),
            cont_id: self.cont_id.clone(),
            parameters: params,
        };
        let resp = self.handle.send(request).await?;
        match resp {
            Response::QueryResult(q) if q.id == id => Ok(Rows::new(q, self.handle.clone())),
            Response::Error(e) => Err(crate::job_helpers::server_error(e)),
            ref other => Err(crate::job_helpers::unexpected(other)),
        }
    }

    /// Execute the prepared statement once per parameter set in `batches`.
    /// Returns one [`Rows`] per execution.
    ///
    /// The first execution failure is returned as `Err`; results from
    /// previously-completed executions in this call are not returned
    /// (fail-fast). No transactional rollback is performed — use an explicit
    /// SQL transaction if atomicity is required. A "best-effort all" mode may
    /// be added in v0.3.
    ///
    /// # Errors
    ///
    /// [`Error::Server`] for daemon-side SQL errors (with SQLSTATE);
    /// [`Error::Transport`]/[`Error::Protocol`] for connection issues.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Job, TlsConfig};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let job = Job::connect(&server).await?;
    /// let query = job.prepare("INSERT INTO T VALUES(?)").await?;
    /// let batches: &[&[serde_json::Value]] = &[&[serde_json::json!(1)], &[serde_json::json!(2)]];
    /// let results = query.execute_batch(job.ids(), batches).await?;
    /// assert_eq!(results.len(), 2);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute_batch(
        &self,
        ids: &IdAllocator,
        batches: &[&[serde_json::Value]],
    ) -> Result<Vec<Rows>, Error> {
        let mut out = Vec::with_capacity(batches.len());
        for params in batches {
            out.push(self.execute_with(ids, params).await?);
        }
        Ok(out)
    }
}

/// Result-set rows and paging cursor. Constructed by [`crate::Job::execute`],
/// [`crate::Job::execute_with`], and [`Query::execute_with`].
///
/// The first page of rows is available immediately on the `inner` field.
/// Additional pages are fetched lazily via `sqlmore` (Task 16).
///
/// Like [`Query`], `Rows` is `!Clone` — it owns the server-side result-set
/// state. Dropping a fully-consumed `Rows` does not need to issue a close;
/// dropping a partially-consumed one issues a best-effort `sqlclose`
/// (Task 18).
// NOTE: doc example deferred until Task 16/17 adds Row accessors.
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
    pub(crate) fn new(inner: QueryResult, handle: DispatcherHandle) -> Self {
        Self { inner, handle }
    }
}
