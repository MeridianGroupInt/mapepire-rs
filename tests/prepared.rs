//! Phase 6 integration test: prepared statement happy path.
//!
//! Two tests against `MockBehavior::PrepareAndExecute`:
//! - `Job::prepare(sql)` returns a `Query` with the mock's `cont_id`;
//!   `Query::execute_with(&params)` returns `Rows` with the canned `update_count`.
//! - `Query::execute_batch(&[&params...])` returns `Vec<Rows>` with one entry per batch.
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
            job: None,
            parameters: vec![],
        },
        data: vec![],
        cont_id: None,
        is_done: true,
        error: None,
        sqlcode: None,
        sqlstate: None,
        parameter_count: None,
        output_parms: vec![],
    }
}

#[cfg(feature = "rustls-tls")]
fn id_prefix(id: &str) -> &str {
    id.split_once('-').map_or(id, |(p, _)| p)
}

#[cfg(feature = "rustls-tls")]
fn recorded_sql_ids(observed: &[mapepire::protocol::Request]) -> Vec<String> {
    use mapepire::protocol::Request;
    observed
        .iter()
        .filter_map(|r| match r {
            Request::PrepareSql { id, .. }
            | Request::PrepareSqlExecute { id, .. }
            | Request::Execute { id, .. }
            | Request::Sql { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
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
        .execute_with(&[json!(1), json!("a")])
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
    let results = query.execute_batch(batches).await.expect("execute_batch");

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

/// `Query::execute` / `execute_opts` / `execute_with_opts` with a server
/// handle (`PrepareAndExecute` returns `cont_id`) send `execute` with the
/// requested page size.
#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_query_execute_opts_with_cont_id() {
    use mapepire::{Error, ExecuteOptions, ProtocolError};
    use serde_json::json;

    let job = common::connect_to_mock(common::MockBehavior::PrepareAndExecute {
        cont_id: "stmt-opts".to_string(),
        results: vec![
            dml_qr("placeholder", 1),
            dml_qr("placeholder", 2),
            dml_qr("placeholder", 3),
        ],
    })
    .await;

    let query = job
        .prepare("INSERT INTO T VALUES(?,?)")
        .await
        .expect("prepare");

    let err = query
        .execute_opts(ExecuteOptions {
            rows: Some(0),
            terse: false,
        })
        .await
        .expect_err("rows: 0 must not be sent");
    assert!(
        matches!(err, Error::Protocol(ProtocolError::ZeroPageSize)),
        "unexpected error: {err}"
    );

    let unbound = query.execute().await.expect("Query::execute");
    assert_eq!(unbound.update_count(), Some(1));

    let opts = query
        .execute_opts(ExecuteOptions {
            rows: Some(10),
            terse: false,
        })
        .await
        .expect("Query::execute_opts");
    assert_eq!(opts.update_count(), Some(2));

    let bound = query
        .execute_with_opts(
            &[json!(1), json!("a")],
            ExecuteOptions {
                rows: Some(25),
                terse: false,
            },
        )
        .await
        .expect("Query::execute_with_opts");
    assert_eq!(bound.update_count(), Some(3));
}

/// A `Query` prepared on job A issues ids from A's allocator, not job B's.
#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_query_uses_originating_job_allocator() {
    use serde_json::json;

    let (job_a, rec_a) =
        common::connect_to_mock_with_recorder(vec![dml_qr("placeholder", 1)]).await;
    let (job_b, rec_b) =
        common::connect_to_mock_with_recorder(vec![dml_qr("placeholder", 1)]).await;

    let query = job_a
        .prepare("INSERT INTO T VALUES(?,?)")
        .await
        .expect("prepare on A");
    drop(
        query
            .execute_with(&[json!(1), json!("a")])
            .await
            .expect("execute on Query from A"),
    );
    drop(
        job_b
            .execute("SELECT 1 FROM SYSIBM.SYSDUMMY1")
            .await
            .expect("execute on B"),
    );

    let a_ids = recorded_sql_ids(&rec_a.lock().expect("rec_a").clone());
    let b_ids = recorded_sql_ids(&rec_b.lock().expect("rec_b").clone());
    assert!(a_ids.len() >= 2, "prepare + execute on A, got {a_ids:?}");
    assert!(!b_ids.is_empty(), "B must send sql, got {b_ids:?}");
    let a_prefix = id_prefix(&a_ids[0]);
    for id in &a_ids {
        assert_eq!(
            id_prefix(id),
            a_prefix,
            "Query from A must use A's allocator, got {a_ids:?}"
        );
    }
    assert_ne!(
        id_prefix(&b_ids[0]),
        a_prefix,
        "job B must have a distinct allocator prefix"
    );
}
