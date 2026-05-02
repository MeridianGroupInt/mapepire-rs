//! Prepared-statement handle (`Query`) + result-set types (`Rows`, `Row`).
//!
//! Entry points (`Job::prepare`, `Job::execute`) were added in Task 13.
//! Paging via `sqlmore` and row-level access land in Task 16.

use std::sync::Arc;

use crate::error::Error;
use crate::protocol::{IdAllocator, QueryResult, Request, Response};
use crate::transport::DispatcherHandle;

/// Internal state machine for [`Rows::stream`].
///
/// Kept at module level so the closure in `unfold` doesn't define a struct
/// after local `let` statements (clippy `items_after_statements`).
struct StreamState {
    rows: std::vec::IntoIter<serde_json::Map<String, serde_json::Value>>,
    cont_id: Option<String>,
    done: bool,
    handle: DispatcherHandle,
    ids: Arc<IdAllocator>,
}

impl Drop for StreamState {
    fn drop(&mut self) {
        // If the stream was dropped mid-iteration with the cursor still
        // open, fire a best-effort sqlclose. `done` is set when the
        // server reports `is_done = true` (see `unfold` body), so we
        // skip the close in the natural-exhaustion path.
        if !self.done {
            if let Some(cont_id) = self.cont_id.take() {
                spawn_close(self.handle.clone(), cont_id);
            }
        }
    }
}

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
    pub async fn execute(&self, ids: &IdAllocator) -> crate::Result<Rows> {
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
    ) -> crate::Result<Rows> {
        self.execute_inner(ids, Some(params.to_vec())).await
    }

    async fn execute_inner(
        &self,
        ids: &IdAllocator,
        params: Option<Vec<serde_json::Value>>,
    ) -> crate::Result<Rows> {
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
    ) -> crate::Result<Vec<Rows>> {
        let mut out = Vec::with_capacity(batches.len());
        for params in batches {
            out.push(self.execute_with(ids, params).await?);
        }
        Ok(out)
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        // Best-effort sqlclose — share the helper with `Drop for Rows`
        // and the inner `StreamState` so all three call sites use the
        // same semantics (id format, runtime guard, error swallowing).
        spawn_close(self.handle.clone(), self.cont_id.clone());
    }
}

/// Issue a fire-and-forget `sqlclose` for `cont_id` on `handle`.
///
/// Shared between `Drop for Query`, `Drop for Rows`, and the inner
/// `Drop for StreamState` so every cursor-owning destructor uses the
/// same id format and runtime guard.
///
/// If the originating `Job` has already been dropped, the dispatcher's
/// mpsc receiver is gone — `handle.send(SqlClose)` returns
/// `TransportError::Closed` and the `let _` swallows it. The server-side
/// cursor then leaks until the daemon's idle timer reaps it, matching the
/// protocol's normal idle-expiry path. Acceptable for v0.2.
///
/// See [`crate::job_helpers::spawn_best_effort`] for the runtime-guard
/// rationale.
pub(crate) fn spawn_close(handle: DispatcherHandle, cont_id: String) {
    // cont_id is server-issued and unique per cursor / prepared
    // statement, so `close-{cont_id}` is also unique among pending
    // dispatcher entries — no IdAllocator coupling needed.
    let id = format!("close-{cont_id}");
    crate::job_helpers::spawn_best_effort(async move {
        let _ = handle.send(Request::SqlClose { id, cont_id }).await;
    });
}

/// Result-set rows and paging cursor. Constructed by [`crate::Job::execute`],
/// [`crate::Job::execute_with`], and [`Query::execute_with`].
///
/// The first page of rows is available immediately in the `inner` field.
/// Additional pages are fetched lazily on demand by [`Rows::stream`] via
/// `sqlmore`.
///
/// Like [`Query`], `Rows` is `!Clone` — it owns the server-side result-set
/// state. Dropping a fully-consumed `Rows` (the server set `is_done`) does
/// not issue a close; dropping a partially-consumed one issues a
/// best-effort `sqlclose` to release the server-side cursor. Once
/// [`Rows::stream`] is called the cursor moves into the returned stream's
/// state, and the same best-effort `sqlclose` fires if the stream is
/// dropped before exhaustion.
///
/// # Example
///
/// ```no_run
/// # use mapepire::{DaemonServer, Job, Row, TlsConfig};
/// # use futures::{StreamExt, pin_mut};
/// # async fn example() -> mapepire::Result<()> {
/// # let server = DaemonServer::builder()
/// #     .host("ibmi.example.com")
/// #     .user("MYUSER")
/// #     .password("s3cret".to_string())
/// #     .tls(TlsConfig::Verified)
/// #     .build()
/// #     .expect("missing required field");
/// let job = Job::connect(&server).await?;
/// let rows = job
///     .execute("SELECT EMPNO, FIRSTNME FROM CORPDATA.EMPLOYEE")
///     .await?;
/// let stream = rows.stream();
/// pin_mut!(stream);
/// while let Some(result) = stream.next().await {
///     let row: Row = result?;
///     let empno: String = row.get("EMPNO")?;
///     let name: String = row.get("FIRSTNME")?;
///     println!("{empno}: {name}");
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Rows {
    /// The first page of rows from the initial SQL response.
    inner: QueryResult,
    /// Cloned dispatcher handle for issuing `sqlmore` / `sqlclose`.
    handle: DispatcherHandle,
}

