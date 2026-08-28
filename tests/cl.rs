//! Live-dialect CL: untagged `QueryResult` job log, including CPF0006 failure.
//!
//! mapepire-js `SQLJob.clcommand` does not throw on `success: false` with
//! `data`; `Job::cl` returns [`mapepire::ClOutcome`] the same way.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use mapepire::{DaemonServer, Job, QueryResult, TlsConfig};
#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;
#[cfg(feature = "rustls-tls")]
use serde_json::{Map, Value, json};

#[cfg(feature = "rustls-tls")]
fn job_log_row(
    message_id: &str,
    severity: Value,
    message_type: &str,
    text: &str,
) -> Map<String, Value> {
    let mut row = Map::new();
    row.insert("MESSAGE_ID".into(), json!(message_id));
    row.insert("SEVERITY".into(), severity);
    row.insert(
        "MESSAGE_TIMESTAMP".into(),
        json!("2026-08-27-12.00.00.000000"),
    );
    row.insert("FROM_LIBRARY".into(), json!("QSYS"));
    row.insert("FROM_PROGRAM".into(), json!("QCAEXEC"));
    row.insert("MESSAGE_TYPE".into(), json!(message_type));
    row.insert("MESSAGE_TEXT".into(), json!(text));
    row.insert("MESSAGE_SECOND_LEVEL_TEXT".into(), json!("Cause . . . ."));
    row
}

#[cfg(feature = "rustls-tls")]
fn cl_success_result() -> QueryResult {
    QueryResult {
        id: "placeholder".into(),
        success: true,
        has_results: true,
        update_count: -1,
        cont_id: None,
        is_done: true,
        metadata: mapepire::QueryMetaData::default(),
        data: vec![job_log_row(
            "CPC2102",
            json!("0"),
            "COMPLETION",
            "Library QGPL displayed.",
        )],
        execution_time: 12.0,
        error: None,
        sqlcode: None,
        sqlstate: None,
    }
}

#[cfg(feature = "rustls-tls")]
fn cl_failure_result() -> QueryResult {
    QueryResult {
        id: "placeholder".into(),
        success: false,
        has_results: true,
        update_count: -1,
        cont_id: None,
        is_done: true,
        metadata: mapepire::QueryMetaData::default(),
        data: vec![job_log_row(
            "CPF0006",
            json!(40),
            "ESCAPE",
            "[CPF0006] Errors occurred in command.",
        )],
        execution_time: 8.0,
        error: Some("[CPF0006] Errors occurred in command.".into()),
        sqlcode: Some(-443),
        sqlstate: Some("38501".into()),
    }
}

#[cfg(feature = "rustls-tls")]
async fn connect_cl(behavior: common::MockBehavior) -> Job {
    let (addr, cert_der) = common::spawn_mock(behavior);
    let server = DaemonServer::builder()
        .host(addr.ip().to_string())
        .port(addr.port())
        .user("USER")
        .password(common::dummy_password())
        .tls(TlsConfig::Ca(cert_der))
        .build()
        .expect("test builder fields all set");
    Job::connect(&server)
        .await
        .expect("Job::connect against mock")
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_cl_untagged_success_returns_job_log_entries() {
    let job = connect_cl(common::MockBehavior::ClThen {
        result: cl_success_result(),
    })
    .await;

    let outcome = job.cl("DSPLIB QGPL").await.expect("cl success");
    assert!(outcome.success);
    assert!(outcome.error.is_none());
    assert_eq!(outcome.entries.len(), 1);
    assert_eq!(outcome.entries[0].message_id.as_deref(), Some("CPC2102"));
    assert_eq!(outcome.entries[0].severity.as_deref(), Some("0"));
    assert_eq!(
        outcome.entries[0].message_text.as_deref(),
        Some("Library QGPL displayed.")
    );
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_cl_untagged_cpf0006_is_ok_with_job_log() {
    let job = connect_cl(common::MockBehavior::ClThen {
        result: cl_failure_result(),
    })
    .await;

    let outcome = job
        .cl("INVALIDCOMMAND")
        .await
        .expect("failed CL is Ok, not Err");
    assert!(!outcome.success);
    assert_eq!(
        outcome.error.as_deref(),
        Some("[CPF0006] Errors occurred in command.")
    );
    assert_eq!(outcome.sqlcode, Some(-443));
    assert_eq!(outcome.sqlstate.as_deref(), Some("38501"));
    assert_eq!(outcome.entries.len(), 1);
    assert_eq!(outcome.entries[0].message_id.as_deref(), Some("CPF0006"));
    assert_eq!(outcome.entries[0].severity.as_deref(), Some("40"));

    job.ping().await.expect("dispatcher still alive after CL");
}
