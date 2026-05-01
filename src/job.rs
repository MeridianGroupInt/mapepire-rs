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
    pub async fn connect(server: &DaemonServer) -> Result<Self, Error> {
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
    pub(crate) async fn send(&self, request: Request) -> Result<Response, Error> {
        self.handle.send(request).await
    }

    /// Crate-private accessor for the `IdAllocator` (used by `Query` to
    /// stamp ids on `execute` / `sqlmore` / `sqlclose` requests).
    // NOTE: unused until the SQL tasks (Task 13+) are added.
    #[allow(dead_code)]
    pub(crate) fn ids(&self) -> &IdAllocator {
        &self.ids
    }

    /// Crate-private accessor for the dispatcher handle (used by
    /// `Rows::stream` to issue follow-up `sqlmore`/`sqlclose`).
    // NOTE: unused until Task 16 adds `Rows::stream`.
    #[allow(dead_code)]
    pub(crate) fn handle(&self) -> DispatcherHandle {
        self.handle.clone()
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
    pub async fn ping(&self) -> Result<std::time::Duration, Error> {
        let id = self.ids.next();
        let start = std::time::Instant::now();
        let resp = self.send(Request::Ping { id: id.clone() }).await?;
        match resp {
            Response::Pong { id: got } if got == id => Ok(start.elapsed()),
            ref other => Err(unexpected(other)),
        }
    }

    /// Retrieve the daemon's reported version string.
    ///
    /// # Errors
    ///
    /// As [`Job::ping`], plus [`Error::Server`] if the daemon's response
    /// carries `success: false`.
    pub async fn server_version(&self) -> Result<String, Error> {
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
                    Err(server_failed("server_version"))
                }
            }
            ref other => Err(unexpected(other)),
        }
    }

    /// Retrieve the current Db2 job name on the daemon.
    ///
    /// # Errors
    ///
    /// As [`Job::ping`], plus [`Error::Server`] if the daemon's response
    /// carries `success: false`.
    pub async fn db_job_name(&self) -> Result<String, Error> {
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
                    Err(server_failed("db_job_name"))
                }
            }
            ref other => Err(unexpected(other)),
        }
    }
}

fn unexpected(response: &Response) -> Error {
    use crate::error::ProtocolError;
    Error::from(ProtocolError::UnknownResponseType(format!(
        "unexpected variant: {response:?}"
    )))
}

fn server_failed(method: &str) -> Error {
    use crate::error::ServerError;
    Error::from(ServerError {
        message: format!("daemon returned success=false for {method}"),
        sqlstate: None,
        sqlcode: None,
        job_name: None,
        diagnostics: vec![],
    })
}
