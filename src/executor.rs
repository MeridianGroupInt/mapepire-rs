//! `Executor` trait — common SQL dispatch surface.
//!
//! A single trait that `&Job`, `&Pool`, and `&Reserved` all implement lets
//! callers write generic helpers (e.g. a retry wrapper, a tracing decorator)
//! once instead of once per concrete type. Methods return
//! `Pin<Box<dyn Future<...> + Send + 'a>>` rather than `async fn` so the
//! trait remains object-safe and usable as `&dyn Executor`. For monomorphic
//! call sites, prefer the concrete types' inherent methods — they incur no
//! boxing overhead.
//!
//! Single-statement surface only: `execute` / `execute_with` /
//! `execute_opts` / `execute_with_opts`. 2-D `execute_sets` is inherent on
//! [`crate::Job`] / [`crate::Query`], not this trait.
//!
//! ## Example — write a helper once, pass any executor
//!
//! ```no_run
//! use mapepire::Executor;
//! # use mapepire::{DaemonServer, Pool, TlsConfig};
//!
//! async fn count_all<E: Executor>(exe: &E) -> mapepire::Result<()> {
//!     let _rows = exe.execute("SELECT COUNT(*) FROM SYSIBM.SYSDUMMY1").await?;
//!     Ok(())
//! }
//!
//! # async fn example() -> mapepire::Result<()> {
//! # let server = DaemonServer::builder()
//! #     .host("ibmi.example.com")
//! #     .user("MYUSER")
//! #     .password("s3cret".to_string())
//! #     .tls(TlsConfig::Verified)
//! #     .build()
//! #     .expect("missing required field");
//! let pool = Pool::builder(server).max_size(2).build().await?;
//! count_all(&pool).await?;
//! let conn = pool.acquire().await?;
//! count_all(&conn).await?;
//! # Ok(()) }
//! ```

use std::future::Future;
use std::pin::Pin;

use crate::job::Job;
use crate::pool::{Pool, Reserved};
use crate::{ExecuteOptions, Rows};

/// Anything that can run a SQL statement against a Db2 daemon.
///
/// Three concrete impls are provided in this crate:
///
/// - [`Job`] — single-connection direct dispatch (added in v0.2).
/// - [`Pool`] — least-busy-job pool routing (added in v0.3).
/// - [`Reserved`] — exclusive single-connection handle for transactions (added in v0.3).
///
/// Single-statement methods only: [`Self::execute`], [`Self::execute_with`],
/// [`Self::execute_opts`], [`Self::execute_with_opts`]. 2-D
/// [`crate::Job::execute_sets`] is inherent on [`Job`] / [`crate::Query`],
/// not this trait, so the trait stays object-safe as `&dyn Executor`
/// without a seal.
///
/// The trait returns boxed futures so it can be used as a trait object.
/// For monomorphic call sites, prefer the concrete types' inherent methods.
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

    /// Execute a SQL statement with explicit [`ExecuteOptions`].
    ///
    /// # Errors
    ///
    /// As [`Executor::execute`], plus a protocol error when `rows` is 0.
    fn execute_opts<'a>(
        &'a self,
        sql: &'a str,
        opts: ExecuteOptions,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>>;

    /// Execute a parameterized SQL statement with explicit [`ExecuteOptions`].
    ///
    /// # Errors
    ///
    /// As [`Executor::execute_with`], plus a protocol error when `rows` is 0.
    fn execute_with_opts<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [serde_json::Value],
        opts: ExecuteOptions,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>>;
}

impl Executor for Job {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        Box::pin(async move { Job::execute(self, sql).await })
    }

    fn execute_with<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [serde_json::Value],
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        Box::pin(async move { Job::execute_with(self, sql, params).await })
    }

    fn execute_opts<'a>(
        &'a self,
        sql: &'a str,
        opts: ExecuteOptions,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        Box::pin(async move { Job::execute_opts(self, sql, opts).await })
    }

    fn execute_with_opts<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [serde_json::Value],
        opts: ExecuteOptions,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        Box::pin(async move { Job::execute_with_opts(self, sql, params, opts).await })
    }
}

impl Executor for Pool {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        // Pool::execute / Pool::execute_with futures contain the routing
        // scan + checkout state machine; box at the trait boundary to
        // satisfy clippy::large_futures.
        Box::pin(Pool::execute(self, sql))
    }

    fn execute_with<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [serde_json::Value],
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        // Pool::execute / Pool::execute_with futures contain the routing
        // scan + checkout state machine; box at the trait boundary to
        // satisfy clippy::large_futures.
        Box::pin(Pool::execute_with(self, sql, params))
    }

    fn execute_opts<'a>(
        &'a self,
        sql: &'a str,
        opts: ExecuteOptions,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        Box::pin(Pool::execute_opts(self, sql, opts))
    }

    fn execute_with_opts<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [serde_json::Value],
        opts: ExecuteOptions,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        Box::pin(Pool::execute_with_opts(self, sql, params, opts))
    }
}

impl Executor for Reserved {
    fn execute<'a>(
        &'a self,
        sql: &'a str,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        // Reserved derefs to &Job; we route through the Job impl so the
        // in_flight bookkeeping (set to u32::MAX while reserved) remains
        // consistent.
        let job: &Job = self;
        Box::pin(async move { Job::execute(job, sql).await })
    }

    fn execute_with<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [serde_json::Value],
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        let job: &Job = self;
        Box::pin(async move { Job::execute_with(job, sql, params).await })
    }

    fn execute_opts<'a>(
        &'a self,
        sql: &'a str,
        opts: ExecuteOptions,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        Box::pin(async move { Reserved::execute_opts(self, sql, opts).await })
    }

    fn execute_with_opts<'a>(
        &'a self,
        sql: &'a str,
        params: &'a [serde_json::Value],
        opts: ExecuteOptions,
    ) -> Pin<Box<dyn Future<Output = crate::Result<Rows>> + Send + 'a>> {
        Box::pin(async move { Reserved::execute_with_opts(self, sql, params, opts).await })
    }
}
