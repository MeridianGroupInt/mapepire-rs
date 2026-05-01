//! Phase 6 integration test: prepared statement happy path.
//!
//! Two tests against `MockBehavior::PrepareAndExecute`:
//! - `Job::prepare(sql)` returns a `Query` with the mock's `cont_id`; `Query::execute_with(&ids,
//!   &params)` returns `Rows` with the canned `update_count`.
//! - `Query::execute_batch(&ids, &[&params...])` returns `Vec<Rows>` with one entry per batch.
//!
//! Per-item `#[cfg(feature = "rustls-tls")]` gating; mock harness is rustls-only.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;

#[cfg(feature = "rustls-tls")]
fn dml_qr(id: &str, count: i64) -> mapepire::QueryResult {
    use mapepire::{QueryMetaData, QueryResult};

    QueryResult {
        id: id.to_string(),
        success: true,
        // execution_time is not under test; 0.0 is a placeholder.
        execution_time: 0.0,
        has_results: false,
        update_count: count,
        metadata: QueryMetaData {
            column_count: 0,
            columns: vec![],
        },
        data: vec![],
        cont_id: None,
        is_done: true,
    }
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_prepare_then_execute() {
    use serde_json::json;

    let job = common::connect_to_mock(common::MockBehavior::PrepareAndExecute {
        cont_id: "stmt-1".to_string(),
        results: vec![dml_qr("placeholder", 1)],
    })
    .await;

    let query = job
        .prepare("INSERT INTO T VALUES(?,?)")
        .await
        .expect("prepare");
    let rows = query
        .execute_with(job.ids(), &[json!(1), json!("a")])
        .await
        .expect("execute_with");

    assert_eq!(rows.update_count(), Some(1));
    assert!(!rows.has_results());
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_execute_batch() {
    use serde_json::json;

    let job = common::connect_to_mock(common::MockBehavior::PrepareAndExecute {
        cont_id: "stmt-batch".to_string(),
        results: vec![dml_qr("placeholder", 1), dml_qr("placeholder", 1)],
    })
    .await;

    let query = job
        .prepare("INSERT INTO T VALUES(?,?)")
        .await
        .expect("prepare");
    let batches: &[&[serde_json::Value]] = &[&[json!(1), json!("a")], &[json!(2), json!("b")]];
    let results = query
        .execute_batch(job.ids(), batches)
        .await
        .expect("execute_batch");

    assert_eq!(
        results.len(),
        2,
        "execute_batch should return one Rows per batch"
    );
    for rows in &results {
        assert_eq!(rows.update_count(), Some(1));
        assert!(!rows.has_results());
    }
}
