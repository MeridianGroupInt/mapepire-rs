//! Verify the dispatcher maintains [`mapepire::Job::in_flight`] across a
//! request/response round-trip.
//!
//! Two assertions:
//! 1. Post-response, `in_flight()` returns to 0 (no leak; the increment that must have fired during
//!    the round-trip was matched by a decrement).
//! 2. A concurrent observer task catches at least one `>= 1` reading while a ping is outstanding,
//!    proving the increment is actually visible to other tasks (not just bookkeeping internal to
//!    the dispatcher).
//!
//! The mock harness is rustls-only test infrastructure, so the `mod common;`
//! declaration and the test fn are gated by `#[cfg(feature = "rustls-tls")]`.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use std::sync::Arc;

#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_increments_then_resets_in_flight() {
    let job = common::spawn_mock_and_connect().await;

    // Pre-condition: the handshake's Connect request bumped to 1 and the
    // matching Connected response decremented back to 0.
    assert_eq!(job.in_flight(), 0, "fresh Job has 0 in-flight requests");

    // Simple post-await check: a single ping completes and `in_flight`
    // returns to 0. If the dispatcher's increment leaked (no matching
    // decrement) or never fired, this would fail (the latter only catches
    // the leak side; see the concurrent-observer below for catching the
    // increment side directly).
    job.ping().await.expect("ping ok");
    assert_eq!(job.in_flight(), 0, "in_flight resets to 0 after response");

    // Stronger check: drive a second ping concurrently with an observer
    // task that polls `in_flight()` and reports the highest value it sees.
    // Wrap `Job` in `Arc` so both tasks can call `&self` methods through
    // it (`Job` is `!Clone` by design — one dispatcher per Job).
    let job = Arc::new(job);
    let observer_job = Arc::clone(&job);
    let observer = tokio::spawn(async move {
        let mut max_seen = 0u32;
        // Bounded poll loop. The mock responds synchronously, so the ping
        // resolves in microseconds-to-milliseconds; 1_000 yields gives the
        // observer plenty of opportunity to catch a >= 1 reading without
        // spinning indefinitely if something goes wrong.
        for _ in 0..1_000 {
            let v = observer_job.in_flight();
            if v > max_seen {
                max_seen = v;
            }
            if max_seen >= 1 {
                return max_seen;
            }
            tokio::task::yield_now().await;
        }
        max_seen
    });

    let ping_result = job.ping().await;
    let observed_max = observer.await.expect("observer task ok");

    ping_result.expect("ping ok");
    assert!(
        observed_max >= 1,
        "observer caught in_flight >= 1 mid-request, got {observed_max}"
    );
    assert_eq!(job.in_flight(), 0, "in_flight resets after concurrent ping");
}
