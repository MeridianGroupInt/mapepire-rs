//! Phase 6 integration test: paging via sqlmore.
//!
//! Covers the tagged-mock two-page path and the live dialect (no
//! `cont_id`, omitted/`false` `is_done`) where `sqlmore.cont_id` is the
//! opening request id.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;

/// Build a single `QueryResult` page for the paging test.
///
/// `start` is the first row value, `count` is the number of rows on this page,
/// `cont_id` is the server-side cursor handle (present if more pages follow),
/// and `is_done` signals that no further pages exist.
#[cfg(feature = "rustls-tls")]
fn page(start: i64, count: i64, cont_id: Option<&str>, is_done: bool) -> mapepire::QueryResult {
    use mapepire::{Column, QueryMetaData, QueryResult};
    use serde_json::{Map, Value, json};

    let data: Vec<Map<String, Value>> = (start..start + count)
        .map(|i| {
            let mut row = Map::new();
            row.insert("n".into(), json!(i));
            row
        })
        .collect();

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
            job: None,
            parameters: vec![],
        },
        data,
        cont_id: cont_id.map(str::to_string),
        is_done,
        error: None,
        sqlcode: None,
        sqlstate: None,
        parameter_count: None,
        output_parms: vec![],
    }
}

/// Verify that `Rows::stream()` correctly issues a `sqlmore` request for the
/// follow-up page and yields all rows in order across the page boundary.
///
/// The "exactly 1 sqlmore was sent" assertion is implicit via the row count:
/// - Zero sqlmores → only the first 50 rows are yielded → `collected.len() == 50`.
/// - Two sqlmores → the mock's iterator exhausts and triggers its `expect("mock Pages ran out of
///   pre-baked pages")` panic → test failure.
/// - Exactly one sqlmore → 100 rows total, which is what we assert.
#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_paging_across_two_pages() {
    use futures::{StreamExt, pin_mut};

    let pages = vec![
        // Page 1: rows 0..50, is_done = false, cont_id = "cur-1".
        page(0, 50, Some("cur-1"), false),
        // Page 2: rows 50..100, is_done = true, cont_id = None.
        page(50, 50, None, true),
    ];

    let job = common::connect_to_mock(common::MockBehavior::Pages {
        pages,
        recorder: None,
    })
    .await;

    let rows = job
        .execute("SELECT n FROM SCHEMA.NUMBERS")
        .await
        .expect("execute");
    assert!(
        rows.has_results(),
        "SELECT should report has_results = true"
    );

    let stream = rows.stream();
    pin_mut!(stream);

    let mut collected: Vec<i64> = Vec::with_capacity(100);
    while let Some(row_result) = stream.next().await {
        let row = row_result.expect("row");
        let n: i64 = row.get("n").expect("n column");
        collected.push(n);
    }

    assert_eq!(
        collected.len(),
        100,
        "should have 100 rows total across two pages"
    );
    let expected: Vec<i64> = (0..100).collect();
    assert_eq!(collected, expected, "rows should be in order 0..100");
}

/// Opening `execute_opts(rows: 10)` must be reused on the follow-up `sqlmore`.
#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_sqlmore_uses_opening_page_size() {
    use mapepire::ExecuteOptions;
    use mapepire::protocol::Request;

    let pages = vec![page(0, 10, Some("cur-1"), false), page(10, 10, None, true)];
    let (job, recorder) = common::connect_to_mock_with_recorder(pages).await;

    let rows = job
        .execute_opts(
            "SELECT n FROM SCHEMA.NUMBERS",
            ExecuteOptions {
                rows: Some(10),
                terse: false,
            },
        )
        .await
        .expect("execute_opts");
    let all = rows.into_dynamic().await.expect("into_dynamic");
    assert_eq!(all.len(), 20);

    let observed = recorder.lock().expect("recorder mutex").clone();
    let sql: Vec<&Request> = observed
        .iter()
        .filter(|r| matches!(r, Request::Sql { .. }))
        .collect();
    assert_eq!(sql.len(), 1, "full trace: {observed:?}");
    match sql[0] {
        Request::Sql { rows, .. } => assert_eq!(*rows, Some(10)),
        other => panic!("expected Sql, got {other:?}"),
    }
    let more: Vec<&Request> = observed
        .iter()
        .filter(|r| matches!(r, Request::SqlMore { .. }))
        .collect();
    assert_eq!(more.len(), 1, "expected one sqlmore, got {observed:?}");
    match more[0] {
        Request::SqlMore { rows, .. } => assert_eq!(*rows, 10),
        other => panic!("expected SqlMore, got {other:?}"),
    }
}

/// Opening execute id from the recorder (PROTOCOL cursor handle).
#[cfg(feature = "rustls-tls")]
fn opening_sql_id(observed: &[mapepire::protocol::Request]) -> String {
    use mapepire::protocol::Request;
    observed
        .iter()
        .find_map(|r| match r {
            Request::Sql { id, .. }
            | Request::PrepareSqlExecute { id, .. }
            | Request::Execute { id, .. } => Some(id.clone()),
            _ => None,
        })
        .expect("opening execute request")
}