impl Rows {
    /// Create a new `Rows` from the initial [`QueryResult`] and a clone of
    /// the connection's [`DispatcherHandle`].
    pub(crate) fn new(inner: QueryResult, handle: DispatcherHandle) -> Self {
        Self { inner, handle }
    }

    /// Number of rows affected for INSERT/UPDATE/DELETE; `None` for SELECT.
    #[must_use]
    pub fn update_count(&self) -> Option<i64> {
        if self.inner.has_results || self.inner.update_count < 0 {
            None
        } else {
            Some(self.inner.update_count)
        }
    }

    /// `true` if the query produced a result set (SELECT), `false` for DML / DDL.
    #[must_use]
    pub fn has_results(&self) -> bool {
        self.inner.has_results
    }

    /// Wall-clock execution time on the server.
    ///
    /// The server reports duration in milliseconds; this method converts
    /// to [`std::time::Duration`].
    #[must_use]
    pub fn execution_time(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.inner.execution_time / 1000.0)
    }

    /// Stream rows as a [`futures::Stream`].
    ///
    /// Yields rows from the in-memory first page first. When the first page is
    /// exhausted and `is_done` is `false`, sends a `sqlmore` request for the
    /// next page (100 rows per fetch) and continues. Repeats until `is_done`.
    ///
    /// Each `Rows::stream` call creates a **fresh [`IdAllocator`]** scoped to
    /// the stream — this avoids contention with the [`crate::Job`]-level
    /// allocator. The `cont_id` is the only persistent server-side identifier;
    /// the per-stream id sequence is safe as long as each call's ids are
    /// unique within that stream, which the allocator guarantees.
    ///
    /// Dropping the stream mid-fetch cancels the in-flight `sqlmore` future.
    /// The dispatcher will silently discard the response when it arrives —
    /// no resource leak occurs. The stream's internal `Drop` then issues a
    /// best-effort `sqlclose` for the cursor (no `await`, no panic, no
    /// runtime requirement); if the originating [`crate::Job`] has already
    /// been dropped the close is dropped too and the daemon's idle timer
    /// reaps the cursor.
    ///
    /// If the server returns an empty page **without** setting `is_done`, the
    /// stream yields [`Error::Internal`] and halts — this indicates a daemon
    /// bug and we surface it loudly to prevent an infinite poll loop.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Job, Row, TlsConfig};
    /// # use futures::{StreamExt, pin_mut};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let job = Job::connect(&server).await?;
    /// let rows = job.execute("SELECT EMPNO FROM CORPDATA.EMPLOYEE").await?;
    /// let stream = rows.stream();
    /// pin_mut!(stream);
    /// while let Some(result) = stream.next().await {
    ///     let row: Row = result?;
    ///     let empno: String = row.get("EMPNO")?;
    ///     println!("{empno}");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn stream(mut self) -> impl futures::Stream<Item = crate::Result<Row>> {
        use futures::stream::unfold;

        // We own the cursor through `self.inner.cont_id`, but we cannot move
        // fields out of `self` because `Rows` has a `Drop` impl. Instead:
        //  * `take()` the cont_id (leaves `None`) so `Drop for Rows` no-ops when `self` drops at
        //    the end of this function — the cursor ownership has transferred to `StreamState`
        //    cleanly.
        //  * `take()` the data Vec (leaves an empty Vec) so we don't clone the first page.
        //  * `clone()` the dispatcher handle (cheap — Arc<…> internally).
        let cont_id = self.inner.cont_id.take();
        let data = std::mem::take(&mut self.inner.data);
        let handle = self.handle.clone();
        let ids = Arc::new(IdAllocator::new());

        unfold(
            StreamState {
                rows: data.into_iter(),
                cont_id,
                done: self.inner.is_done,
                handle,
                ids,
            },
            |mut state| async move {
                if let Some(row_data) = state.rows.next() {
                    return Some((Ok(Row { data: row_data }), state));
                }
                if state.done {
                    return None;
                }
                let cont_id = match &state.cont_id {
                    Some(c) => c.clone(),
                    None => return None,
                };
                let id = state.ids.next();
                let resp = state
                    .handle
                    .send(Request::SqlMore {
                        id: id.clone(),
                        cont_id,
                        rows: 100,
                    })
                    .await;
                match resp {
                    Ok(Response::QueryResult(q)) if q.id == id => {
                        state.rows = q.data.into_iter();
                        state.done = q.is_done;
                        state.cont_id = q.cont_id;
                        if let Some(row_data) = state.rows.next() {
                            Some((Ok(Row { data: row_data }), state))
                        } else if state.done {
                            None
                        } else {
                            // Empty page without is_done is a daemon bug. Issue a best-effort
                            // close so the server-side cursor doesn't leak waiting for the idle
                            // timer, then surface Error::Internal and terminate the stream —
                            // set done = true so a careless caller who polls again gets None
                            // instead of repeatedly re-issuing sqlmore against the same
                            // misbehaving cursor.
                            if let Some(cid) = state.cont_id.take() {
                                spawn_close(state.handle.clone(), cid);
                            }
                            state.done = true;
                            Some((
                                Err(Error::Internal(
                                    "server returned empty page without is_done".into(),
                                )),
                                state,
                            ))
                        }
                    }
                    Ok(Response::Error(e)) => {
                        Some((Err(crate::job_helpers::server_error(e)), state))
                    }
                    // Defensive catch-all: the dispatcher routes responses by id, so a
                    // mismatched-id QueryResult or any other variant arriving here would
                    // indicate a dispatcher routing bug. Surface as Error::Protocol.
                    Ok(other) => Some((Err(crate::job_helpers::unexpected(&other)), state)),
                    Err(e) => Some((Err(e), state)),
                }
            },
        )
    }

    /// Eagerly materialize all rows into `Vec<T>` via `serde::Deserialize`.
    ///
    /// Consumes `self`, drives [`Rows::stream`] to exhaustion, and
    /// deserializes each row's JSON object into `T`.
    ///
    /// Stream-level errors (transport, protocol, server-side) propagate
    /// before per-row decode errors: the stream is fully collected first,
    /// then each [`Result<T, Error>`] is folded via [`Iterator::collect`].
    ///
    /// # Note
    ///
    /// The entire result set is held in memory until the returned `Vec`
    /// is dropped. For large result sets prefer [`Rows::stream`] to
    /// process rows incrementally.
    ///
    /// # Errors
    ///
    /// [`Error::Transport`] / [`Error::Protocol`] / [`Error::Server`] for
    /// paging failures; [`Error::Decode`] for per-row deserialization
    /// failures.
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
    /// #[derive(serde::Deserialize)]
    /// struct Stat {
    ///     name: String,
    ///     count: i64,
    /// }
    /// let rows = job.execute("SELECT name, count FROM stats").await?;
    /// let stats: Vec<Stat> = rows.into_typed().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn into_typed<T>(self) -> crate::Result<Vec<T>>
    where
        T: serde::de::DeserializeOwned,
    {
        use futures::TryStreamExt;
        self.stream()
            .map_ok(|row| {
                serde_json::from_value::<T>(serde_json::Value::Object(row.data)).map_err(|e| {
                    Error::Decode {
                        column: None,
                        source: crate::error::DecodeError::Serde(e.to_string()),
                    }
                })
            })
            .try_collect::<Vec<Result<T, Error>>>()
            .await?
            .into_iter()
            .collect()
    }

    /// Eagerly materialize all rows into `Vec<Row>`.
    ///
    /// Consumes `self` and drives [`Rows::stream`] to exhaustion via
    /// [`futures::TryStreamExt::try_collect`].
    ///
    /// # Note
    ///
    /// The entire result set is held in memory until the returned `Vec`
    /// is dropped. For large result sets prefer [`Rows::stream`] to
    /// process rows incrementally.
    ///
    /// # Errors
    ///
    /// [`Error::Transport`] / [`Error::Protocol`] / [`Error::Server`] for
    /// paging failures.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Job, Row, TlsConfig};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let job = Job::connect(&server).await?;
    /// let rows = job
    ///     .execute("SELECT EMPNO, FIRSTNME FROM CORPDATA.EMPLOYEE")
    ///     .await?;
    /// let all_rows: Vec<Row> = rows.into_dynamic().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn into_dynamic(self) -> crate::Result<Vec<Row>> {
        use futures::TryStreamExt;
        self.stream().try_collect().await
    }
}

