//! Single connection to a Mapepire daemon.
//!
//! [`Job`] wraps a per-connection dispatcher task. Construct via
//! [`Job::connect`]. Drop runs a best-effort `exit` to let the daemon
//! shut down cleanly.

use std::fmt;
use std::sync::Arc;

use crate::config::DaemonServer;
use crate::error::Error;
use crate::protocol::{IdAllocator, Request, Response};
use crate::transport::{self, ConnectedDispatcher, Dispatcher, DispatcherHandle};

/// A single open connection to a Mapepire daemon.
///
/// `Job` is `!Clone` (the underlying dispatcher is exclusive to one
/// `Job`). Use a connection pool — added in v0.3 — to share work
/// across multiple connections.
pub struct Job {
    handle: DispatcherHandle,
    ids: Arc<IdAllocator>,
    /// Daemon-reported version string from the `Connected` response.
    pub version: String,
    /// Initial Db2 job name from the `Connected` response.
    pub initial_job: String,
    // Hold the Dispatcher so dropping the Job aborts the spawned task.
    // Must be declared last so it drops after `handle` and `ids`.
    _dispatcher: Dispatcher,
}

impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Job")
            .field("version", &self.version)
            .field("initial_job", &self.initial_job)
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
    /// println!("connected: {} ({})", job.version, job.initial_job);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn connect(server: &DaemonServer) -> crate::Result<Self> {
        let ConnectedDispatcher {
            dispatcher,
            version,
            initial_job,
            ids,
        } = transport::connect(server).await?;
        let handle = dispatcher.handle();
        Ok(Self {
            handle,
            ids: Arc::new(ids),
            version,
            initial_job,
            _dispatcher: dispatcher,
        })
    }

    /// Send a request through the dispatcher and await the response.
    /// Internal helper — public methods build the appropriate `Request`
    /// variant and call this.
    pub(crate) async fn send(&self, request: Request) -> crate::Result<Response> {
        self.handle.send(request).await
    }

    /// Return the [`IdAllocator`] shared by this connection.
    ///
    /// Consumers pass this to [`crate::Query::execute`] /
    /// [`crate::Query::execute_with`] / [`crate::Query::execute_batch`] so
    /// that correlation ids are unique across all requests on the same `Job`.
    #[must_use]
    pub fn ids(&self) -> &IdAllocator {
        &self.ids
    }

    /// Crate-private accessor for the dispatcher handle (used by
    /// `Rows::stream` to issue follow-up `sqlmore`/`sqlclose`).
    // NOTE: unused until Task 16 adds `Rows::stream`.
    #[allow(dead_code)]
    pub(crate) fn handle(&self) -> DispatcherHandle {
        self.handle.clone()
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
        let id = self.ids.next();
        let request = Request::Sql {
            id: id.clone(),
            sql: sql.to_owned(),
            rows: None,
            parameters: params,
        };
        let resp = self.send(request).await?;
        match resp {
            Response::QueryResult(q) if q.id == id => {
                Ok(crate::query::Rows::new(q, self.handle.clone()))
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
        let id = self.ids.next();
        let resp = self
            .send(Request::PrepareSql {
                id: id.clone(),
                sql: sql.to_owned(),
            })
            .await?;
        match resp {
            Response::PreparedStatement {
                id: got, cont_id, ..
            } if got == id => Ok(crate::query::Query::new(cont_id, self.handle.clone())),
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
        let id = self.ids.next();
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
        let id = self.ids.next();
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
        let id = self.ids.next();
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
        let id = self.ids.next();
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
        let handle = self.handle.clone();
        let id = self.ids.next();
        crate::job_helpers::spawn_best_effort(async move {
            let _ = handle.send(Request::Exit { id }).await;
        });
    }
}
