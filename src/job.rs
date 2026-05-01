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
    // NOTE: `handle` and `ids` are unused until Task 9 adds `Job::ping`,
    // `Job::execute`, etc. The `allow` keeps `-D warnings` clean on this branch.
    #[allow(dead_code)]
    handle: DispatcherHandle,
    #[allow(dead_code)]
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
    // NOTE: unused until Task 9 adds callers; allow keeps `-D warnings` clean.
    #[allow(dead_code)]
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
}
