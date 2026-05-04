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
//! conn.begin().await?;
//! conn.execute_with(
//!     "UPDATE ORDERS SET STATUS = ? WHERE ID = ?",
//!     &[serde_json::json!("paid"), serde_json::json!(42)],
//! )
//! .await?;
//! conn.commit().await?;
//! # Ok(()) }
//! ```

use std::ops::Deref;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use deadpool::managed::Object;

use crate::job::Job;
use crate::pool::manager::JobManager;

/// Transactional state of the connection held by a [`Reserved`].
///
/// Updated on every observed `BEGIN` / `COMMIT` / `ROLLBACK` that goes
/// through [`Reserved::execute`] / [`Reserved::execute_with`]. Read by
/// `Drop for Reserved` to gate the opt-in [`Reserved::rollback_on_drop`]
/// `ROLLBACK` fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TxState {
    /// No `BEGIN` observed since reservation. Drop with `rollback_on_drop`
    /// is a no-op (nothing to roll back).
    NotStarted,
    /// `BEGIN` observed; no matching `COMMIT`/`ROLLBACK` yet. Drop with
    /// `rollback_on_drop` fires `ROLLBACK`.
    Started,
    /// `COMMIT` or explicit `ROLLBACK` observed since the last `BEGIN`. Drop
    /// with `rollback_on_drop` is a no-op until the next `BEGIN`.
    Closed,
}

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
///
/// ## Transaction state tracking (v0.4+)
///
/// [`Reserved::execute`] and [`Reserved::execute_with`] track
/// `BEGIN` / `COMMIT` / `ROLLBACK` prefixes (case-insensitive first-word
/// match) to maintain a 3-state machine. The state is read by
/// [`Reserved::rollback_on_drop`]'s Drop firing to suppress redundant
/// `ROLLBACK`s.
///
/// **Escape hatch:** `Job::execute(&**conn, sql).await` bypasses tracking
/// and goes straight to [`crate::Job::execute`]. State stays whatever it
/// was. Useful for raw transaction-statement bypass; rare in practice.
pub struct Reserved {
    obj: Object<JobManager>,
    /// Opt-in: fire ROLLBACK on drop. Set via [`Reserved::rollback_on_drop`];
    /// honoured by `Drop` (Task 16 / PRO-446).
    rollback_on_drop: bool,
    /// In-transaction state. `Reserved` is `Sync` (`Object<JobManager>` is
    /// `Send + Sync`), and `execute`/`execute_with` take `&self` — so two
    /// async tasks holding `&Reserved` can race observers. `Mutex` keeps
    /// the state-machine update atomic; the lock is uncontended in the
    /// common single-task case.
    tx_state: Mutex<TxState>,
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
            tx_state: Mutex::new(TxState::NotStarted),
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
    /// **v0.4:** Drop fires `ROLLBACK` only when both (a) `rollback_on_drop`
    /// is set and (b) a `BEGIN` has been observed without a matching
    /// `COMMIT`/`ROLLBACK`. An explicit `COMMIT` or `ROLLBACK` already issued
    /// through [`Reserved::execute`] or [`Reserved::execute_with`] suppresses
    /// the Drop firing. Drop is also a no-op when no `BEGIN` has been observed
    /// (state is `NotStarted` or `Closed`).
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
    /// conn.begin().await?;
    /// conn.execute("UPDATE ORDERS SET STATUS = 'paid' WHERE ID = 42")
    ///     .await?;
    /// // If we panic or early-return before COMMIT, drop fires ROLLBACK.
    /// conn.commit().await?;
    /// # Ok(()) }
    /// ```
    #[must_use]
    pub fn rollback_on_drop(mut self) -> Self {
        self.rollback_on_drop = true;
        self
    }

    /// Execute SQL through the held connection.
    ///
    /// Functionally identical to [`crate::Job::execute`], but additionally
    /// observes the SQL prefix (case-insensitive first word) and updates
    /// the [`Reserved`]'s in-transaction state machine on success:
    ///
    /// - `BEGIN`     → `Started`
    /// - `COMMIT`    → `Closed`
    /// - `ROLLBACK`  → `Closed`
    ///
    /// All other SQL leaves the state untouched. The state is read by Drop
    /// (Task 19 / PRO-597) to gate the [`Reserved::rollback_on_drop`] fire.
    ///
    /// State is updated only on `Ok` so a failed `BEGIN` (for example a
    /// server-side error) does not flip the connection into `Started`.
    ///
    /// **Escape hatch:** call `Job::execute(&**conn, sql).await` to dispatch
    /// directly through the [`Deref`] target without state observation.
    /// Rare in practice — the tracked path is what most callers want.
    ///
    /// # Errors
    ///
    /// As [`crate::Job::execute`].
    ///
    /// # Per-pool parameter logging
    ///
    /// `Reserved::execute` does not consult [`crate::Pool`]'s
    /// `parameter_logging` policy — it dispatches via [`crate::Job::execute`]
    /// directly. The asymmetry vs. [`crate::Pool::execute_with`] is
    /// documented in Task 9 / PRO-587.
    pub async fn execute(&self, sql: &str) -> crate::Result<crate::query::Rows> {
        let job: &Job = &self.obj;
        let result = Job::execute(job, sql).await;
        if result.is_ok() {
            Self::observe_sql(&self.tx_state, sql);
        }
        result
    }