impl Drop for Rows {
    fn drop(&mut self) {
        // Best-effort sqlclose for the server-side cursor when the user
        // drops `Rows` *without* having called `stream()` /
        // `into_typed()` / `into_dynamic()` (each of which transfers
        // ownership of the cursor into a `StreamState`).
        //
        // Skip when the result set was fully delivered in the first
        // page (`is_done == true`) — the server has already released
        // the cursor and there's nothing to close. Skip when
        // `cont_id` is `None` for the same reason, including the
        // post-`stream()` case where `stream` already `take()`d it.
        if !self.inner.is_done {
            if let Some(cont_id) = self.inner.cont_id.take() {
                spawn_close(self.handle.clone(), cont_id);
            }
        }
    }
}

/// A single result-set row returned by [`Rows::stream`].
///
/// Values are stored as a JSON object keyed by column name. Use [`Row::get`]
/// to deserialize a column value into a Rust type, or [`Row::try_get`] when
/// you want to distinguish "column absent" from "decode failure".
///
/// `Row` derives `Clone`, which deep-copies the inner column map. For
/// wide rows or large text/CLOB columns this can be expensive — prefer
/// borrowing where possible.
///
/// # Example
///
/// ```no_run
/// # use mapepire::{DaemonServer, Job, Row, TlsConfig};
/// # use futures::{StreamExt, pin_mut};
/// # async fn example() -> mapepire::Result<()> {
/// # let server = DaemonServer::builder()
/// #     .host("ibmi.example.com")
/// #     .user("MYUSER")
/// #     .password("s3cret".to_string())
/// #     .tls(TlsConfig::Verified)
/// #     .build()
/// #     .expect("missing required field");
/// let job = Job::connect(&server).await?;
/// let rows = job
///     .execute("SELECT EMPNO, SALARY FROM CORPDATA.EMPLOYEE")
///     .await?;
/// let stream = rows.stream();
/// pin_mut!(stream);
/// if let Some(result) = stream.next().await {
///     let row: Row = result?;
///     let empno: String = row.get("EMPNO")?;
///     let salary: Option<f64> = row.try_get("SALARY").and_then(|r| r.ok());
///     println!("{empno}: {salary:?}");
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Row {
    data: serde_json::Map<String, serde_json::Value>,
}

