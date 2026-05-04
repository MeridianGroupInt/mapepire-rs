//! Pool routing registry — weak references to every `Job` the pool has
//! ever created. Used by [`crate::Pool::execute`] (§7.3 step 1 and step 2)
//! to peek `in_flight` on Jobs without taking ownership.
//!
//! `Type = Arc<Job>` from `JobManager` (Task 6 / PRO-436) is what makes
//! `Weak<Job>` storage possible — the routing scan can upgrade a `Weak`
//! to peek the `in_flight` counter without touching the deadpool checkout
//! state.

use std::sync::{Arc, Mutex, Weak};

use crate::job::Job;

#[derive(Default)]
pub(crate) struct Registry {
    weaks: Mutex<Vec<Weak<Job>>>,
}

impl Registry {
    /// Track a newly-created `Arc<Job>`. Stores a `Weak` so the registry
    /// doesn't keep the connection alive past its natural drop.
    pub(crate) fn track(&self, job: &Arc<Job>) {
        let mut w = self.weaks.lock().expect("registry mutex poisoned");
        w.push(Arc::downgrade(job));
    }

    /// Return the first `Arc<Job>` in the registry that is genuinely idle:
    /// `in_flight == 0` AND `Arc::strong_count == 2` (one base ref held by
    /// deadpool's internal slot + the one we just upgraded — i.e. nobody
    /// has it checked out via `Object<JobManager>`).
    ///
    /// Skips Jobs whose `in_flight` is `u32::MAX` (Reserved sentinel — they
    /// are exclusively held and must NOT be picked for routed work).
    ///
    /// Garbage-collects dead `Weak` entries as a side effect (opportunistic
    /// GC, same pattern as `least_busy`).
    ///
    /// Used by `Pool::execute` / `execute_with` step 1 (Task 22 / PRO-600)
    /// as the v0.4 replacement for the v0.3 `timeout_get(recycle: ZERO)`
    /// fast path that thrashed connections on real IBM i ping RTT.
    pub(crate) fn peek_idle(&self) -> Option<Arc<Job>> {
        let mut w = self.weaks.lock().expect("registry mutex poisoned");
        // GC dead refs while we hold the lock.
        w.retain(|wk| wk.strong_count() > 0);
        for wk in w.iter() {
            if let Some(arc) = wk.upgrade() {
                let n = arc.in_flight();
                if n == u32::MAX {
                    // Reserved sentinel — skip.
                    continue;
                }
                if n == 0 && Arc::strong_count(&arc) == 2 {
                    // deadpool's internal slot holds 1 ref; our upgrade holds
                    // the 2nd. strong_count == 2 means nobody has this Job
                    // checked out via Object<JobManager>.
                    return Some(arc);
                }
            }
        }
        None
    }

    /// Return up to `limit` upgradeable Jobs sorted by `in_flight` ascending.
    /// Skips Jobs whose `in_flight` is `u32::MAX` (Reserved sentinel — they
    /// are exclusively held and must NOT be picked for routed work).
    ///
    /// Garbage-collects dead `Weak` entries as a side effect (whenever this
    /// method runs — opportunistic GC is sufficient for v0.3).
    ///
    /// Consumed by `Pool::execute`'s §7.3 routing scan (Task 24 / PRO-454).
    pub(crate) fn least_busy(&self, limit: usize) -> Vec<Arc<Job>> {
        let mut live: Vec<(u32, Arc<Job>)> = {
            let mut w = self.weaks.lock().expect("registry mutex poisoned");
            // GC dead refs while we hold the lock.
            w.retain(|wk| wk.strong_count() > 0);
            w.iter()
                .filter_map(Weak::upgrade)
                .filter_map(|arc| {
                    let n = arc.in_flight();
                    if n == u32::MAX { None } else { Some((n, arc)) }
                })
                .collect()
        };
        live.sort_by_key(|(n, _)| *n);
        live.into_iter().take(limit).map(|(_, a)| a).collect()
    }
}
