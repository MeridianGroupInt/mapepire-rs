//! Pool routing registry — weak references to every `Job` the pool has
//! ever created. Used by [`crate::Pool::execute`] (§7.3 step 2) to peek
//! `in_flight` on currently-checked-out jobs without taking ownership.
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

    /// Return up to `limit` upgradeable Jobs sorted by `in_flight` ascending.
    /// Skips Jobs whose `in_flight` is `u32::MAX` (Reserved sentinel — they
    /// are exclusively held and must NOT be picked for routed work).
    ///
    /// Garbage-collects dead `Weak` entries as a side effect (whenever this
    /// method runs — opportunistic GC is sufficient for v0.3).
    ///
    /// `dead_code` allow until Task 24 / PRO-454 wires `Pool::execute` to
    /// call this method (the routing scan).
    #[allow(dead_code)]
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