impl Row {
    /// Get a typed value by column name.
    ///
    /// # Errors
    ///
    /// [`Error::Decode`] if the column is missing or the value can't be
    /// deserialized as `T`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Job, Row, TlsConfig};
    /// # use futures::{StreamExt, pin_mut};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let job = Job::connect(&server).await?;
    /// let rows = job.execute("SELECT 1 AS N FROM SYSIBM.SYSDUMMY1").await?;
    /// let stream = rows.stream();
    /// pin_mut!(stream);
    /// if let Some(result) = stream.next().await {
    ///     let row = result?;
    ///     let n: i64 = row.get("N")?;
    ///     assert_eq!(n, 1);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn get<T: serde::de::DeserializeOwned>(&self, column: &str) -> crate::Result<T> {
        use crate::error::DecodeError;
        let value = self.data.get(column).ok_or_else(|| Error::Decode {
            column: Some(column.to_owned()),
            source: DecodeError::MissingColumn(column.to_owned()),
        })?;
        T::deserialize(value).map_err(|e| Error::Decode {
            column: Some(column.to_owned()),
            source: DecodeError::Serde(e.to_string()),
        })
    }

    /// Same as [`Row::get`] but returns `None` if the column is missing
    /// (instead of [`Error::Decode`]), and `Some(Err(...))` if the value
    /// exists but cannot be deserialized as `T`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Job, Row, TlsConfig};
    /// # use futures::{StreamExt, pin_mut};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let job = Job::connect(&server).await?;
    /// let rows = job.execute("SELECT SALARY FROM CORPDATA.EMPLOYEE").await?;
    /// let stream = rows.stream();
    /// pin_mut!(stream);
    /// if let Some(result) = stream.next().await {
    ///     let row = result?;
    ///     // Returns None if "SALARY" column is absent; Some(Err) if present
    ///     // but not deserializable as f64.
    ///     let salary: Option<f64> = row.try_get("SALARY").transpose()?;
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn try_get<T: serde::de::DeserializeOwned>(
        &self,
        column: &str,
    ) -> Option<crate::Result<T>> {
        use crate::error::DecodeError;
        let value = self.data.get(column)?;
        Some(T::deserialize(value).map_err(|e| Error::Decode {
            column: Some(column.to_owned()),
            source: DecodeError::Serde(e.to_string()),
        }))
    }
}
