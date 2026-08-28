//! Single connection to a Mapepire daemon.
//!
//! [`Job`] wraps a per-connection dispatcher task. Construct via
//! [`Job::connect`]. Drop runs a best-effort `exit` to let the daemon
//! shut down cleanly.
//!
//! ## Tracing (optional, `tracing` feature)
//!
//! With the `tracing` feature enabled, every public dispatch method emits a
//! `tracing::Span` named after the method. Common fields:
//!
//! - `job_id` — daemon-reported initial job name (groups spans by Db2 job).
//! - `sql` — SQL text for SQL-bearing methods.
//! - `param_count` — number of parameters for parameterized variants.
//! - `command` — CL command text for [`Job::cl`].
//! - `level` — trace level for [`Job::set_trace`] / [`Job::set_trace_config`].
//! - `dest` — trace destination for [`Job::set_trace_config`].
//!
//! Per-parameter values are governed by per-Pool [`crate::ParameterLogging`]
//! policy (added in Task 9 / PRO-587). Direct-Job users get the equivalent
//! of `ParameterLogging::None` (no parameter values on spans).
//!
//! Zero overhead when the `tracing` feature is disabled.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use crate::config::DaemonServer;
use crate::error::Error;
use crate::protocol::{ClMessage, IdAllocator, JobLogEntry, QueryResult, Request, Response};
use crate::query::ExecuteOptions;
use crate::transport::{self, ConnectedDispatcher, Dispatcher, DispatcherHandle};

/// Trace level for `setconfig.tracelevel`.
///
/// mapepire-js `ServerTraceLevel` is `OFF | ON | ERRORS | DATASTREAM`.
/// [`TraceLevel::All`] is the `"ON"` wire value — the daemon has no
/// `ALL` constant. Use [`Job::set_trace`] or [`Job::set_trace_config`].
///
/// ```
/// use mapepire::TraceLevel;
///
/// assert_eq!(TraceLevel::Off.as_str(), "OFF");
/// assert_eq!(TraceLevel::All.as_str(), "ON");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceLevel {
    /// No tracing (`"OFF"`).
    Off,
    /// Errors only (`"ERRORS"`).
    Errors,
    /// Errors + statement boundaries (`"DATASTREAM"`).
    Datastream,
    /// Full diagnostic (`"ON"` on the wire). High overhead.
    All,
}

impl TraceLevel {
    /// Wire token: `OFF`, `ON`, `ERRORS`, or `DATASTREAM`.
    ///
    /// [`TraceLevel::All`] returns `"ON"` (mapepire-js). Never `"ALL"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            TraceLevel::Off => "OFF",
            TraceLevel::Errors => "ERRORS",
            TraceLevel::Datastream => "DATASTREAM",
            TraceLevel::All => "ON",
        }
    }
}

/// Trace destination for `setconfig.tracedest`.
///
/// Jetty `Tracer.Dest` is `FILE` or `IN_MEM`. Empty string is not a dest
/// (`No enum constant Tracer.Dest`). [`Job::set_trace`] defaults to
/// [`TraceDest::InMem`] so [`Job::fetch_trace`] can read the buffer.
///
/// ```
/// use mapepire::TraceDest;
///
/// assert_eq!(TraceDest::File.as_str(), "FILE");
/// assert_eq!(TraceDest::InMem.as_str(), "IN_MEM");
/// assert_eq!(TraceDest::default(), TraceDest::InMem);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TraceDest {
    /// Write trace records to a server-side file.
    File,
    /// Buffer in memory so [`Job::fetch_trace`] (`gettracedata`) can return them.
    #[default]
    InMem,
}

impl TraceDest {
    /// Wire token: `"FILE"` or `"IN_MEM"`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            TraceDest::File => "FILE",
            TraceDest::InMem => "IN_MEM",
        }
    }
}

/// Shared inner state of a [`Job`].
///
/// Wrapped in [`Arc`] by [`Job`] so v0.3 pool routing (PRO-453) can
/// hand out [`std::sync::Weak`] references to in-flight requests
/// without owning the connection. Dispatcher remains a sibling field on
/// [`Job`] so its abort-on-drop is tied to the `Job`'s lifetime, not
/// the inner Arc's refcount.
pub(crate) struct JobInner {
    pub(crate) handle: DispatcherHandle,
    pub(crate) ids: Arc<IdAllocator>,
    pub(crate) version: String,
    pub(crate) initial_job: String,
    /// Outstanding-request counter, used by the v0.3 pool router for
    /// least-loaded selection. Shared with the dispatcher task via
    /// [`Arc`]: the dispatcher increments after each successful socket
    /// write, decrements when the matching response is routed back to
    /// the caller, and decrements once per drained pending entry on
    /// socket-close paths.
    pub(crate) in_flight: Arc<AtomicU32>,
}

