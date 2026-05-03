//! Connection pool for [`crate::Job`] (v0.3 §4.6, §7).
//!
//! `Pool` (added in Task 10) wraps `deadpool::managed::Pool<JobManager>`.
//! Construct via `Pool::builder`. Acquire a transactional connection via
//! `Pool::acquire` (returns `Reserved`, added in Task 13).

pub(crate) mod manager;

// Future siblings (added in subsequent tasks):
// pub(crate) mod builder;
// pub(crate) mod pool;
// pub(crate) mod reserved;
// pub(crate) mod routing;

#[allow(unused_imports)]
pub(crate) use manager::JobManager;
