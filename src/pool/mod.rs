//! Connection pool for [`crate::Job`] (v0.3 §4.6, §7).
//!
//! `Pool` (added in Task 10) wraps `deadpool::managed::Pool<JobManager>`.
//! Construct via `Pool::builder`. Acquire a transactional connection via
//! `Pool::acquire` (returns `Reserved`, added in Task 13).

pub(crate) mod builder;
pub(crate) mod manager;
// `pool::pool` mirrors the spec layout (§4.6 names the type-bearing module
// `pool`). External callers see `crate::Pool` via the re-export below.
#[allow(clippy::module_inception)]
pub(crate) mod pool;
pub(crate) mod routing;

// Future siblings (added in subsequent tasks):
// pub(crate) mod reserved;

pub use builder::{ParameterLogging, PoolBuilder, RecyclingMethod};
// `pub` (instead of `pub(crate)`) so integration tests in
// `tests/manager_smoke.rs` can construct `JobManager` directly. The
// `#[doc(hidden)]` attribute keeps the type out of the rendered rustdoc API
// surface — external users construct `Pool` via `Pool::builder` (Task 10) and
// never need to touch `JobManager`. See plan §7.3.
#[doc(hidden)]
pub use manager::JobManager;
pub use pool::Pool;