/// A single open connection to a Mapepire daemon.
///
/// `Job` is `!Clone` (the underlying dispatcher is exclusive to one
/// `Job`). Use a connection pool — added in v0.3 — to share work
/// across multiple connections.
pub struct Job {
    // INVARIANT: `inner` MUST be declared before `_dispatcher`.
    // Rust drops struct fields top-to-bottom in declaration order (RFC 1857).
    // `inner` (handle + ids) must drop first so that the best-effort Exit
    // fire in `Drop for Job` can use the handle before the dispatcher task
    // is aborted. See PRO-409.
    pub(crate) inner: Arc<JobInner>,
    _dispatcher: Dispatcher,
}

impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Job")
            .field("version", &self.inner.version)
            .field("initial_job", &self.inner.initial_job)
            .finish_non_exhaustive()
    }
}

impl Job {
    /// Open a new connection to the Mapepire daemon described by
    /// `server`. Performs the full TCP → TLS → WebSocket Upgrade →
    /// `Connect` handshake.
    ///
    /// # Errors
    ///
    /// - [`Error::Transport`] for TCP/TLS/WebSocket failures.
    /// - [`Error::Auth`] if the daemon rejects the credentials.
    /// - [`Error::Protocol`] if the daemon's response shape is unexpected.
    /// - [`Error::Internal`] for unrecoverable construction or WebSocket-upgrade failures during
    ///   the handshake.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Job, TlsConfig};
    /// # async fn example() -> mapepire::Result<()> {
    /// let server = DaemonServer::builder()
    ///     .host("ibmi.example.com")
    ///     .user("MYUSER")
    ///     .password("s3cret".to_string())
    ///     .tls(TlsConfig::Verified)
    ///     .build()
    ///     .expect("missing required field");
    ///
    /// let job = Job::connect(&server).await?;
    /// // `version()` is often empty on a live connect; use `server_version()`.
    /// println!("connected: {} ({})", job.version(), job.initial_job());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect(server: &DaemonServer) -> crate::Result<Self> {
        let ConnectedDispatcher {
            dispatcher,
            version,
            initial_job,
            ids,
            in_flight,
        } = transport::connect(server).await?;
        let handle = dispatcher.handle();
        Ok(Self {
            inner: Arc::new(JobInner {
                handle,
                ids: Arc::new(ids),
                version,
                initial_job,
                in_flight,
            }),
            _dispatcher: dispatcher,
        })
    }

    /// Daemon-reported version string from the `Connected` response.
    ///
    /// Live Mapepire daemons often omit `version` on connect, so this may
    /// be empty. Call [`Job::server_version`] (the `getversion` operation)
    /// for the daemon version string.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.inner.version
    }

    /// Initial Db2 job name from the `Connected` response.
    #[must_use]
    pub fn initial_job(&self) -> &str {
        &self.inner.initial_job
    }

    /// Send a request through the dispatcher and await the response.
    /// Internal helper — public methods build the appropriate `Request`
    /// variant and call this.
    pub(crate) async fn send(&self, request: Request) -> crate::Result<Response> {
        self.inner.handle.send(request).await
    }

    /// Return the [`IdAllocator`] shared by this connection.
    ///
    /// [`crate::Query`] clones this `Arc` at [`Job::prepare`] so execute
    /// paths do not take an `ids` argument. Crate-visible for tests and
    /// pool internals; not part of the 1.0 public surface.
    #[must_use]
    pub(crate) fn ids(&self) -> &Arc<IdAllocator> {
        &self.inner.ids
    }

    /// Crate-private accessor for the dispatcher handle (used by
    /// `Rows::stream` to issue follow-up `sqlmore`/`sqlclose`).
    // NOTE: unused until Task 16 adds `Rows::stream`.
    #[allow(dead_code)]
    pub(crate) fn handle(&self) -> DispatcherHandle {
        self.inner.handle.clone()
    }

    /// In-flight request count. The pool's routing scan in v0.3 §7.3
    /// reads this for least-loaded selection; tests use it to assert
    /// that a fresh-connected `Job` starts at zero.
    #[must_use]
    pub fn in_flight(&self) -> u32 {
        self.inner
            .in_flight
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Execute a SQL statement and return the [`crate::query::Rows`] handle.
    ///
    /// For DML (INSERT/UPDATE/DELETE), `rows.update_count()` returns
    /// `Some(n)` (Task 16). For SELECT, iterate via `rows.stream()` or
    /// materialize via `rows.into_typed::<T>()` / `rows.into_dynamic()`
    /// (Tasks 16-17).
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
    /// let rows = job.execute("SELECT * FROM SYSIBM.SYSDUMMY1").await?;
    /// drop(rows);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(job_id = %self.inner.initial_job, sql = %sql))
    )]
    pub async fn execute(&self, sql: &str) -> crate::Result<crate::query::Rows> {
        self.execute_opts(sql, ExecuteOptions::default()).await
    }

    /// Execute a SQL statement with explicit [`ExecuteOptions`].
    ///
    /// Sends `rows` on the `sql` request. [`ExecuteOptions::rows`] `None`
    /// uses 100 (mapepire-js). `Some(0)` is rejected before send.
    /// [`ExecuteOptions::terse`] `true` sends `terse: true`; `false` omits
    /// the field.
    ///
    /// # Errors
    ///
    /// As [`Job::execute`], plus [`Error::Protocol`] when
    /// [`ExecuteOptions::rows`] is `Some(0)`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, ExecuteOptions, Job, TlsConfig};
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
    ///     .execute_opts(
    ///         "SELECT * FROM CORPDATA.EMPLOYEE",
    ///         ExecuteOptions {
    ///             rows: Some(50),
    ///             terse: false,
    ///         },
    ///     )
    ///     .await?;
    /// drop(rows);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, opts),
            fields(job_id = %self.inner.initial_job, sql = %sql, rows = ?opts.rows)
        )
    )]
    pub async fn execute_opts(
        &self,
        sql: &str,
        opts: ExecuteOptions,
    ) -> crate::Result<crate::query::Rows> {
        #[cfg(feature = "metrics")]
        let start = std::time::Instant::now();
        let result = self.execute_inner(sql, None, opts).await;
        #[cfg(feature = "metrics")]
        record_execute_latency(start);
        result
    }

    /// Execute a parameterized SQL statement.
    ///
    /// Stored-procedure `CALL` uses this path (no separate opcode). OUT /
    /// INOUT values are on [`crate::query::Rows::output_parms`].
    ///
    /// # Errors
    ///
    /// As [`Job::execute`].
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
    /// let rows = job
    ///     .execute_with(
    ///         "SELECT * FROM ORDERS WHERE CUSTNO = ?",
    ///         &[serde_json::json!(42)],
    ///     )
    ///     .await?;
    /// drop(rows);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, params),
            fields(
                job_id = %self.inner.initial_job,
                sql = %sql,
                param_count = params.len(),
            )
        )
    )]
    pub async fn execute_with(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> crate::Result<crate::query::Rows> {
        self.execute_with_opts(sql, params, ExecuteOptions::default())
            .await
    }

    /// Execute a parameterized SQL statement with explicit [`ExecuteOptions`].
    ///
    /// Always sends `rows` on `prepare_sql_execute` so the default path
    /// does not hit SQLSTATE 24000 from an omitted page size.
    ///
    /// # Errors
    ///
    /// As [`Job::execute_with`], plus [`Error::Protocol`] when
    /// [`ExecuteOptions::rows`] is `Some(0)`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, ExecuteOptions, Job, TlsConfig};
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
    ///     .execute_with_opts(
    ///         "SELECT * FROM ORDERS WHERE CUSTNO = ?",
    ///         &[serde_json::json!(42)],
    ///         ExecuteOptions {
    ///             rows: Some(50),
    ///             terse: false,
    ///         },
    ///     )
    ///     .await?;
    /// drop(rows);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, params, opts),
            fields(
                job_id = %self.inner.initial_job,
                sql = %sql,
                param_count = params.len(),
                rows = ?opts.rows,
            )
        )
    )]
    pub async fn execute_with_opts(
        &self,
        sql: &str,
        params: &[serde_json::Value],
        opts: ExecuteOptions,
    ) -> crate::Result<crate::query::Rows> {
        #[cfg(feature = "metrics")]
        let start = std::time::Instant::now();
        let result = self.execute_inner(sql, Some(params.to_vec()), opts).await;
        #[cfg(feature = "metrics")]
        record_execute_latency(start);
        result
    }

    async fn execute_inner(
        &self,
        sql: &str,
        params: Option<Vec<serde_json::Value>>,
        opts: ExecuteOptions,
    ) -> crate::Result<crate::query::Rows> {
        let page_size = opts.resolved_rows()?;
        let terse = opts.terse_on_wire();
        let id = self.inner.ids.next();
        let request = match params {
            None => Request::Sql {
                id: id.clone(),
                sql: sql.to_owned(),
                rows: Some(page_size),
                parameters: None,
                terse,
            },
            Some(params) => Request::PrepareSqlExecute {
                id: id.clone(),
                sql: sql.to_owned(),
                parameters: Some(vec![params]),
                rows: Some(page_size),
                terse,
            },
        };
        let resp = self.send(request).await?;
        match resp {
            Response::QueryResult(q) if q.id == id => Ok(crate::query::Rows::new(
                q,
                self.inner.handle.clone(),
                page_size,
            )),
            Response::Error(e) => Err(crate::job_helpers::server_error(e)),
            ref other => Err(crate::job_helpers::unexpected(other)),
        }
    }

    /// Execute every parameter set in **one** `prepare_sql_execute`.
    ///
    /// JS `addToBatch` / 2-D `parameters`. Two or more sets serialize
    /// nested (`[[1,"a"],[2,"b"]]`). A single set still flattens (`[7]`,
    /// same as [`Job::execute_with`]). [`crate::Query::execute_batch`]
    /// stays sequential — this is the one-shot path.
    ///
    /// An empty outer list is [`Error::Protocol`]
    /// ([`crate::ProtocolError::EmptyParameterSets`]) and is not sent.
    /// Same [`ExecuteOptions`] `rows` / `terse` as [`Job::execute_with_opts`].
    ///
    /// # Errors
    ///
    /// As [`Job::execute_with_opts`], plus [`Error::Protocol`] when `sets`
    /// is empty.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, ExecuteOptions, Job, TlsConfig};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password(String::from("test") + "-only")
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let job = Job::connect(&server).await?;
    /// let sets = vec![
    ///     vec![serde_json::json!(1), serde_json::json!("a")],
    ///     vec![serde_json::json!(2), serde_json::json!("b")],
    /// ];
    /// let rows = job
    ///     .execute_sets(
    ///         "INSERT INTO T VALUES(?,?)",
    ///         &sets,
    ///         ExecuteOptions::default(),
    ///     )
    ///     .await?;
    /// assert_eq!(rows.update_count(), Some(2));
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, sets, opts),
            fields(
                job_id = %self.inner.initial_job,
                sql = %sql,
                set_count = sets.len(),
                rows = ?opts.rows,
            )
        )
    )]
    pub async fn execute_sets(
        &self,
        sql: &str,
        sets: &[Vec<serde_json::Value>],
        opts: ExecuteOptions,
    ) -> crate::Result<crate::query::Rows> {
        let parameters = crate::query::owned_parameter_sets(sets)?;
        #[cfg(feature = "metrics")]
        let start = std::time::Instant::now();
        let page_size = opts.resolved_rows()?;
        let terse = opts.terse_on_wire();
        let id = self.inner.ids.next();
        let request = Request::PrepareSqlExecute {
            id: id.clone(),
            sql: sql.to_owned(),
            parameters: Some(parameters),
            rows: Some(page_size),
            terse,
        };
        let resp = self.send(request).await?;
        let result = match resp {
            Response::QueryResult(q) if q.id == id => Ok(crate::query::Rows::new(
                q,
                self.inner.handle.clone(),
                page_size,
            )),
            Response::Error(e) => Err(crate::job_helpers::server_error(e)),
            ref other => Err(crate::job_helpers::unexpected(other)),
        };
        #[cfg(feature = "metrics")]
        record_execute_latency(start);
        result
    }

    /// Prepare a SQL statement for repeated execution.
    ///
    /// Live daemons often reply `{id, success:true}` with no `cont_id`.
    /// That is not a failure: the returned [`crate::query::Query`] caches
    /// `sql` on the client and [`crate::query::Query::execute_with`] sends
    /// `prepare_sql_execute`. When the daemon does return a `cont_id`,
    /// execute uses that handle.
    ///
    /// # Errors
    ///
    /// As [`Job::execute`].
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
    /// drop(query);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(job_id = %self.inner.initial_job, sql = %sql))
    )]
    pub async fn prepare(&self, sql: &str) -> crate::Result<crate::query::Query> {
        let id = self.inner.ids.next();
        let resp = self
            .send(Request::PrepareSql {
                id: id.clone(),
                sql: sql.to_owned(),
                terse: None,
            })
            .await?;
        match resp {
            Response::PreparedStatement {
                id: got, cont_id, ..
            } if got == id => {
                let handle = if cont_id.is_empty() {
                    None
                } else {
                    Some(cont_id)
                };
                Ok(crate::query::Query::new(
                    handle,
                    sql.to_owned(),
                    self.inner.handle.clone(),
                    Arc::clone(self.ids()),
                ))
            }
            // Dispatcher remaps PrepareSql + Pong to PreparedStatement with
            // an empty handle; keep Pong as a fallback so a missed remap
            // still succeeds as a client-side Query.
            Response::Pong { id: got } if got == id => Ok(crate::query::Query::new(
                None,
                sql.to_owned(),
                self.inner.handle.clone(),
                Arc::clone(self.ids()),
            )),
            Response::Error(e) => Err(crate::job_helpers::server_error(e)),
            ref other => Err(crate::job_helpers::unexpected(other)),
        }
    }

    /// Round-trip a `ping` to the daemon. Returns the ping RTT.
    ///
    /// The RTT is measured from just before the request is handed to the
    /// dispatcher through to the moment the response is received. It
    /// therefore includes serialization, async-channel enqueue, socket
    /// write, server processing, socket read, deserialization, and
    /// oneshot delivery — appropriate for a health-check heartbeat, but
    /// not a low-level network latency measurement.
    ///
    /// # Errors
    ///
    /// [`Error::Transport`] if the socket is closed; [`Error::Protocol`]
    /// if the response shape is unexpected.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(job_id = %self.inner.initial_job))
    )]
    pub async fn ping(&self) -> crate::Result<std::time::Duration> {
        let id = self.inner.ids.next();
        let start = std::time::Instant::now();
        let resp = self.send(Request::Ping { id: id.clone() }).await?;
        match resp {
            Response::Pong { id: got } if got == id => Ok(start.elapsed()),
            ref other => Err(crate::job_helpers::unexpected(other)),
        }
    }

    /// Retrieve the daemon's reported version string.
    ///
    /// Sends `getversion`. Prefer this over [`Job::version`], which is
    /// filled from the connect response and is often empty on a live daemon.
    ///
    /// # Errors
    ///
    /// As [`Job::ping`], plus [`Error::Server`] if the daemon's response
    /// carries `success: false`.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(job_id = %self.inner.initial_job))
    )]
    pub async fn server_version(&self) -> crate::Result<String> {
        let id = self.inner.ids.next();
        let resp = self.send(Request::GetVersion { id: id.clone() }).await?;
        match resp {
            Response::Version {
                id: got,
                success,
                version,
                ..
            } if got == id => {
                if success {
                    Ok(version)
                } else {
                    Err(crate::job_helpers::server_failed("server_version"))
                }
            }
            ref other => Err(crate::job_helpers::unexpected(other)),
        }
    }

    /// Retrieve the current Db2 job name on the daemon.
    ///
    /// # Errors
    ///
    /// As [`Job::ping`], plus [`Error::Server`] if the daemon's response
    /// carries `success: false`.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(job_id = %self.inner.initial_job))
    )]
    pub async fn db_job_name(&self) -> crate::Result<String> {
        let id = self.inner.ids.next();
        let resp = self.send(Request::GetDbJob { id: id.clone() }).await?;
        match resp {
            Response::DbJob {
                id: got,
                success,
                job,
                ..
            } if got == id => {
                if success {
                    Ok(job)
                } else {
                    Err(crate::job_helpers::server_failed("db_job_name"))
                }
            }
            ref other => Err(crate::job_helpers::unexpected(other)),
        }
    }

    /// Configure the daemon's trace level via `setconfig`.
    ///
    /// Destination is [`TraceDest::InMem`] so [`Job::fetch_trace`] can
    /// read the buffer. Use [`Job::set_trace_config`] to write a
    /// server-side [`TraceDest::File`]. Never sends `tracedest: ""`.
    ///
    /// # Errors
    ///
    /// As [`Job::ping`], plus [`Error::Server`] if the daemon's
    /// response carries `success: false`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Job, TlsConfig, TraceLevel};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let job = Job::connect(&server).await?;
    /// job.set_trace(TraceLevel::Errors).await?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(job_id = %self.inner.initial_job, dest = "IN_MEM", level = ?level))
    )]
    pub async fn set_trace(&self, level: TraceLevel) -> crate::Result<()> {
        self.apply_trace(TraceDest::InMem, level).await
    }

    /// Configure daemon tracing with an explicit destination (JS
    /// `setTraceConfig`).
    ///
    /// [`TraceDest::InMem`] buffers for [`Job::fetch_trace`];
    /// [`TraceDest::File`] writes a server-side path. Never sends
    /// `tracedest: ""`.
    ///
    /// # Errors
    ///
    /// As [`Job::set_trace`].
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Job, TlsConfig, TraceDest, TraceLevel};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let job = Job::connect(&server).await?;
    /// job.set_trace_config(TraceDest::File, TraceLevel::Datastream)
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(job_id = %self.inner.initial_job, dest = ?dest, level = ?level))
    )]
    pub async fn set_trace_config(&self, dest: TraceDest, level: TraceLevel) -> crate::Result<()> {
        self.apply_trace(dest, level).await
    }

    async fn apply_trace(&self, dest: TraceDest, level: TraceLevel) -> crate::Result<()> {
        let id = self.inner.ids.next();
        let resp = self
            .send(Request::SetConfig {
                id: id.clone(),
                tracelevel: level.as_str().to_owned(),
                tracedest: dest.as_str().to_owned(),
            })
            .await?;
        match resp {
            Response::ConfigSet {
                id: got, success, ..
            } if got == id => {
                if success {
                    Ok(())
                } else {
                    Err(crate::job_helpers::server_failed("set_trace"))
                }
            }
            ref other => Err(crate::job_helpers::unexpected(other)),
        }
    }

    /// Fetch the daemon's accumulated trace data as raw text.
    ///
    /// Returns whatever the daemon has buffered since the last
    /// [`Job::set_trace`] / [`Job::set_trace_config`] call. Only populated
    /// when dest is [`TraceDest::InMem`] (the [`Job::set_trace`] default).
    ///
    /// # Errors
    ///
    /// As [`Job::ping`], plus [`crate::Error::Server`] if the daemon's
    /// response carries `success: false`.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Job, TlsConfig, TraceLevel};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let job = Job::connect(&server).await?;
    /// job.set_trace(TraceLevel::Errors).await?;
    /// // ... run some failing SQL ...
    /// let trace = job.fetch_trace().await?;
    /// println!("trace ({} bytes)", trace.len());
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(job_id = %self.inner.initial_job))
    )]
    pub async fn fetch_trace(&self) -> crate::Result<String> {
        let id = self.inner.ids.next();
        let resp = self.send(Request::GetTraceData { id: id.clone() }).await?;
        match resp {
            Response::TraceData {
                id: got,
                success,
                tracedata,
            } if got == id => {
                if success {
                    Ok(tracedata)
                } else {
                    Err(crate::job_helpers::server_failed("fetch_trace"))
                }
            }
            ref other => Err(crate::job_helpers::unexpected(other)),
        }
    }

    /// Run a daemon-side `visual_explain` (the `dove` op) on a SQL statement.
    ///
    /// Sends `run: true` to match mapepire-js `SQLJob.explain()` /
    /// `ExplainType.RUN`. Returns live `vedata` (the explain tree).
    /// SQLSTATE **42505** (no Visual Explain authority) is
    /// [`crate::Error::Server`], not a crate protocol failure.
    ///
    /// # Errors
    ///
    /// As [`Job::execute`], plus [`crate::Error::Server`] if the daemon's
    /// response carries `success: false` (including 42505).
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
    /// let plan = job
    ///     .visual_explain("SELECT * FROM CORPDATA.EMPLOYEE WHERE SALARY > 50000")
    ///     .await?;
    /// // `plan` is opaque JSON — daemon-defined shape.
    /// println!("plan: {plan:#}");
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(job_id = %self.inner.initial_job, sql = %sql))
    )]
    pub async fn visual_explain(&self, sql: &str) -> crate::Result<serde_json::Value> {
        let id = self.inner.ids.next();
        let resp = self
            .send(Request::Dove {
                id: id.clone(),
                sql: sql.to_owned(),
                run: Some(true),
                rows: None,
                terse: None,
            })
            .await?;
        match resp {
            Response::DoveResult {
                id: got,
                success,
                vedata,
                ..
            } if got == id => {
                if success {
                    Ok(vedata)
                } else {
                    Err(crate::job_helpers::server_failed("visual_explain"))
                }
            }
            Response::Error(e) => Err(crate::job_helpers::server_error(e)),
            ref other => Err(crate::job_helpers::unexpected(other)),
        }
    }

    /// Run an IBM i CL command.
    ///
    /// Live daemons reply with an untagged [`QueryResult`] whose `data` is
    /// the job log. Failed commands (`CPF0006`, `sql_rc = -443`) still
    /// return [`Ok`] with [`ClOutcome::success`] = `false` and the log in
    /// [`ClOutcome::entries`] — matching mapepire-js `SQLJob.clcommand`,
    /// which does not throw.
    ///
    /// Tagged `cl_result` mock frames are still accepted and mapped onto
    /// the same [`ClOutcome`].
    ///
    /// # Errors
    ///
    /// As [`Job::execute`] for transport / protocol failures, plus
    /// [`Error::Server`] for a bare `success: false` frame with no `data`
    /// (not a job-log reply).
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
    /// let outcome = job.cl("DSPLIB MYLIB").await?;
    /// if outcome.success {
    ///     for entry in &outcome.entries {
    ///         if let Some(text) = &entry.message_text {
    ///             println!("CL message: {text}");
    ///         }
    ///     }
    /// } else {
    ///     println!("CL failed: {:?}", outcome.error);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self), fields(job_id = %self.inner.initial_job, command = %command))
    )]
    pub async fn cl(&self, command: &str) -> crate::Result<ClOutcome> {
        let id = self.inner.ids.next();
        let resp = self
            .send(Request::Cl {
                id: id.clone(),
                cmd: command.to_owned(),
                terse: None,
            })
            .await?;
        cl_outcome_from_response(&id, resp)
    }
}

