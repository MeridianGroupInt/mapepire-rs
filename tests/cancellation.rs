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

/// Deterministic counterpart to the timeout-based test above.
///
/// The earlier test relies on `tokio::time::timeout(Duration::from_micros(1), ...)`
/// to cancel the ping mid-flight. On fast hardware the ping can complete before
/// the timeout fires, in which case the cancellation path is *not* exercised.
/// CI green there only proves "subsequent ping works after a maybe-cancelled
/// future was dropped" — useful but weak.
///
/// This test forces the cancellation. The mock is configured with
/// [`common::MockBehavior::SwallowFirstPing`]: on the very first `Ping` it
/// reads off the wire, it fires a oneshot signal and intentionally does NOT
/// reply. The test races a `biased` [`tokio::select!`] between the signal
/// receiver and `job.ping()`. Polling order matters:
///
/// 1. First poll: `signal_rx` is `Pending` (mock has not seen the ping yet), so the select polls
///    `job.ping()`. The dispatcher writes the Ping frame onto the WebSocket and parks the future on
///    its `oneshot::Receiver`.
/// 2. The mock reads the frame and fires `signal_tx.send(())`.
/// 3. Next poll cycle: `biased` polls `signal_rx` first, which is now `Ready`. The select arm runs
///    and the `job.ping()` future is dropped — guaranteed, every run, on every platform.
///
/// Dropping the future drops the dispatcher's `oneshot::Receiver`. The
/// dispatcher's pending `HashMap` entry stays until a *response* with that id
/// arrives — but the mock will never send one for this id, so the entry is
/// only reaped on shutdown drain. A subsequent `ping()` issues a fresh id,
/// the mock responds with `Pong`, and the dispatcher routes it normally —
/// proving cancellation did not corrupt the connection state.
#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_inflight_ping_cancellation_preserves_connection() {
    let (signal_tx, signal_rx) = tokio::sync::oneshot::channel::<()>();
    let job = common::spawn_mock_swallow_first_ping_and_connect(signal_tx).await;

    // Race the in-flight ping against the mock's "I saw the request" signal.
    // `biased` makes the select check `signal_rx` first on every poll, so the
    // moment the mock fires the signal we drop the ping future without ever
    // letting it complete.
    tokio::select! {
        biased;
        _ = signal_rx => {
            // Mock confirmed receipt; dropping the ping future happens
            // implicitly when the select's other arm is discarded.
        }
        _ = job.ping() => {
            panic!(
                "ping must not complete: mock SwallowFirstPing is configured to \
                 swallow the first ping without replying"
            );
        }
    }

    // The fresh ping must succeed — proves cancellation didn't leave the
    // dispatcher in a corrupted state. The mock will respond to this second
    // ping with a normal Pong (signal slot is None after the first take).
    let _rtt = job
        .ping()
        .await
        .expect("subsequent ping must succeed after cancelled in-flight ping");
}
