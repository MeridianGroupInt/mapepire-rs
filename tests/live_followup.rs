//! Live-daemon follow-up (OSS-1): bind, prepare, ping, getdbjob, dispatcher survival.
//!
//! 0.6.0 connected and ran unbound SELECT. Parameterized `execute_with`,
//! `prepare`, and untagged ping still failed on a live Jetty Mapepire
//! daemon. Two of those decode misses killed the dispatcher.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use mapepire::{Column, QueryMetaData, QueryResult};
#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;
#[cfg(feature = "rustls-tls")]
use serde_json::{Map, Value, json};

#[cfg(feature = "rustls-tls")]
fn int_row(id: &str, value: i64) -> QueryResult {
    let mut row: Map<String, Value> = Map::new();
    row.insert("1".into(), json!(value));
    QueryResult {
        id: id.to_string(),
        success: true,
        has_results: true,
        update_count: -1,
        metadata: QueryMetaData {
            column_count: 1,
            columns: vec![Column {
                name: "1".into(),
                label: Some("1".into()),
                type_name: Some("INTEGER".into()),
                display_size: Some(11),
                precision: Some(10),
                scale: Some(0),
            }],
            job: None,
        },
        data: vec![row],
        cont_id: None,
        is_done: true,
        execution_time: 1.0,
    }
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_execute_with_sends_prepare_sql_execute() {
    use mapepire::protocol::Request;

    let (job, recorder) =
        common::connect_to_mock_with_recorder(vec![int_row("placeholder", 7)]).await;

    let rows = job
        .execute_with("VALUES (CAST(? AS INTEGER))", &[json!(7)])
        .await
        .expect("execute_with");
    let dyn_rows = rows.into_dynamic().await.expect("into_dynamic");
    assert_eq!(dyn_rows.len(), 1);
    let got: i64 = dyn_rows[0].get("1").expect("column 1");
    assert_eq!(got, 7);

    let observed = recorder.lock().expect("recorder mutex").clone();
    let bound: Vec<&Request> = observed
        .iter()
        .filter(|r| matches!(r, Request::PrepareSqlExecute { .. }))
        .collect();
    assert_eq!(bound.len(), 1, "full trace: {observed:?}");
    match bound[0] {
        Request::PrepareSqlExecute {
            sql, parameters, ..
        } => {
            assert!(sql.contains('?'));
            assert_eq!(parameters.as_ref(), Some(&vec![vec![json!(7)]]));
        }
        other => panic!("expected PrepareSqlExecute, got {other:?}"),
    }
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_prepare_then_execute_twice() {
    let job = common::connect_to_mock(common::MockBehavior::PrepareAndExecute {
        cont_id: "stmt-live".to_string(),
        results: vec![int_row("placeholder", 7), int_row("placeholder", 11)],
    })
    .await;

    let query = job
        .prepare("VALUES (CAST(? AS INTEGER))")
        .await
        .expect("prepare");
    let first = query
        .execute_with(job.ids(), &[json!(7)])
        .await
        .expect("execute 7");
    let second = query
        .execute_with(job.ids(), &[json!(11)])
        .await
        .expect("execute 11");

    let a = first.into_dynamic().await.expect("first");
    let b = second.into_dynamic().await.expect("second");
    let v7: i64 = a[0].get("1").expect("7");
    let v11: i64 = b[0].get("1").expect("11");
    assert_eq!(v7, 7);
    assert_eq!(v11, 11);
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_ping_then_sql_on_same_job() {
    let job = common::connect_to_mock(common::MockBehavior::Pages {
        pages: vec![int_row("placeholder", 1)],
        recorder: None,
    })
    .await;

    job.ping().await.expect("ping");
    let rows = job
        .execute("SELECT 1 FROM SYSIBM.SYSDUMMY1")
        .await
        .expect("sql after ping");
    drop(rows);
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_getdbjob_and_getversion_after_ping() {
    let job = common::connect_to_mock(common::MockBehavior::AcceptAndConnect).await;

    job.ping().await.expect("ping");
    let version = job.server_version().await.expect("getversion");
    let db_job = job.db_job_name().await.expect("getdbjob");
    assert_eq!(version, "0.0.0-mock");
    assert_eq!(db_job, "MOCK/QUSER/000001");
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_unknown_frame_does_not_kill_dispatcher() {
    let job = common::connect_to_mock(common::MockBehavior::UnknownTypeThenPages {
        pages: vec![int_row("placeholder", 1)],
    })
    .await;

    let err = job.ping().await.expect_err("unknown type must fail ping");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown response type") || msg.contains("protocol"),
        "unexpected error: {msg}"
    );

    let rows = job
        .execute("SELECT 1 FROM SYSIBM.SYSDUMMY1")
        .await
        .expect("sql after decode miss");
    drop(rows);
}
