//! Phase 5 integration test: `Reserved` keeps every statement on one socket.
//!
//! Spec §7.4 invariant: all statements issued through a [`mapepire::Reserved`]
//! (BEGIN, DML, COMMIT) MUST land on the same Db2 job — i.e., the same TCP
//! socket. The mock harness in `tests/common/mock_server.rs` is
//! *single-connection per spawn* (one TCP accept then exit), so the
//! "single-socket" invariant is **architecturally implicit**: one
//! `Pool::acquire()` triggers one `JobManager::create()` which opens exactly
//! one TCP session, and every request issued through `Reserved` (which derefs
//! to the held `&Job`) round-trips on that one connection.
//!
//! What we *can* verify with the existing mock is the **end-to-end behavior
//! of a multi-statement transaction through Reserved**:
//!
//! 1. The dispatcher actually saw all 3 SQL requests in the order issued (BEGIN, DML, COMMIT) —
//!    verified via the [`MockBehavior::Pages`] recorder.
//! 2. None of the statements panicked or surfaced an error — verified by `expect`-ing each
//!    `Job::execute*` future.
//!
//! Full transaction-isolation coverage with concurrent acquires lands in
//! Task 28 / PRO-458 (which extends this binary toward
//! reserved-vs-one-shot interleaving). The plan's original
//! `MockHandle::observed_socket_ids` design is impractical against the
//! current single-connection mock — that assertion would always pass even
//! if `Reserved` were broken — so this test pins the observable side of the
//! invariant via the request recorder instead.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reserved_runs_begin_dml_commit_on_one_socket() {
    use std::time::Duration;

    use common::spawn_mock_pool_with_recorder;
    use mapepire::protocol::{QueryResult, Request};
    use mapepire::{Column, QueryMetaData};

    // Three canned `QueryResult` pages drained in order by the mock for
    // BEGIN, the parameterized UPDATE, and COMMIT. Each is a no-op result
    // (no rows, `is_done = true`) — Db2 BEGIN/COMMIT/UPDATE typically
    // surface as `update_count` rather than result rows. The mock stamps
    // the per-request id onto `page.id` before sending, so dispatcher
    // correlation works regardless of the placeholder.
    let canned = || QueryResult {
        id: "placeholder".into(),
        success: true,
        execution_time: 0.0,
        has_results: false,
        update_count: 0,
        metadata: QueryMetaData {
            column_count: 0,
            columns: Vec::<Column>::new(),
        },
        data: Vec::new(),
        cont_id: None,
        is_done: true,
    };
    let pages = vec![canned(), canned(), canned()];

    let (server_arc, recorder) = spawn_mock_pool_with_recorder(pages);

    // `Box::pin` the pool-build future to satisfy `clippy::large_futures`
    // — the build path contains no eager TLS handshake (default
    // `starting_size = 0`), so this is mostly stylistic consistency with
    // sibling tests.
    let pool = Box::pin(mapepire::Pool::builder(server_arc).max_size(2).build())
        .await
        .expect("pool builds");

    // Acquire the Reserved, issue BEGIN → UPDATE → COMMIT.
    //
    // `Box::pin` on `pool.acquire()` and each `execute*` future for the
    // same `clippy::large_futures` reason as the sibling tests in
    // `tests/manager_smoke.rs` — the futures contain TLS state machines
    // and dispatcher correlation maps. Dropping the resulting `Rows` is
    // fire-and-forget; we don't iterate (every page is empty).
    let conn = Box::pin(pool.acquire()).await.expect("acquire");
    drop(Box::pin(conn.execute("BEGIN")).await.expect("begin"));
    drop(
        Box::pin(conn.execute_with(
            "UPDATE T SET C = ? WHERE I = ?",
            &[serde_json::json!(1), serde_json::json!(2)],
        ))
        .await
        .expect("dml"),
    );
    drop(Box::pin(conn.execute("COMMIT")).await.expect("commit"));

    // Verify the dispatcher emitted exactly the three expected SQL
    // requests in order. `Job::execute` and `Job::execute_with` both emit
    // `Request::Sql` (see `src/job.rs::execute_inner`). Filtering by
    // variant guards against incidental traffic — e.g., the `recycle()`
    // ping that fires when `conn` drops below.
    let observed = recorder.lock().expect("recorder mutex").clone();
    let sql_requests: Vec<&Request> = observed
        .iter()
        .filter(|r| matches!(r, Request::Sql { .. }))
        .collect();
    assert_eq!(
        sql_requests.len(),
        3,
        "expected 3 Sql requests through Reserved, got {} (full trace: {:?})",
        sql_requests.len(),
        observed,
    );

    // Spot-check the SQL text and parameter shape. The mock returns pages
    // in the order received via `Vec::drain`, so position is stable.
    match &sql_requests[0] {
        Request::Sql {
            sql, parameters, ..
        } => {
            assert_eq!(sql, "BEGIN");
            assert!(parameters.is_none(), "BEGIN should carry no parameters");
        }
        other => panic!("expected Sql(BEGIN), got {other:?}"),
    }
    match &sql_requests[1] {
        Request::Sql {
            sql, parameters, ..
        } => {
            assert!(
                sql.contains("UPDATE T"),
                "expected UPDATE T statement, got {sql:?}"
            );
            let params = parameters
                .as_ref()
                .expect("UPDATE should carry 2 parameters");
            assert_eq!(params.len(), 2, "UPDATE should carry 2 parameters");
        }
        other => panic!("expected Sql(UPDATE), got {other:?}"),
    }
    match &sql_requests[2] {
        Request::Sql {
            sql, parameters, ..
        } => {
            assert_eq!(sql, "COMMIT");
            assert!(parameters.is_none(), "COMMIT should carry no parameters");
        }
        other => panic!("expected Sql(COMMIT), got {other:?}"),
    }

    // Drop the Reserved — resets the routing-skip sentinel. The pool's
    // `recycle()` will issue a Ping on the way back; a brief sleep lets
    // any post-drop async work transit before the test exits, matching
    // the precedent in `tests/drop_rows.rs`.
    drop(conn);
    tokio::time::sleep(Duration::from_millis(20)).await;
}

