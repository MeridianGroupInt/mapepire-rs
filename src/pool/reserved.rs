//! [`Reserved`] — exclusive single-connection handle for transactions.
//!
//! Spec §7.4. `Reserved` is `!Clone`. It derefs to `&Job`, so v0.2's
//! [`crate::Job::execute`], [`crate::Job::prepare`], [`crate::Job::ping`]
//! etc. work directly:
//!
//! ```no_run
//! # use mapepire::{DaemonServer, Pool, TlsConfig};
//! # async fn example() -> mapepire::Result<()> {
//! # let server = DaemonServer::builder()
//! #     .host("ibmi.example.com")
//! #     .user("MYUSER")
//! #     .password("s3cret".to_string())
//! #     .tls(TlsConfig::Verified)
//! #     .build()
//! #     .expect("missing required field");
//! # let pool = Pool::builder(server).max_size(2).build().await?;
//! let conn = pool.acquire().await?;
//! conn.execute("BEGIN").await?;
//! conn.execute_with(
//!     "UPDATE ORDERS SET STATUS = ? WHERE ID = ?",
//!     &[serde_json::json!("paid"), serde_json::json!(42)],
//! )
//! .await?;
//! conn.execute("COMMIT").await?;
//! # Ok(()) }
//! ```

use std::ops::Deref;
use std::sync::atomic::Ordering;

use deadpool::managed::Object;

use crate::job::Job;
use crate::pool::manager::JobManager;

/// Exclusive single-connection handle. Drops back to the pool on `Drop`.
///
/// Construct via [`crate::Pool::acquire`]. The held connection is marked
/// with `in_flight = u32::MAX` (a sentinel) so the routing scan in v0.3
/// §7.3 / Task 24 / PRO-454 treats it as "fully busy" and never picks it
/// for one-shot work — preserving exclusive transactional access.
///
/// `Reserved` is intentionally `!Clone`. The `rollback_on_drop()` opt-in
/// (added in Task 15 / PRO-445) provides safety against forgotten
/// `COMMIT`/`ROLLBACK`.
pub struct Reserved {
    obj: Object<JobManager>,
    /// Opt-in: fire ROLLBACK on drop. Set via [`Reserved::rollback_on_drop`];
    /// honoured by `Drop` (Task 16 / PRO-446).
    rollback_on_drop: bool,
}

impl Reserved {
    pub(crate) fn new(obj: Object<JobManager>) -> Self {
        // Mark the Job as routing-skip: `u32::MAX` is a sentinel, NOT a real
        // in-flight count. The routing scan in Task 24 / PRO-454 reads this
        // value and treats it as "this connection is exclusively held" — it
        // will pick a different idle Job for one-shot routed work. Drop
        // resets to 0 so the connection is routable again.
        obj.inner.in_flight.store(u32::MAX, Ordering::Relaxed);
        Self {
            obj,
            rollback_on_drop: false,
        }
    }

    /// Opt-in safety: if this `Reserved` drops without an explicit
    /// `COMMIT`/`ROLLBACK`, fire a fire-and-forget `ROLLBACK` on drop.
    ///
    /// The rollback runs via a runtime-guarded fire-and-forget spawn (the
    /// crate's internal `spawn_best_effort` helper) so `Drop` never blocks.
    /// If the rollback fails (e.g., the connection is already dead), the
    /// error is silently dropped — the pool's next `recycle()` will pick up
    /// the failed connection.
    ///
    /// **v0.3 limitation:** the rollback fires unconditionally when this flag
    /// is set, even if the caller already issued an explicit `COMMIT`.
    /// Tighter "only-if-still-in-tx" semantics need `BEGIN`/`COMMIT` state
    /// tracking on `Reserved` and are deferred to v0.4. For now, callers who
    /// set `rollback_on_drop()` and then `COMMIT` explicitly will see a
    /// best-effort `ROLLBACK` follow the `COMMIT` — Db2 returns a no-op
    /// `SQLSTATE 25000` ("invalid transaction state") which the pool's
    /// recycle path tolerates.
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
    /// # let pool = Pool::builder(server).max_size(2).build().await?;
    /// let conn = pool.acquire().await?.rollback_on_drop();
    /// conn.execute("BEGIN").await?;
    /// conn.execute("UPDATE ORDERS SET STATUS = 'paid' WHERE ID = 42")
    ///     .await?;
    /// // If we panic or early-return before COMMIT, drop fires ROLLBACK.
    /// conn.execute("COMMIT").await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn rollback_on_drop(mut self) -> Self {
        self.rollback_on_drop = true;
        self
    }
}

impl Deref for Reserved {
    type Target = Job;
    fn deref(&self) -> &Job {
        // Object<JobManager> derefs to &Arc<Job>; Arc<Job> derefs to &Job.
        &self.obj
    }
}

impl Drop for Reserved {
    fn drop(&mut self) {
        // 1. If opt-in, fire a best-effort ROLLBACK. The dispatcher is still alive (Reserved holds
        //    Object<JobManager> which holds Arc<Job>), so the request can be enqueued; the future
        //    is spawned so Drop never blocks. spawn_best_effort guards on Handle::try_current() —
        //    no destructor panic if there's no runtime.
        //
        //    v0.3 limitation: this fires unconditionally when the flag is
        //    set, even if the user already issued an explicit COMMIT. Tighter
        //    "only-if-still-in-tx" semantics need BEGIN/COMMIT state tracking
        //    on Reserved and are deferred to v0.4.
        if self.rollback_on_drop {
            let handle = self.obj.inner.handle.clone();
            let id = self.obj.inner.ids.next();
            crate::job_helpers::spawn_best_effort(async move {
                let req = crate::protocol::Request::Sql {
                    id: id.clone(),
                    sql: "ROLLBACK".into(),
                    rows: None,
                    parameters: None,
                };
                let _ = handle.send(req).await;
            });
            #[cfg(feature = "metrics")]
            metrics::counter!(crate::observability::POOL_RESERVED_ROLLBACK_TOTAL).increment(1);
        }

        // 2. Emit a single trace event capturing the state-at-drop. Fires after the best-effort
        //    ROLLBACK enqueue (so the rolled_back flag reflects what we actually did) and before
        //    the routing-skip sentinel reset (so observers see Reserved's last moment as a held
        //    connection). The `in_tx` field will be added in Task 18 / PRO-596 once TxState
        //    tracking lands.
        #[cfg(feature = "tracing")]
        tracing::trace!(rolled_back = self.rollback_on_drop, "Reserved dropped");

        // 3. Reset the routing-skip sentinel so the Job is reusable. The fetch_add(1)/fetch_sub(1)
        //    ping-pong on u32::MAX is benign for routing (Task 24 special-cases u32::MAX) but we
        //    reset to 0 here so normal counting resumes after Drop. v0.4 may switch to a dedicated
        //    `reserved: AtomicBool` flag for cleaner separation.
        self.obj.inner.in_flight.store(0, Ordering::Relaxed);
        // 4. Object<JobManager>'s own Drop returns the Job to the pool.
    }
}
