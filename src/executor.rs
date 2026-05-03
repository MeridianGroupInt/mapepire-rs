//! `Executor` trait — common SQL dispatch surface.
//!
//! A single trait that `&Job`, `&Pool`, and `&Reserved` all implement lets
//! callers write generic helpers (e.g. a retry wrapper, a tracing decorator)
//! once instead of once per concrete type. Methods return
//! `Pin<Box<dyn Future<...> + Send + 'a>>` rather than `async fn` so the
//! trait remains object-safe and usable as `&dyn Executor`. For monomorphic
//! call sites, prefer the concrete types' inherent methods — they incur no
//! boxing overhead.

use std::future::Future;
use std::pin::Pin;

use crate::query::Rows;

/// Anything that can run a SQL statement against a Db2 daemon.
///
/// Three concrete impls are provided in this crate:
///
/// - `&Job` — single-connection direct dispatch (added in v0.2).
/// - `&Pool` — least-busy-job pool routing (added in v0.3).
/// - `&Reserved` — exclusive single-connection handle for transactions (added in v0.3).
///
/// The trait returns boxed futures so it can be used as a trait object
/// (`&dyn Executor`). For monomorphic call sites, prefer the concrete
/// types' inherent methods.
pub trait Executor {
    /// Execute a SQL statement with no parameters.
    ///
    /// # Errors
    ///
    /// Whatever the underlying connection surfaces — see
    /// [`crate::Job::execute`] for the `&Job` impl.
    fn execute<'a>(
        &'a self,
        sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>>;

    /// Execute a parameterized SQL statement.
    ///
    /// # Errors
    ///
    /// As [`Executor::execute`].
    fn execute_with<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [serde_json::Value],
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>>;
}