/// Multi-connection extension of `reserved_runs_begin_dml_commit_on_one_socket`
/// (Task 14) — uses Task 26's multi-connection mock + `MockHandle` observation
/// hooks to verify the spec §7.4 invariant *as an explicit equality* on
/// observed socket ids, not just an architectural implication.
///
/// Setup: build a `Pool` of capacity 4 against a multi-connection mock,
/// pre-populate two background sockets (so the routing scan has multiple
/// least-busy candidates), then acquire a `Reserved` and run BEGIN /
/// parameterized UPDATE / COMMIT through it. Assert that
/// `MockHandle::last_socket_for_sql` returns the **same** socket id for all
/// three statements.
#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reserved_pins_transaction_to_one_socket() {
    use common::spawn_mock_pool;

    let (pool, mock) = spawn_mock_pool(4).await;

    // Pre-populate two more sockets so the routing scan would otherwise
    // see lower-in_flight candidates (background traffic). Drop the rows
    // immediately so they're not held mid-Reserved.
    drop(
        Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
            .await
            .expect("bg1"),
    );
    drop(
        Box::pin(pool.execute("SELECT 1 FROM SYSIBM.SYSDUMMY1"))
            .await
            .expect("bg2"),
    );

    // Now acquire a Reserved and run BEGIN/UPDATE/COMMIT.
    let conn = Box::pin(pool.acquire()).await.expect("acquire");
    drop(Box::pin(conn.execute("BEGIN")).await.expect("begin"));
    drop(
        Box::pin(conn.execute_with(
            "UPDATE T SET C = ? WHERE I = ?",
            &[serde_json::json!(1), serde_json::json!(2)],
        ))
        .await
        .expect("dml"),
    );
    drop(Box::pin(conn.execute("COMMIT")).await.expect("commit"));

    // All three transactional statements must land on the same socket id.
    let socket_begin = mock
        .last_socket_for_sql("BEGIN")
        .expect("BEGIN must have been observed");
    let socket_update = mock
        .last_socket_for_sql("UPDATE")
        .expect("UPDATE must have been observed");
    let socket_commit = mock
        .last_socket_for_sql("COMMIT")
        .expect("COMMIT must have been observed");

    assert_eq!(
        socket_begin, socket_update,
        "BEGIN and UPDATE must share a socket; saw {socket_begin} vs {socket_update}"
    );
    assert_eq!(
        socket_update, socket_commit,
        "UPDATE and COMMIT must share a socket; saw {socket_update} vs {socket_commit}"
    );

    drop(conn);
}

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_with_rollback_on_drop_sends_rollback() {
    use std::time::Duration;

    use common::spawn_mock_pool;

    // Use the multi-connection mock so we get a MockHandle for wait_for_sql.
    // The generic happy-path mock responds to any Sql request with an empty
    // terminal QueryResult, which is all BEGIN / UPDATE / ROLLBACK need.
    let (pool, mock) = spawn_mock_pool(1).await;

    {
        let conn = Box::pin(pool.acquire())
            .await
            .expect("acquire")
            .rollback_on_drop();
        drop(Box::pin(conn.execute("BEGIN")).await.expect("begin"));
        drop(
            Box::pin(conn.execute("UPDATE T SET C = 1"))
                .await
                .expect("dml"),
        );
        // No COMMIT — drop fires ROLLBACK best-effort because tx_state == Started.
    }

    // Wait for the fire-and-forget Drop ROLLBACK to land at the mock.
    // Polls every 10ms up to a 2s budget — tighter than the former 100ms
    // sleep and immune to CI scheduler pressure.
    let arrived = mock.wait_for_sql("ROLLBACK", Duration::from_secs(2)).await;
    assert!(arrived, "ROLLBACK should have arrived within 2s budget");

    // Belt-and-suspenders: confirm via last_socket_for_sql that the mock
    // recorded a Sql request whose text is exactly "ROLLBACK" (not just a
    // substring match on another request). Redundant but cheap.
    assert!(
        mock.last_socket_for_sql("ROLLBACK").is_some(),
        "expected ROLLBACK in observed SQL history"
    );
}

