//! OSS-11: live `dove` replies (`vedata` / `vemetadata`) and `run: true`.
//!
//! SQLSTATE 42505 (no Visual Explain authority) is
//! [`mapepire::Error::Server`], not a crate protocol failure.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use mapepire::protocol::Request;
#[cfg(feature = "rustls-tls")]
use mapepire::{Error, ErrorResponse};
#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;
#[cfg(feature = "rustls-tls")]
use serde_json::json;

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_visual_explain_untagged_vedata() {
    let (job, recorder) = common::connect_to_mock_with_recorder(vec![]).await;
    let plan = job
        .visual_explain("SELECT 1 FROM SYSIBM.SYSDUMMY1")
        .await
        .expect("visual_explain");
    assert_eq!(plan, json!([{"op": "TBSCAN"}]));

    let observed = recorder.lock().expect("recorder mutex").clone();
    let dove: Vec<&Request> = observed
        .iter()
        .filter(|r| matches!(r, Request::Dove { .. }))
        .collect();
    assert_eq!(dove.len(), 1, "full trace: {observed:?}");
    match dove[0] {
        Request::Dove { run, rows, sql, .. } => {
            assert_eq!(*run, Some(true), "JS ExplainType.RUN default");
            assert!(rows.is_none());
            assert!(sql.contains("SELECT"));
        }
        other => panic!("expected Dove, got {other:?}"),
    }
    let json = serde_json::to_string(dove[0]).expect("serialize dove");
    assert!(json.contains(r#""run":true"#), "missing run:true in {json}");
    assert!(
        json.contains(r#""type":"dove""#),
        "missing type dove in {json}"
    );
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_visual_explain_42505_is_server() {
    let job = common::connect_to_mock(common::MockBehavior::ReturnError(ErrorResponse {
        id: "placeholder".into(),
        success: false,
        sqlstate: Some("42505".into()),
        sqlcode: None,
        error: Some("not authorized to visual explain".into()),
        job: None,
    }))
    .await;

    let err = job
        .visual_explain("SELECT 1 FROM SYSIBM.SYSDUMMY1")
        .await
        .expect_err("42505 must be Server");
    match err {
        Error::Server(s) => {
            assert_eq!(s.sqlstate.as_deref(), Some("42505"));
            assert!(
                s.message.contains("not authorized"),
                "unexpected message: {}",
                s.message
            );
        }
        other => panic!("expected Error::Server, got {other:?}"),
    }
}
