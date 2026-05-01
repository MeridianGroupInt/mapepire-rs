//! Phase 6 integration test: SQL one-shot SELECT and DML paths.
//!
//! Two tests against `MockBehavior::Pages`:
//! - SELECT yields a `Rows` with `has_results() == true`, `update_count() == None`, and the canned
//!   row data is recoverable via `into_dynamic()` / `Row::get`.
//! - DML yields `update_count() == Some(3)` and `has_results() == false`.
//!
//! Per-item `#[cfg(feature = "rustls-tls")]` gating; mock harness is rustls-only.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;

#[cfg(feature = "rustls-tls")]
fn select_query_result(id: &str) -> mapepire::QueryResult {
    use mapepire::{Column, QueryMetaData, QueryResult};
    use serde_json::{Map, Value, json};

    let mut row: Map<String, Value> = Map::new();
    row.insert("name".into(), json!("Alice"));
    row.insert("age".into(), json!(30));

    QueryResult {
        id: id.to_string(),
        success: true,
        has_results: true,
        // -1 = N/A sentinel for SELECT (see Rows::update_count, which
        // maps any negative value to None).
        update_count: -1,
        metadata: QueryMetaData {
            column_count: 2,
            columns: vec![
                Column {
                    name: "name".into(),
                    label: Some("name".into()),
                    type_name: Some("VARCHAR".into()),
                    display_size: Some(50),
                    scale: Some(0),
                    precision: Some(50),
                },
                Column {
                    name: "age".into(),
                    label: Some("age".into()),
                    type_name: Some("INTEGER".into()),
                    display_size: Some(10),
                    scale: Some(0),
                    precision: Some(10),
                },
            ],
        },
        data: vec![row],
        cont_id: None,
        is_done: true,
        execution_time: 5.0,
    }
}

#[cfg(feature = "rustls-tls")]
fn dml_query_result(id: &str, count: i64) -> mapepire::QueryResult {
    use mapepire::{QueryMetaData, QueryResult};

    QueryResult {
        id: id.to_string(),
        success: true,
        has_results: false,
        update_count: count,
        metadata: QueryMetaData {
            column_count: 0,
            columns: vec![],
        },
        data: vec![],
        cont_id: None,
        is_done: true,
        execution_time: 2.0,
    }
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_select_returns_rows_with_data() {
    // The mock writes the request id into the QueryResult.id at response time,
    // so we can give a placeholder id here.
    let job = common::connect_to_mock(common::MockBehavior::Pages(vec![select_query_result(
        "placeholder",
    )]))
    .await;

    let rows = job
        .execute("SELECT name, age FROM USERS")
        .await
        .expect("execute SELECT");
    assert!(
        rows.has_results(),
        "SELECT should report has_results = true"
    );
    assert_eq!(
        rows.update_count(),
        None,
        "SELECT update_count should be None"
    );

    let dyn_rows = rows.into_dynamic().await.expect("into_dynamic");
    assert_eq!(dyn_rows.len(), 1, "should have exactly 1 row");
    let row = &dyn_rows[0];
    let name: String = row.get("name").expect("name column");
    let age: i64 = row.get("age").expect("age column");
    assert_eq!(name, "Alice");
    assert_eq!(age, 30);
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_dml_returns_update_count() {
    let job = common::connect_to_mock(common::MockBehavior::Pages(vec![dml_query_result(
        "placeholder",
        3,
    )]))
    .await;

    let rows = job
        .execute("INSERT INTO USERS VALUES ('Bob', 25)")
        .await
        .expect("execute INSERT");
    assert!(!rows.has_results(), "DML should report has_results = false");
    assert_eq!(
        rows.update_count(),
        Some(3),
        "DML update_count should be Some(3)"
    );
    // No rows to page (has_results=false, data=[]); paging coverage lives
    // in tests/paging.rs (Task 26).
}