#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_without_rollback_on_drop_does_not_send_rollback() {
    use std::time::Duration;

    use common::spawn_mock_pool_with_recorder;
    use mapepire::protocol::{QueryResult, Request};
    use mapepire::{Column, QueryMetaData};

    let canned = || QueryResult {
        id: "placeholder".into(),
        success: true,
        execution_time: 0.0,
        has_results: false,
        update_count: 0,
        metadata: QueryMetaData {
            column_count: 0,
            columns: Vec::<Column>::new(),
        },
        data: Vec::new(),
        cont_id: None,
        is_done: true,
    };
    let pages = vec![canned()]; // just the UPDATE; no ROLLBACK expected

    let (server_arc, recorder) = spawn_mock_pool_with_recorder(pages);
    let pool = Box::pin(mapepire::Pool::builder(server_arc).max_size(1).build())
        .await
        .expect("pool builds");

    {
        let conn = Box::pin(pool.acquire()).await.expect("acquire");
        // NO .rollback_on_drop() — drop must NOT fire ROLLBACK.
        drop(
            Box::pin(conn.execute("UPDATE T SET C = 1"))
                .await
                .expect("dml"),
        );
    }

    // Same wait as the positive test — confirms absence rather than
    // racing the spawned task.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let observed = recorder.lock().expect("recorder mutex").clone();
    let saw_rollback = observed.iter().any(|r| {
        matches!(
            r,
            Request::Sql { sql, .. } if sql.eq_ignore_ascii_case("ROLLBACK")
        )
    });
    assert!(
        !saw_rollback,
        "did NOT expect ROLLBACK in observed requests, got {observed:?}"
    );
}

/// Third coverage scenario: with `rollback_on_drop()` set AND an explicit
/// `COMMIT` already issued, dropping `Reserved` is a no-op — Task 19's
/// tightened v0.4 contract suppresses the redundant `ROLLBACK` because
/// `tx_state` already transitioned to `Closed` on the COMMIT.
#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_commit_suppresses_drop_rollback() {
    use std::time::Duration;

    use common::spawn_mock_pool_with_recorder;
    use mapepire::protocol::{QueryResult, Request};
    use mapepire::{Column, QueryMetaData};

    let canned = || QueryResult {
        id: "placeholder".into(),
        success: true,
        execution_time: 0.0,
        has_results: false,
        update_count: 0,
        metadata: QueryMetaData {
            column_count: 0,
            columns: Vec::<Column>::new(),
        },
        data: Vec::new(),
        cont_id: None,
        is_done: true,
    };
    // Three canned responses: BEGIN, COMMIT, and the recycle ping that
    // fires when the connection returns to the pool. No post-drop ROLLBACK
    // under the v0.4 contract.
    let pages = vec![canned(), canned(), canned()];

    let (server_arc, recorder) = spawn_mock_pool_with_recorder(pages);
    let pool = Box::pin(mapepire::Pool::builder(server_arc).max_size(1).build())
        .await
        .expect("pool builds");

    {
        let conn = Box::pin(pool.acquire())
            .await
            .expect("acquire")
            .rollback_on_drop();
        drop(Box::pin(conn.execute("BEGIN")).await.expect("begin"));
        drop(Box::pin(conn.execute("COMMIT")).await.expect("commit"));
        // v0.4 contract: COMMIT closed the tx, so Drop must NOT fire ROLLBACK.
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    let observed = recorder.lock().expect("recorder mutex").clone();
    let rollback_count = observed
        .iter()
        .filter(|r| {
            matches!(
                r,
                Request::Sql { sql, .. } if sql.eq_ignore_ascii_case("ROLLBACK")
            )
        })
        .count();

    assert_eq!(
        rollback_count, 0,
        "v0.4 contract: explicit COMMIT must suppress Drop ROLLBACK, got {rollback_count} (full trace: {observed:?})"
    );
}
