//! Phase 6 integration test: cancellation safety.
//!
//! Drops the future returned by `Job::ping()` mid-flight (via `tokio::time::timeout`
//! with a very short duration), then issues a normal `ping()` and asserts it succeeds.
//! Tests AGENTS.md §5.3's load-bearing invariant: dropping a public future must not
//! leak resources or leave the connection in an invalid state.
//!
//! The dispatcher's design (per PR #30): when the caller drops the future, the
//! `oneshot::Receiver` drops; the dispatcher's eventual `reply.send(Ok(_))` silently
//! fails. The pending `HashMap` entry is reaped on the next response (silently
//! discarded) or on shutdown drain. No leak in operation; no panic.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_dropped_ping_does_not_break_subsequent_calls() {
    use std::time::Duration;

    let job = common::spawn_mock_and_connect().await;

    // Cancel a ping mid-flight via timeout. 1 µs is below most platforms' clock
    // resolution, so the timeout fires nearly immediately. If the ping completes
    // before the timeout fires on this platform, that's fine — the test asserts the
    // OBSERVABLE consequence (the subsequent ping works), not whether cancellation
    // was actually triggered on this particular run. Either way the future is
    // eventually dropped, exercising the cancellation path at the end of the test.
    let _ = tokio::time::timeout(Duration::from_micros(1), job.ping()).await;

    // The next ping must succeed — proves the dispatcher recovered cleanly and the
    // connection is not in an invalid state.
    let _rtt = job
        .ping()
        .await
        .expect("subsequent ping must succeed after cancelled ping");
}
