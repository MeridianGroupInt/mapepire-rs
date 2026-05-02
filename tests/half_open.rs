//! Phase 6 integration test: half-open socket (server stops responding).
//!
//! The mock's `MockBehavior::HalfOpen` accepts the WebSocket Upgrade, responds
//! to the client's `Connect` request with `Connected`, then enters a "go silent"
//! loop — never responding to any subsequent request. This simulates a
//! production failure mode where the daemon process is hung but the OS-level
//! socket is still alive (no FIN, no RST).
//!
//! The dispatcher's `pending` `HashMap` accumulates entries that will never be
//! resolved. Production callers wrap individual operations in
//! `tokio::time::timeout` to bound waits; this test verifies the pattern works
//! end-to-end.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_half_open_socket_ping_times_out() {
    use std::time::Duration;

    use tokio::time;

    let job = common::connect_to_mock(common::MockBehavior::HalfOpen).await;

    // Mock won't respond to ping. tokio::time::timeout bounds the wait.
    // 200ms is enough to be confident the ping is genuinely stuck without
    // slowing the suite; production callers would use longer bounds.
    let result = time::timeout(Duration::from_millis(200), job.ping()).await;

    match result {
        Err(_elapsed) => {
            // Expected: ping never completed; timeout fired.
        }
        Ok(other) => panic!("expected timeout against half-open mock, got ping result: {other:?}"),
    }
}
