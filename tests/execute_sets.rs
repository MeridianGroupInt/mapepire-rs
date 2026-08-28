//! OSS-12: one-shot `prepare_sql_execute` with 2-D `parameters`.
//!
//! [`mapepire::Query::execute_batch`] stays sequential. Single-set
//! [`mapepire::Job::execute_with`] still flattens `[7]`.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use mapepire::protocol::Request;
#[cfg(feature = "rustls-tls")]
use mapepire::{Error, ExecuteOptions, ProtocolError, QueryMetaData, QueryResult};
#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;
#[cfg(feature = "rustls-tls")]
use serde_json::json;

#[cfg(feature = "rustls-tls")]
fn dml_qr(count: i64) -> QueryResult {
    QueryResult {
        id: "placeholder".into(),
        success: true,
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
fn last_prepare_sql_execute(recorder: &common::RequestRecorder) -> Request {
    let observed = recorder.lock().expect("recorder mutex").clone();
    let bound: Vec<&Request> = observed
        .iter()
        .filter(|r| matches!(r, Request::PrepareSqlExecute { .. }))
        .collect();
    assert_eq!(bound.len(), 1, "full trace: {observed:?}");
    bound[0].clone()
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_execute_sets_one_prepare_sql_execute_nested() {
    let (job, recorder) = common::connect_to_mock_with_recorder(vec![dml_qr(2)]).await;
    let sets = vec![vec![json!(1), json!("a")], vec![json!(2), json!("b")]];
    let rows = job
        .execute_sets(
            "INSERT INTO T VALUES(?,?)",
            &sets,
            ExecuteOptions::default(),
        )
        .await
        .expect("execute_sets");
    assert_eq!(rows.update_count(), Some(2));

    let req = last_prepare_sql_execute(&recorder);
    match &req {
        Request::PrepareSqlExecute {
            parameters, rows, ..
        } => {
            assert_eq!(
                parameters.as_ref(),
                Some(&vec![
                    vec![json!(1), json!("a")],
                    vec![json!(2), json!("b")]
                ])
            );
            assert_eq!(*rows, Some(100));
        }
        other => panic!("expected PrepareSqlExecute, got {other:?}"),
    }
    let json = serde_json::to_string(&req).expect("serialize");
    assert!(
        json.contains(r#""parameters":[[1,"a"],[2,"b"]]"#),
        "two-set batch must stay nested, got {json}"
    );
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_execute_sets_one_set_still_flattens() {
    let (job, recorder) = common::connect_to_mock_with_recorder(vec![dml_qr(1)]).await;
    let sets = vec![vec![json!(7)]];
    let rows = job
        .execute_sets(
            "VALUES (CAST(? AS INTEGER))",
            &sets,
            ExecuteOptions::default(),
        )
        .await
        .expect("execute_sets single");
    assert_eq!(rows.update_count(), Some(1));

    let req = last_prepare_sql_execute(&recorder);
    let json = serde_json::to_string(&req).expect("serialize");
    assert!(
        json.contains(r#""parameters":[7]"#),
        "single-set must flatten to [7], got {json}"
    );
    assert!(
        !json.contains(r#""parameters":[[7]]"#),
        "single-set must not nest as [[7]], got {json}"
    );
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_execute_sets_empty_outer_is_protocol_error() {
    let job = common::connect_to_mock(common::MockBehavior::AcceptAndConnect).await;
    let err = job
        .execute_sets("INSERT INTO T VALUES(?)", &[], ExecuteOptions::default())
        .await
        .expect_err("empty outer");
    assert!(matches!(
        err,
        Error::Protocol(ProtocolError::EmptyParameterSets)
    ));
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_query_execute_sets_one_shot() {
    let (job, recorder) = common::connect_to_mock_with_recorder(vec![dml_qr(2)]).await;
    let query = job
        .prepare("INSERT INTO T VALUES(?,?)")
        .await
        .expect("prepare");
    let sets = vec![vec![json!(1), json!("a")], vec![json!(2), json!("b")]];
    let rows = query
        .execute_sets(&sets, ExecuteOptions::default())
        .await
        .expect("query execute_sets");
    assert_eq!(rows.update_count(), Some(2));

    let req = last_prepare_sql_execute(&recorder);
    let json = serde_json::to_string(&req).expect("serialize");
    assert!(
        json.contains(r#""parameters":[[1,"a"],[2,"b"]]"#),
        "query execute_sets must send one nested batch, got {json}"
    );
}

/// Live prepare ack has no `cont_id`; `execute_sets` then gets a `Pong`
/// from [`common::MockBehavior::AcceptAndConnect`] (unexpected variant).
#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_query_execute_sets_unexpected_is_protocol() {
    let job = common::connect_to_mock(common::MockBehavior::AcceptAndConnect).await;
    let query = job
        .prepare("INSERT INTO T VALUES(?)")
        .await
        .expect("prepare");
    let err = query
        .execute_sets(&[vec![json!(1)]], ExecuteOptions::default())
        .await
        .expect_err("pong is not QueryResult");
    assert!(
        matches!(err, Error::Protocol(_)),
        "expected Protocol, got {err:?}"
    );
}
