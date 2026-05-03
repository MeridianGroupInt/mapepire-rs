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
//
// `dead_code` is silenced because `JobManager::new` has no caller until
// Task 10 (`Pool::builder().build()`) wires the manager into a deadpool
// `Pool`. The `#[cfg(test)]` trait-bound assertion in this file references
// the type but doesn't construct it. Remove this attribute when Task 10
// lands.
#[allow(dead_code)]
pub(crate) struct JobManager {
    server: Arc<DaemonServer>,
}

#[allow(dead_code)]
impl JobManager {
    pub(crate) fn new(server: Arc<DaemonServer>) -> Self {
        Self { server }
    }
}

impl Manager for JobManager {
    type Type = Arc<Job>;
    type Error = Error;

    async fn create(&self) -> Result<Arc<Job>, Error> {
        let job = Job::connect(&self.server).await?;
        Ok(Arc::new(job))
    }

    async fn recycle(&self, _job: &mut Arc<Job>, _: &Metrics) -> RecycleResult<Error> {
        // Real impl in Task 7 (PRO-437) — round-trip a ping per
        // RecyclingMethod::Verified.
        Ok(())
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