/// Outcome of [`Job::cl`].
///
/// Failed CL is `success: false` with the job log still in `entries`.
/// That is not a [`crate::Error::Server`].
///
/// # Example
///
/// ```
/// use mapepire::ClOutcome;
///
/// let failed = ClOutcome {
///     success: false,
///     error: Some("[CPF0006] Errors occurred in command.".into()),
///     sqlcode: Some(-443),
///     sqlstate: Some("38501".into()),
///     entries: vec![],
/// };
/// assert!(!failed.success);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClOutcome {
    /// `true` when the command completed without an escape message.
    pub success: bool,
    /// Daemon `error` text, when present (typically the CPF0006 line).
    pub error: Option<String>,
    /// Db2 SQL code (`sql_rc` on the wire). `-443` for a CL escape.
    pub sqlcode: Option<i32>,
    /// SQLSTATE (`sql_state` on the wire). `38501` for a CL escape.
    pub sqlstate: Option<String>,
    /// Full job log, one row per `QueryResult.data` object.
    pub entries: Vec<JobLogEntry>,
}

impl ClOutcome {
    fn from_query_result(q: QueryResult) -> Self {
        Self {
            success: q.success,
            error: q.error,
            sqlcode: q.sqlcode,
            sqlstate: q.sqlstate,
            entries: q
                .data
                .into_iter()
                .map(|row| {
                    serde_json::from_value(serde_json::Value::Object(row)).unwrap_or_default()
                })
                .collect(),
        }
    }

