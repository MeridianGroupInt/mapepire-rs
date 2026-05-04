//! [`deadpool::managed::Manager`] impl that produces [`crate::Job`]s.
//!
//! Spec §7.1: `create()` calls [`crate::Job::connect`]; `recycle()` (added in
//! Task 7 / PRO-437) runs a ping to verify the connection's liveness because
//! IBM i firewalls silently kill idle TCP sessions.

use std::sync::Arc;

use deadpool::managed::{Manager, Metrics, RecycleResult};

use crate::config::DaemonServer;
use crate::error::Error;
use crate::job::Job;

/// `deadpool::managed::Manager` impl that produces [`crate::Job`]s for the
/// pool runtime. `Type = Arc<Job>` so the routing registry (Task 23) can
/// store `Weak<Job>` references — see plan §7.3.
///
/// **Visibility note:** `pub` so the integration test in
/// `tests/manager_smoke.rs` (Task 8) can construct it directly, but the
/// re-export at `mapepire::pool::JobManager` carries `#[doc(hidden)]` so the
/// type stays out of the rendered rustdoc API surface. External users
/// construct `Pool` via `Pool::builder` (Task 10) — they never need to touch
/// `JobManager`.
pub struct JobManager {
    server: Arc<DaemonServer>,
    registry: Arc<crate::pool::routing::Registry>,
}

impl JobManager {
    /// Build a [`JobManager`] bound to the given [`DaemonServer`] and routing
    /// registry. The manager clones the [`DaemonServer`] `Arc` on each
    /// [`Manager::create`] call, and registers the freshly-spawned `Arc<Job>`
    /// with the shared registry so the §7.3 routing scan can later peek
    /// `in_flight` without taking ownership.
    ///
    /// `pub` for the same reason as the struct itself — see the type-level
    /// doc comment.
    ///
    /// `Registry` is `pub(crate)` (it's a routing-internal type — external
    /// callers have no use for it), but `JobManager::new` is `pub` so that
    /// the `#[doc(hidden)]` re-export stays useful for the `manager_smoke`
    /// integration test. The `private_interfaces` allow narrows the
    /// suppression to this one signature; the `Registry` type stays out of
    /// the rendered API surface.
    #[allow(private_interfaces)]
    #[must_use]
    pub fn new(server: Arc<DaemonServer>, registry: Arc<crate::pool::routing::Registry>) -> Self {
        Self { server, registry }
    }
}

impl Manager for JobManager {
    type Type = Arc<Job>;
    type Error = Error;

    async fn create(&self) -> Result<Arc<Job>, Error> {
        let job = Arc::new(Job::connect(&self.server).await?);
        self.registry.track(&job);
        Ok(job)
    }

    async fn recycle(&self, job: &mut Arc<Job>, _: &Metrics) -> RecycleResult<Error> {
        use deadpool::managed::RecycleError;
        // RecyclingMethod::Verified — round-trip a ping. IBM i firewalls
        // silently kill idle TCP sessions; without this check, the next caller
        // discovers the dead connection mid-execute. Spec §7.1.
        job.ping().await.map(|_| ()).map_err(RecycleError::Backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-only assertion: JobManager satisfies deadpool's Manager trait
    // bounds at the type level. No runtime test here — the live create/recycle
    // path is exercised in tests/manager_smoke.rs (Task 8) against the mock
    // server.
    fn assert_manager<M: Manager>() {}

    #[test]
    fn jobmanager_satisfies_manager_trait() {
        assert_manager::<JobManager>();
    }
}
