//! Public [`Pool`] entry point (spec §4.6).
//!
//! Wraps `deadpool::managed::Pool<JobManager>`. The pool surface itself is
//! intentionally minimal — most users construct via [`Pool::builder`], call
//! `Pool::execute` (Task 11) for one-shot work, or `Pool::acquire`
//! (Task 13) for transactional connections. Routing scan landing in Task 24
//! / PRO-454 will replace the naive checkout.

use std::sync::Arc;
use std::time::Duration;

use deadpool::managed::Pool as DeadPool;

use crate::config::DaemonServer;
use crate::pool::builder::PoolBuilder;
use crate::pool::manager::JobManager;
use crate::pool::routing::Registry;

/// Connection pool for one or more [`crate::Job`] connections to a single
/// Mapepire daemon.
///
/// Construct via [`Pool::builder`]. `Pool` is `Clone` — clones share the
/// same underlying deadpool runtime and registry.
///
/// `Pool::execute` (Task 11) and `Pool::acquire` (Task 13) land in
/// subsequent tasks of v0.3 Phase 4 / Phase 5.
//
// `dead_code` is allowed here because the consumers of these fields —
// `Pool::execute` (Task 11 / PRO-441) and `Pool::acquire` (Task 13 /
// PRO-443) — have not landed yet. The fields are populated by
// `PoolBuilder::build` (this task) and read in subsequent Phase 4 / 5 tasks.
#[allow(dead_code)]
#[derive(Clone)]
pub struct Pool {
    pub(crate) inner: DeadPool<JobManager>,
    pub(crate) registry: Arc<Registry>,
    pub(crate) acquire_timeout: Option<Duration>,
}

impl Pool {
    /// Begin building a `Pool` from a [`DaemonServer`] (or `Arc<DaemonServer>`).
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use mapepire::{DaemonServer, Pool, TlsConfig};
    /// # async fn example() -> mapepire::Result<()> {
    /// # let server = DaemonServer::builder()
    /// #     .host("ibmi.example.com")
    /// #     .user("MYUSER")
    /// #     .password("s3cret".to_string())
    /// #     .tls(TlsConfig::Verified)
    /// #     .build()
    /// #     .expect("missing required field");
    /// let pool = Pool::builder(server).max_size(8).build().await?;
    /// # let _ = pool;
    /// # Ok(())
    /// # }
    /// ```
    pub fn builder(server: impl Into<Arc<DaemonServer>>) -> PoolBuilder {
        PoolBuilder::new(server.into())
    }
}