/// Live dialect: first page 100 rows, no `cont_id`, `is_done` omitted/false.
/// Stream must `sqlmore` with `cont_id` = opening request id and collect 101.
#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_stream_sqlmore_without_cont_id_uses_request_id() {
    use mapepire::ExecuteOptions;
    use mapepire::protocol::Request;

    let pages = vec![page(0, 100, None, false), page(100, 1, None, true)];
    let (job, recorder) = common::connect_to_mock_with_recorder(pages).await;

    let rows = job
        .execute_opts(
            "SELECT n FROM SCHEMA.NUMBERS",
            ExecuteOptions {
                rows: Some(100),
                terse: false,
            },
        )
        .await
        .expect("execute_opts");
    assert!(!rows.is_done());
    assert_eq!(rows.first_page_len(), 100);

    let all = rows.into_dynamic().await.expect("into_dynamic");
    assert_eq!(all.len(), 101);

    let observed = recorder.lock().expect("recorder mutex").clone();
    let sql_id = opening_sql_id(&observed);
    let more: Vec<&Request> = observed
        .iter()
        .filter(|r| matches!(r, Request::SqlMore { .. }))
        .collect();
    assert_eq!(more.len(), 1, "expected one sqlmore, got {observed:?}");
    match more[0] {
        Request::SqlMore {
            cont_id, rows: n, ..
        } => {
            assert_eq!(
                cont_id, &sql_id,
                "sqlmore.cont_id is the opening request id"
            );
            assert_eq!(*n, 100);
        }
        other => panic!("expected SqlMore, got {other:?}"),
    }
}

/// Live dialect: two pages of 100 with no `cont_id` collect 200.
#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_stream_two_pages_without_cont_id_collects_200() {
    use mapepire::ExecuteOptions;

    let pages = vec![page(0, 100, None, false), page(100, 100, None, true)];
    let (job, recorder) = common::connect_to_mock_with_recorder(pages).await;

    let rows = job
        .execute_opts(
            "SELECT n FROM SCHEMA.NUMBERS",
            ExecuteOptions {
                rows: Some(100),
                terse: false,
            },
        )
        .await
        .expect("execute_opts");
    let all = rows.into_dynamic().await.expect("into_dynamic");
    assert_eq!(all.len(), 200);
    let expected: Vec<i64> = (0..200).collect();
    let got: Vec<i64> = all.iter().map(|r| r.get::<i64>("n").expect("n")).collect();
    assert_eq!(got, expected);

    let observed = recorder.lock().expect("recorder mutex").clone();
    let sql_id = opening_sql_id(&observed);
    let more: Vec<_> = observed
        .iter()
        .filter(|r| matches!(r, mapepire::protocol::Request::SqlMore { .. }))
        .collect();
    assert_eq!(more.len(), 1, "expected one sqlmore, got {observed:?}");
    match more[0] {
        mapepire::protocol::Request::SqlMore { cont_id, .. } => {
            assert_eq!(cont_id, &sql_id);
        }
        other => panic!("expected SqlMore, got {other:?}"),
    }
}

/// `is_done: true` after one row (SYSDUMMY1) must not issue `sqlmore`.
#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_stream_skips_sqlmore_when_is_done_after_one_row() {
    use mapepire::protocol::Request;

    let pages = vec![page(0, 1, None, true)];
    let (job, recorder) = common::connect_to_mock_with_recorder(pages).await;

    let rows = job
        .execute("SELECT 1 FROM SYSIBM.SYSDUMMY1")
        .await
        .expect("execute");
    assert!(rows.is_done());
    assert_eq!(rows.first_page_len(), 1);
    let all = rows.into_dynamic().await.expect("into_dynamic");
    assert_eq!(all.len(), 1);

    let observed = recorder.lock().expect("recorder mutex").clone();
    assert!(
        !observed
            .iter()
            .any(|r| matches!(r, Request::SqlMore { .. })),
        "is_done: sqlmore must be skipped, got {observed:?}"
    );
    assert!(
        !observed
            .iter()
            .any(|r| matches!(r, Request::SqlClose { .. })),
        "is_done: sqlclose must be skipped, got {observed:?}"
    );
}

/// `is_done: true` after a full 3-row page (FETCH NEXT 3) must not `sqlmore`.
#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_stream_skips_sqlmore_when_is_done_after_full_small_page() {
    use mapepire::ExecuteOptions;
    use mapepire::protocol::Request;

    let pages = vec![page(0, 3, None, true)];
    let (job, recorder) = common::connect_to_mock_with_recorder(pages).await;

    let rows = job
        .execute_opts(
            "SELECT n FROM SCHEMA.NUMBERS FETCH NEXT 3 ROWS ONLY",
            ExecuteOptions {
                rows: Some(100),
                terse: false,
            },
        )
        .await
        .expect("execute_opts");
    let all = rows.into_dynamic().await.expect("into_dynamic");
    assert_eq!(all.len(), 3);

    let observed = recorder.lock().expect("recorder mutex").clone();
    assert!(
        !observed
            .iter()
            .any(|r| matches!(r, Request::SqlMore { .. })),
        "done after 3 rows: no sqlmore, got {observed:?}"
    );
}

/// Empty follow-up page without `is_done` is `Error::Internal` (no infinite poll).
#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_stream_empty_page_without_is_done_is_internal() {
    let pages = vec![page(0, 1, None, false), page(0, 0, None, false)];
    let job = common::connect_to_mock(common::MockBehavior::Pages {
        pages,
        recorder: None,
    })
    .await;

    let rows = job
        .execute("SELECT n FROM SCHEMA.NUMBERS")
        .await
        .expect("execute");
    let err = rows.into_dynamic().await.expect_err("empty page");
    assert!(
        matches!(err, mapepire::Error::Internal(_)),
        "expected Internal, got {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("empty page without is_done"),
        "unexpected message: {msg}"
    );
}
