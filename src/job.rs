//! Single connection to a Mapepire daemon.
//!
//! [`Job`] wraps a per-connection dispatcher task. Construct via
//! [`Job::connect`]. Drop runs a best-effort `exit` to let the daemon
//! shut down cleanly.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use crate::config::DaemonServer;
use crate::error::Error;
use crate::protocol::{IdAllocator, Request, Response};
use crate::transport::{self, ConnectedDispatcher, Dispatcher, DispatcherHandle};

/// Trace level for the daemon. Maps to the `setconfig.tracelevel` key.
///
/// The daemon accepts opaque strings; this enum pins the documented set
/// from the v0.2 wire-protocol notes. Use [`Job::set_trace`] to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceLevel {
    /// No tracing.
    Off,
    /// Errors only.
    Errors,
    /// Errors + statement boundaries.
    Datastream,
    /// Full diagnostic (high overhead — use sparingly).
    All,
}

impl TraceLevel {
    fn as_str(self) -> &'static str {
        match self {
            TraceLevel::Off => "OFF",
            TraceLevel::Errors => "ERRORS",
            TraceLevel::Datastream => "DATASTREAM",
            TraceLevel::All => "ALL",
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
    /// Consumers pass this to [`crate::Query::execute`] /
    /// [`crate::Query::execute_with`] / [`crate::Query::execute_batch`] so
    /// that correlation ids are unique across all requests on the same `Job`.
    #[must_use]
    pub fn ids(&self) -> &IdAllocator {
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
    pub async fn execute(&self, sql: &str) -> crate::Result<crate::query::Rows> {
        self.execute_inner(sql, None).await
    }

    /// Execute a parameterized SQL statement.
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
    pub async fn execute_with(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> crate::Result<crate::query::Rows> {
        self.execute_inner(sql, Some(params.to_vec())).await
    }

    async fn execute_inner(
        &self,
        sql: &str,
        params: Option<Vec<serde_json::Value>>,
    ) -> crate::Result<crate::query::Rows> {
        let id = self.inner.ids.next();
        let request = Request::Sql {
            id: id.clone(),
            sql: sql.to_owned(),
            rows: None,
            parameters: params,
        };
        let resp = self.send(request).await?;
        match resp {
            Response::QueryResult(q) if q.id == id => {
                Ok(crate::query::Rows::new(q, self.inner.handle.clone()))
            }
            Response::Error(e) => Err(crate::job_helpers::server_error(e)),
            ref other => Err(crate::job_helpers::unexpected(other)),
        }
    }

    /// Prepare a SQL statement for repeated execution.
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
    pub async fn prepare(&self, sql: &str) -> crate::Result<crate::query::Query> {
        let id = self.inner.ids.next();
        let resp = self
            .send(Request::PrepareSql {
                id: id.clone(),
                sql: sql.to_owned(),
            })
            .await?;
        match resp {
            Response::PreparedStatement {
                id: got, cont_id, ..
            } if got == id => Ok(crate::query::Query::new(cont_id, self.inner.handle.clone())),
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
    /// # Errors
    ///
    /// As [`Job::ping`], plus [`Error::Server`] if the daemon's response
    /// carries `success: false`.
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
    /// Sets `tracelevel` to the enum's string representation; `tracedest`
    /// is left empty (server uses its default destination).
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
    pub async fn set_trace(&self, level: TraceLevel) -> crate::Result<()> {
        let id = self.inner.ids.next();
        // `tracedest: String::new()` — empty string asks the daemon to use
        // its default trace destination (no override).
        let resp = self
            .send(Request::SetConfig {
                id: id.clone(),
                tracelevel: level.as_str().to_owned(),
                tracedest: String::new(),
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
    /// [`Job::set_trace`] call — typically driver-side trace records, format
    /// is daemon-defined.
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

    /// Run an IBM i CL command.
    ///
    /// Returns the first [`crate::protocol::ClMessage`] from the daemon's
    /// response. The full message list surfaces in a future v0.3+ typed
    /// `CommandResult`; for v0.2 this is a best-effort single-message view.
    ///
    /// # Errors
    ///
    /// As [`Job::execute`], plus [`Error::Server`] if the daemon returns
    /// `success: false`, or [`Error::Internal`] if the daemon returns an
    /// empty message list despite `success: true`.
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
    /// // DSPLIB emits a CPF2102 completion message — a single ClMessage.
    /// let msg = job.cl("DSPLIB MYLIB").await?;
    /// if let Some(text) = msg.text {
    ///     println!("CL message: {text}");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn cl(&self, command: &str) -> crate::Result<crate::protocol::ClMessage> {
        let id = self.inner.ids.next();
        let resp = self
            .send(Request::Cl {
                id: id.clone(),
                cmd: command.to_owned(),
            })
            .await?;
        match resp {
            Response::ClResult {
                id: got,
                success,
                messages,
                ..
            } if got == id => {
                if !success {
                    return Err(crate::job_helpers::server_failed("cl"));
                }
                // Return the first message; the full message list surfaces
                // in a future v0.3+ typed CommandResult (v0.2 limitation).
                messages.into_iter().next().ok_or_else(|| {
                    Error::Internal("daemon returned ClResult with no messages".to_string())
                })
            }
            Response::Error(e) => Err(crate::job_helpers::server_error(e)),
            ref other => Err(crate::job_helpers::unexpected(other)),
        }
    }
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
