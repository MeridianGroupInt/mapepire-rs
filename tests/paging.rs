//! Phase 6 integration test: paging via sqlmore.
//!
//! Spawns a mock with two-page `QueryResult` data: 50 rows on the initial
//! page (`is_done` = false, `cont_id` = Some), 50 more on the next, `is_done` = true
//! on the second. Verifies `Rows::stream()` yields all 100 rows in order
//! across the page boundary.

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
        },
        data,
        cont_id: cont_id.map(str::to_string),
        is_done,
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
