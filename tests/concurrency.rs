//! Phase 6 integration test: concurrent multiplexing on one Job.
//!
//! Issues three `Job::ping()` calls concurrently via `tokio::join!`. The
//! dispatcher's `pending` `HashMap` routes each `Pong` response back to its
//! originating `Ping` request based on the response `id`. All three must
//! complete (no deadlock, no misrouting), each returning a `Duration`.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_three_concurrent_pings_share_one_socket() {
    let job = common::spawn_mock_and_connect().await;

    // tokio::join! polls all three futures concurrently. The dispatcher
    // must correctly route each Pong back to its originating Ping.
    let (rtt1, rtt2, rtt3) = tokio::join!(job.ping(), job.ping(), job.ping());

    // All three must succeed (no deadlock, no misrouting).
    let _d1 = rtt1.expect("first ping");
    let _d2 = rtt2.expect("second ping");
    let _d3 = rtt3.expect("third ping");

    // Each Duration is wall-clock RTT — non-negative by construction.
    // We don't assert specific values; just that all three completed.
}
