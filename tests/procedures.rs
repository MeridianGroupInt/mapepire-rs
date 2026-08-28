//! CALL / OUT parameters (OSS-5).
//!
//! Stored-procedure calls use `prepare_sql_execute` (no CALL opcode). The
//! daemon returns `output_parms` and `metadata.parameters`; serde used to
//! drop them. QCMDEXC as SQL is ordinary `Job::execute` — no helper.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use mapepire::{ParameterDetail, ParameterResult, QueryMetaData, QueryResult};
#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;
#[cfg(feature = "rustls-tls")]
use serde_json::{Value, json};

#[cfg(feature = "rustls-tls")]
fn integer_detail(name: &str, mode: &str) -> ParameterDetail {
    ParameterDetail {
        type_name: "INTEGER".into(),
        mode: mode.into(),
        precision: 10,
        scale: Some(0),
        name: name.into(),
    }
}

#[cfg(feature = "rustls-tls")]
fn integer_result(index: u32, name: &str, value: Option<Value>) -> ParameterResult {
    ParameterResult {
        index,
        type_name: "INTEGER".into(),
        precision: 10,
        scale: Some(0),
        name: name.into(),
        ccsid: None,
        value,
    }
}

/// JS `procedures.test.ts` integer contract: IN/INOUT/OUT `[6, 4, 0]`
/// → output values `None` / `0` / `10`.
#[cfg(feature = "rustls-tls")]
fn call_query_result(id: &str) -> QueryResult {
    QueryResult {
        id: id.to_string(),
        success: true,
        has_results: false,
        update_count: 0,
        cont_id: None,
        is_done: true,
        metadata: QueryMetaData {
            column_count: 0,
            columns: vec![],
            job: None,
            parameters: vec![
                integer_detail("P1", "IN"),
                integer_detail("P2", "INOUT"),
                integer_detail("P3", "OUT"),
            ],
        },
        data: vec![],
        execution_time: 5.0,
        error: None,
        sqlcode: None,
        sqlstate: None,
        parameter_count: Some(3),
        output_parms: vec![
            integer_result(1, "P1", None),
            integer_result(2, "P2", Some(json!(0))),
            integer_result(3, "P3", Some(json!(10))),
        ],
    }
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_execute_with_call_returns_output_parms() {
    use mapepire::protocol::Request;

    let (job, recorder) =
        common::connect_to_mock_with_recorder(vec![call_query_result("placeholder")]).await;

    let rows = job
        .execute_with(
            "CALL SCHEMA.PROCEDURE_TEST(?, ?, ?)",
            &[json!(6), json!(4), json!(0)],
        )
        .await
        .expect("CALL execute_with");

    assert!(!rows.has_results(), "CALL has no result set");
    assert_eq!(rows.update_count(), Some(0));
    assert_eq!(rows.parameter_count(), Some(3));
    assert_eq!(
        rows.parameter_metadata()
            .iter()
            .map(|p| (p.name.as_str(), p.mode.as_str()))
            .collect::<Vec<_>>(),
        [("P1", "IN"), ("P2", "INOUT"), ("P3", "OUT")]
    );
    let values: Vec<Option<Value>> = rows
        .output_parms()
        .iter()
        .map(|p| p.value.clone())
        .collect();
    assert_eq!(values, vec![None, Some(json!(0)), Some(json!(10))]);

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
            assert!(sql.to_ascii_uppercase().contains("CALL"));
            assert_eq!(
                parameters.as_ref(),
                Some(&vec![vec![json!(6), json!(4), json!(0)]])
            );
        }
        other => panic!("expected PrepareSqlExecute, got {other:?}"),
    }
    let json = serde_json::to_string(bound[0]).expect("serialize bound request");
    assert!(
        json.contains(r#""parameters":[6,4,0]"#),
        "single-set CALL parameters must flatten to [6,4,0], got {json}"
    );
    assert!(
        !json.contains(r#""parameters":[[6,4,0]]"#),
        "single-set must not nest as [[6,4,0]], got {json}"
    );
}