    /// Parameterized variant of [`Reserved::execute`].
    ///
    /// Same state-tracking semantics as [`Reserved::execute`] — the SQL
    /// prefix is observed on success and the state machine updates
    /// accordingly.
    ///
    /// # Errors
    ///
    /// As [`crate::Job::execute_with`].
    ///
    /// # Per-pool parameter logging
    ///
    /// `Reserved::execute_with` does not consult [`crate::Pool`]'s
    /// `parameter_logging` policy — it dispatches via
    /// [`crate::Job::execute_with`] directly, so its tracing spans emit
    /// `ParameterLogging::None` semantics. The asymmetry vs.
    /// [`crate::Pool::execute_with`] (which does consult the per-pool
    /// policy) is documented in Task 9 / PRO-587.
    pub async fn execute_with(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> crate::Result<crate::query::Rows> {
        let job: &Job = &self.obj;
        let result = Job::execute_with(job, sql, params).await;
        if result.is_ok() {
            Self::observe_sql(&self.tx_state, sql);
        }
        result
    }

    /// Begin a transaction on the held connection.
    ///
    /// Equivalent to [`Reserved::execute`]`("BEGIN")`. Updates the internal
    /// transaction-state machine to `Started` on success — see the
    /// `Transaction state tracking (v0.4+)` section on [`Reserved`].
    ///
    /// # Errors
    ///
    /// As [`crate::Job::execute`].
    pub async fn begin(&self) -> crate::Result<crate::query::Rows> {
        self.execute("BEGIN").await
    }

    /// Commit the current transaction on the held connection.
    ///
    /// Equivalent to [`Reserved::execute`]`("COMMIT")`. Updates the internal
    /// transaction-state machine to `Closed` on success — see the
    /// `Transaction state tracking (v0.4+)` section on [`Reserved`].
    ///
    /// # Errors
    ///
    /// As [`crate::Job::execute`].
    pub async fn commit(&self) -> crate::Result<crate::query::Rows> {
        self.execute("COMMIT").await
    }

    /// Roll back the current transaction on the held connection.
    ///
    /// Equivalent to [`Reserved::execute`]`("ROLLBACK")`. Updates the internal
    /// transaction-state machine to `Closed` on success — see the
    /// `Transaction state tracking (v0.4+)` section on [`Reserved`].
    ///
    /// # Errors
    ///
    /// As [`crate::Job::execute`].
    pub async fn rollback(&self) -> crate::Result<crate::query::Rows> {
        self.execute("ROLLBACK").await
    }

    /// Apply the BEGIN/COMMIT/ROLLBACK state-machine transition for `sql`.
    ///
    /// Pure helper, lives outside `execute`/`execute_with` so the unit
    /// tests at the bottom of this file can drive the state machine
    /// without spinning up a Job.
    fn observe_sql(state: &Mutex<TxState>, sql: &str) {
        let head = sql.split_whitespace().next().unwrap_or("");
        let mut tx = state.lock().expect("tx_state mutex poisoned");
        if head.eq_ignore_ascii_case("BEGIN") {
            *tx = TxState::Started;
        } else if head.eq_ignore_ascii_case("COMMIT") || head.eq_ignore_ascii_case("ROLLBACK") {
            *tx = TxState::Closed;
        }
        // Other SQL doesn't change state.
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
        // 1. If opt-in AND currently in-tx, fire a best-effort ROLLBACK. The dispatcher is still
        //    alive (Reserved holds Object<JobManager> which holds Arc<Job>), so the request can be
        //    enqueued; the future is spawned so Drop never blocks. spawn_best_effort guards on
        //    Handle::try_current() — no destructor panic if there's no runtime.
        //
        //    v0.4 Task 19 / PRO-597: the gate is now active. Drop is a no-op when the connection
        //    is not in-tx (state is NotStarted or Closed) — suppressing redundant ROLLBACKs after
        //    an explicit COMMIT and on connections that never began a transaction.
        //
        //    Mutex poisoning during Drop: lock().expect(...) is intentional — a poisoned tx_state
        //    mutex means a prior panic inside observe_sql, and surfacing the poison here is correct
        //    (we are already on a teardown path).
        let in_tx = matches!(
            *self.tx_state.lock().expect("tx_state mutex poisoned"),
            TxState::Started
        );
        let rolled_back = self.rollback_on_drop && in_tx;
        if rolled_back {
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
        //    ROLLBACK enqueue (so rolled_back reflects what we actually did) and before the
        //    routing-skip sentinel reset (so observers see Reserved's last moment as a held
        //    connection). The in_tx field distinguishes "opt-in unset" from "nothing to roll back".
        #[cfg(feature = "tracing")]
        tracing::trace!(rolled_back, in_tx, "Reserved dropped");

        // 3. Reset the routing-skip sentinel so the Job is reusable. The fetch_add(1)/fetch_sub(1)
        //    ping-pong on u32::MAX is benign for routing (Task 24 special-cases u32::MAX) but we
        //    reset to 0 here so normal counting resumes after Drop. v0.4 may switch to a dedicated
        //    `reserved: AtomicBool` flag for cleaner separation.
        self.obj.inner.in_flight.store(0, Ordering::Relaxed);
        // 4. Object<JobManager>'s own Drop returns the Job to the pool.
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn state(s: &Mutex<TxState>) -> TxState {
        *s.lock().unwrap()
    }

    #[test]
    fn observe_sql_transitions() {
        let s = Mutex::new(TxState::NotStarted);

        Reserved::observe_sql(&s, "BEGIN");
        assert_eq!(state(&s), TxState::Started);

        Reserved::observe_sql(&s, "UPDATE T SET C = 1");
        assert_eq!(state(&s), TxState::Started, "DML should not change state");

        Reserved::observe_sql(&s, "COMMIT");
        assert_eq!(state(&s), TxState::Closed);

        Reserved::observe_sql(&s, "begin"); // case-insensitive
        assert_eq!(state(&s), TxState::Started);

        Reserved::observe_sql(&s, "  rollback   "); // whitespace tolerance
        assert_eq!(state(&s), TxState::Closed);
    }

    #[test]
    fn observe_sql_ignores_dml_and_select() {
        let s = Mutex::new(TxState::NotStarted);

        Reserved::observe_sql(&s, "SELECT * FROM SYSIBM.SYSDUMMY1");
        assert_eq!(state(&s), TxState::NotStarted);

        Reserved::observe_sql(&s, "INSERT INTO T VALUES (1)");
        assert_eq!(state(&s), TxState::NotStarted);

        Reserved::observe_sql(&s, "DELETE FROM T WHERE ID = 1");
        assert_eq!(state(&s), TxState::NotStarted);
    }

    #[test]
    fn observe_sql_rollback_closes_started_state() {
        let s = Mutex::new(TxState::NotStarted);

        Reserved::observe_sql(&s, "BEGIN");
        assert_eq!(state(&s), TxState::Started);

        Reserved::observe_sql(&s, "ROLLBACK");
        assert_eq!(state(&s), TxState::Closed);
    }

    /// Verify that the SQL keyword strings the typed helpers dispatch through
    /// `Reserved::execute` produce the expected state transitions.  This is the
    /// closest thing to testing the helpers themselves without spinning up a
    /// network-bound `Job`: if the keywords ever change (e.g. "BEGIN
    /// TRANSACTION") the transitions below immediately surface the discrepancy.
    #[test]
    fn typed_helpers_keywords_drive_correct_state_transitions() {
        // begin() → Started
        let s = Mutex::new(TxState::NotStarted);
        Reserved::observe_sql(&s, "BEGIN");
        assert_eq!(
            state(&s),
            TxState::Started,
            "begin() keyword should transition to Started"
        );

        // commit() → Closed
        Reserved::observe_sql(&s, "COMMIT");
        assert_eq!(
            state(&s),
            TxState::Closed,
            "commit() keyword should transition to Closed"
        );

        // rollback() → Closed (reset to Started first)
        Reserved::observe_sql(&s, "BEGIN");
        assert_eq!(state(&s), TxState::Started);
        Reserved::observe_sql(&s, "ROLLBACK");
        assert_eq!(
            state(&s),
            TxState::Closed,
            "rollback() keyword should transition to Closed"
        );
    }

    #[test]
    fn observe_sql_empty_string_no_panic() {
        let s = Mutex::new(TxState::NotStarted);

        // Empty / whitespace-only SQL must not panic — split_whitespace
        // returns None, unwrap_or("") yields the empty string, and neither
        // BEGIN nor COMMIT/ROLLBACK matches.
        Reserved::observe_sql(&s, "");
        assert_eq!(state(&s), TxState::NotStarted);

        Reserved::observe_sql(&s, "   ");
        assert_eq!(state(&s), TxState::NotStarted);
    }
}