    fn from_cl_result(success: bool, messages: Vec<ClMessage>) -> Self {
        let error = if success {
            None
        } else {
            messages.iter().find_map(|m| m.text.clone())
        };
        Self {
            success,
            error,
            sqlcode: None,
            sqlstate: None,
            entries: messages
                .into_iter()
                .map(|m| JobLogEntry {
                    message_id: m.id,
                    message_type: m.kind,
                    message_text: m.text,
                    ..JobLogEntry::default()
                })
                .collect(),
        }
    }
}

fn cl_outcome_from_response(expected_id: &str, resp: Response) -> Result<ClOutcome, Error> {
    match resp {
        Response::QueryResult(q) if q.id == expected_id => Ok(ClOutcome::from_query_result(q)),
        Response::ClResult {
            id: got,
            success,
            messages,
        } if got == expected_id => Ok(ClOutcome::from_cl_result(success, messages)),
        Response::Error(e) => Err(crate::job_helpers::server_error(e)),
        ref other => Err(crate::job_helpers::unexpected(other)),
    }
}

/// Record the elapsed time since `start` to the
/// [`JOB_EXECUTE_LATENCY_MICROS`] histogram in microseconds.
///
/// Saturates at `u64::MAX` µs (~584 942 years) before the f64 cast so we
/// never panic on a pathologically huge elapsed; the cast itself is safe
/// for any realistic value (< 2^53 µs ≈ 285 years).
///
/// [`JOB_EXECUTE_LATENCY_MICROS`]: crate::observability::JOB_EXECUTE_LATENCY_MICROS
#[cfg(feature = "metrics")]
fn record_execute_latency(start: std::time::Instant) {
    let elapsed_micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
    #[allow(clippy::cast_precision_loss)]
    let micros_f64 = elapsed_micros as f64;
    metrics::histogram!(crate::observability::JOB_EXECUTE_LATENCY_MICROS).record(micros_f64);
}

