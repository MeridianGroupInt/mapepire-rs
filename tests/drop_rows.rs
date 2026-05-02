//! Phase 7 / Cleanup D integration test: Drop for Rows issues a
//! best-effort `sqlclose` for the server-side cursor.
//!
//! Three scenarios are exercised against the mock harness:
//!
//! 1. Dropping a `Rows` that was *never* streamed (cursor open, `is_done` = false) issues
//!    `SqlClose`.
//! 2. Dropping the [`futures::Stream`] returned by `Rows::stream` mid-iteration (cursor open)
//!    issues `SqlClose`.
//! 3. Dropping a fully-delivered `Rows` (single page, `is_done` = true, `cont_id` = None) issues
//!    *no* `SqlClose` — there is no cursor to close.
//!
//! `Drop for Rows` / `Drop for StreamState` use `spawn_best_effort` to
//! fire-and-forget the close, so the assertion site sleeps briefly to let
//! the spawned task transit the wire before reading the recorder.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use mapepire::protocol::{QueryResult, Request};
#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;

/// Build a `QueryResult` page suitable for the drop-rows tests.
///
/// `cont_id` is the server-side cursor handle (present when more pages
/// follow), `is_done` signals that no further pages exist. `data` is a
/// single dummy row so the stream-mid-iteration test can pull one row
/// before dropping.
#[cfg(feature = "rustls-tls")]
fn page(cont_id: Option<&str>, is_done: bool) -> QueryResult {
    use mapepire::{Column, QueryMetaData};
    use serde_json::{Map, Value, json};

    let mut row = Map::<String, Value>::new();
    row.insert("n".into(), json!(1));

    QueryResult {
        id: "placeholder".into(),
        success: true,
        execution_time: 1.0,
        has_results: true,
        update_count: -1,
        metadata: QueryMetaData {
            column_count: 1,
            columns: vec![Column {
                name: "n".into(),
                label: Some("n".into()),
                type_name: Some("INTEGER".into()),
                display_size: Some(10),
                scale: Some(0),
                precision: Some(10),
            }],
        },
        data: vec![row],
        cont_id: cont_id.map(str::to_string),
        is_done,
    }
}

/// Spin briefly until `pred` returns true on the recorder, or fail. The
/// best-effort `SqlClose` is dispatched by a fire-and-forget task spawned
/// inside `Drop`; the test thread reaches the assertion before that task
/// has serialized + flushed its frame on most runs. Polling under a budget
/// keeps the test fast in the common case and tolerant of scheduler jitter.
#[cfg(feature = "rustls-tls")]
async fn wait_for(
    recorder: &common::RequestRecorder,
    label: &str,
    mut pred: impl FnMut(&[Request]) -> bool,
) {
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        {
            let guard = recorder.lock().expect("recorder mutex not poisoned");
            if pred(&guard) {
                return;
            }
        }
        if Instant::now() >= deadline {
            let guard = recorder.lock().expect("recorder mutex not poisoned");
            panic!("timed out waiting for: {label}; observed = {:?}", &*guard);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Helper: count how many `SqlClose` requests the recorder has observed
/// for `cont_id`.
#[cfg(feature = "rustls-tls")]
fn count_sqlclose(observed: &[Request], cont_id: &str) -> usize {
    observed
        .iter()
        .filter(|r| matches!(r, Request::SqlClose { cont_id: c, .. } if c == cont_id))
        .count()
}

/// Dropping a `Rows` that was never streamed (cursor open) must fire a
/// best-effort `SqlClose` for the server-side cursor.
#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_drop_rows_without_streaming_releases_cursor() {
    let cont_id = "cur-close-1";

    let (job, recorder) =
        common::connect_to_mock_with_recorder(vec![page(Some(cont_id), false)]).await;

    let rows = job.execute("SELECT n FROM T").await.expect("execute");
    // Drop the Rows immediately — cursor is open, is_done is false.
    drop(rows);

    wait_for(&recorder, "SqlClose for cursor", |observed| {
        count_sqlclose(observed, cont_id) >= 1
    })
    .await;

    let observed = recorder.lock().expect("recorder mutex").clone();
    assert_eq!(
        count_sqlclose(&observed, cont_id),
        1,
        "expected exactly one SqlClose for cont_id={cont_id}; observed = {observed:?}"
    );
}

/// Dropping the stream returned by `Rows::stream` mid-iteration (after
/// pulling one row from the first page, cursor still open) must fire a
/// best-effort `SqlClose`. This is the canonical PRO-424 scenario.
#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_drop_stream_mid_iteration_releases_cursor() {
    use futures::{StreamExt, pin_mut};

    let cont_id = "cur-close-2";

    let (job, recorder) =
        common::connect_to_mock_with_recorder(vec![page(Some(cont_id), false)]).await;

    let rows = job.execute("SELECT n FROM T").await.expect("execute");

    {
        let stream = rows.stream();
        pin_mut!(stream);
        // Pull one row from the in-memory first page — does not trigger
        // a sqlmore (page has 1 row). Cursor is still open server-side.
        let row = stream
            .next()
            .await
            .expect("first row must be available")
            .expect("row decode succeeds");
        let n: i64 = row.get("n").expect("n column");
        assert_eq!(n, 1);
        // Drop the stream here — `Drop for StreamState` should fire SqlClose.
    }

    wait_for(&recorder, "SqlClose for mid-iteration cursor", |observed| {
        count_sqlclose(observed, cont_id) >= 1
    })
    .await;

    let observed = recorder.lock().expect("recorder mutex").clone();
    assert_eq!(
        count_sqlclose(&observed, cont_id),
        1,
        "expected exactly one SqlClose for cont_id={cont_id}; observed = {observed:?}"
    );
}

/// Dropping a fully-delivered `Rows` (server set `is_done = true`,
/// `cont_id = None`) must NOT issue a `SqlClose` — the server has
/// already released the cursor.
#[cfg(feature = "rustls-tls")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_drop_rows_done_does_not_send_close() {
    use std::time::Duration;

    let (job, recorder) = common::connect_to_mock_with_recorder(vec![page(None, true)]).await;

    let rows = job.execute("SELECT n FROM T").await.expect("execute");
    drop(rows);

    // Give any (incorrectly) spawned SqlClose task time to transit before
    // we assert its absence.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let observed = recorder.lock().expect("recorder mutex").clone();
    let close_count = observed
        .iter()
        .filter(|r| matches!(r, Request::SqlClose { .. }))
        .count();
    assert_eq!(
        close_count, 0,
        "expected zero SqlClose for fully-delivered result; observed = {observed:?}"
    );

    // Drop the job; the recorded sequence will then include Exit, but
    // that's not what we're asserting — we already snapshotted observed.
    drop(job);
}