impl Drop for Job {
    fn drop(&mut self) {
        // Best-effort exit. We can't await in Drop, so spawn a fire-and-
        // forget task. The dispatcher will be aborted by its own Drop on
        // the `_dispatcher` field immediately after this fn returns; the
        // Exit may or may not get through depending on the runtime's task
        // schedule.
        //
        // See `spawn_best_effort` for runtime-guard rationale.
        let handle = self.inner.handle.clone();
        let id = self.inner.ids.next();
        crate::job_helpers::spawn_best_effort(async move {
            let _ = handle.send(Request::Exit { id }).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::protocol::ErrorResponse;

    fn job_log_query_result(id: &str, success: bool) -> QueryResult {
        let mut row = serde_json::Map::new();
        row.insert("MESSAGE_ID".into(), serde_json::json!("CPF0006"));
        row.insert("SEVERITY".into(), serde_json::json!(40));
        row.insert(
            "MESSAGE_TEXT".into(),
            serde_json::json!("[CPF0006] Errors occurred in command."),
        );
        QueryResult {
            id: id.into(),
            success,
            has_results: true,
            update_count: -1,
            cont_id: None,
            is_done: true,
            metadata: crate::protocol::QueryMetaData::default(),
            data: vec![row],
            execution_time: 1.0,
            error: if success {
                None
            } else {
                Some("[CPF0006] Errors occurred in command.".into())
            },
            sqlcode: if success { None } else { Some(-443) },
            sqlstate: if success { None } else { Some("38501".into()) },
            parameter_count: None,
            output_parms: vec![],
        }
    }

    #[test]
    fn test_cl_outcome_from_query_result_failure_keeps_job_log() {
        let q = job_log_query_result("cl1", false);
        let outcome = cl_outcome_from_response("cl1", Response::QueryResult(q)).unwrap();
        assert!(!outcome.success);
        assert_eq!(outcome.sqlcode, Some(-443));
        assert_eq!(outcome.sqlstate.as_deref(), Some("38501"));
        assert_eq!(outcome.entries.len(), 1);
        assert_eq!(outcome.entries[0].message_id.as_deref(), Some("CPF0006"));
        assert_eq!(outcome.entries[0].severity.as_deref(), Some("40"));
    }

    #[test]
    fn test_cl_outcome_from_tagged_cl_result() {
        let resp = Response::ClResult {
            id: "cl1".into(),
            success: true,
            messages: vec![ClMessage {
                id: Some("CPC2102".into()),
                kind: Some("COMPLETION".into()),
                text: Some("Library displayed.".into()),
            }],
        };
        let outcome = cl_outcome_from_response("cl1", resp).unwrap();
        assert!(outcome.success);
        assert_eq!(outcome.entries.len(), 1);
        assert_eq!(outcome.entries[0].message_id.as_deref(), Some("CPC2102"));
        assert_eq!(
            outcome.entries[0].message_text.as_deref(),
            Some("Library displayed.")
        );
    }

    #[test]
    fn test_cl_outcome_from_tagged_cl_result_failure_is_ok() {
        let resp = Response::ClResult {
            id: "cl1".into(),
            success: false,
            messages: vec![ClMessage {
                id: Some("CPF0006".into()),
                kind: Some("ESCAPE".into()),
                text: Some("[CPF0006] Errors occurred in command.".into()),
            }],
        };
        let outcome = cl_outcome_from_response("cl1", resp).unwrap();
        assert!(!outcome.success);
        assert_eq!(
            outcome.error.as_deref(),
            Some("[CPF0006] Errors occurred in command.")
        );
        assert_eq!(outcome.entries[0].message_id.as_deref(), Some("CPF0006"));
    }

    #[test]
    fn test_cl_outcome_bare_error_is_err() {
        let resp = Response::Error(ErrorResponse {
            id: "cl1".into(),
            success: false,
            sqlstate: Some("38501".into()),
            sqlcode: Some(-443),
            error: Some("nope".into()),
            job: None,
        });
        let err = cl_outcome_from_response("cl1", resp).unwrap_err();
        assert!(matches!(err, Error::Server(_)));
    }

    #[test]
    fn test_trace_level_wire_strings() {
        assert_eq!(TraceLevel::Off.as_str(), "OFF");
        assert_eq!(TraceLevel::Errors.as_str(), "ERRORS");
        assert_eq!(TraceLevel::Datastream.as_str(), "DATASTREAM");
        assert_eq!(TraceLevel::All.as_str(), "ON");
        assert_ne!(TraceLevel::All.as_str(), "ALL");
    }

    #[test]
    fn test_trace_dest_wire_strings() {
        assert_eq!(TraceDest::File.as_str(), "FILE");
        assert_eq!(TraceDest::InMem.as_str(), "IN_MEM");
        assert_eq!(TraceDest::default(), TraceDest::InMem);
    }

    #[test]
    fn test_set_trace_request_never_sends_empty_dest() {
        let r = Request::SetConfig {
            id: "1".into(),
            tracelevel: TraceLevel::Off.as_str().to_owned(),
            tracedest: TraceDest::InMem.as_str().to_owned(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains(r#""tracedest":"IN_MEM""#));
        assert!(!json.contains(r#""tracedest":"""#));
        let all = Request::SetConfig {
            id: "2".into(),
            tracelevel: TraceLevel::All.as_str().to_owned(),
            tracedest: TraceDest::InMem.as_str().to_owned(),
        };
        let json = serde_json::to_string(&all).unwrap();
        assert!(json.contains(r#""tracelevel":"ON""#));
        assert!(!json.contains(r#""tracelevel":"ALL""#));
    }
}
