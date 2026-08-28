//! OSS-6 dest tokens + OSS-9 live `gettracedata`.
//!
//! Live Jetty rejects `tracedest: ""` (`No enum constant Tracer.Dest`).
//! `Job::set_trace(level)` defaults dest to `IN_MEM`. Live `gettracedata`
//! is untagged `{id, success, tracedata}` — not Pong.

#[cfg(feature = "rustls-tls")]
mod common;

#[cfg(feature = "rustls-tls")]
use mapepire::protocol::Request;
#[cfg(feature = "rustls-tls")]
use mapepire::{TraceDest, TraceLevel};
#[cfg(feature = "rustls-tls")]
use pretty_assertions::assert_eq;

#[cfg(feature = "rustls-tls")]
fn last_setconfig(recorder: &common::RequestRecorder) -> Request {
    let observed = recorder.lock().expect("recorder mutex").clone();
    let configs: Vec<&Request> = observed
        .iter()
        .filter(|r| matches!(r, Request::SetConfig { .. }))
        .collect();
    assert_eq!(configs.len(), 1, "full trace: {observed:?}");
    configs[0].clone()
}

#[cfg(feature = "rustls-tls")]
fn assert_setconfig_json(req: &Request, level: &str, dest: &str) {
    let json = serde_json::to_string(req).expect("serialize SetConfig");
    assert!(
        json.contains(&format!(r#""tracelevel":"{level}""#)),
        "expected tracelevel {level}, got {json}"
    );
    assert!(
        json.contains(&format!(r#""tracedest":"{dest}""#)),
        "expected tracedest {dest}, got {json}"
    );
    assert!(
        !json.contains(r#""tracedest":"""#),
        "empty tracedest is not a Tracer.Dest, got {json}"
    );
    assert!(
        !json.contains(r#""tracelevel":"ALL""#),
        "TraceLevel::All must send ON not ALL, got {json}"
    );
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_set_trace_sends_in_mem_never_empty_dest() {
    let (job, recorder) = common::connect_to_mock_with_recorder(vec![]).await;
    job.set_trace(TraceLevel::Off).await.expect("set_trace");
    let req = last_setconfig(&recorder);
    match &req {
        Request::SetConfig {
            tracelevel,
            tracedest,
            ..
        } => {
            assert_eq!(tracelevel, "OFF");
            assert_eq!(tracedest, "IN_MEM");
        }
        other => panic!("expected SetConfig, got {other:?}"),
    }
    assert_setconfig_json(&req, "OFF", "IN_MEM");
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_set_trace_all_sends_on_not_all() {
    let (job, recorder) = common::connect_to_mock_with_recorder(vec![]).await;
    job.set_trace(TraceLevel::All).await.expect("set_trace All");
    let req = last_setconfig(&recorder);
    match &req {
        Request::SetConfig {
            tracelevel,
            tracedest,
            ..
        } => {
            assert_eq!(tracelevel, "ON");
            assert_eq!(tracedest, "IN_MEM");
        }
        other => panic!("expected SetConfig, got {other:?}"),
    }
    assert_setconfig_json(&req, "ON", "IN_MEM");
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_set_trace_config_file() {
    let (job, recorder) = common::connect_to_mock_with_recorder(vec![]).await;
    job.set_trace_config(TraceDest::File, TraceLevel::Errors)
        .await
        .expect("set_trace_config FILE");
    let req = last_setconfig(&recorder);
    match &req {
        Request::SetConfig {
            tracelevel,
            tracedest,
            ..
        } => {
            assert_eq!(tracelevel, "ERRORS");
            assert_eq!(tracedest, "FILE");
        }
        other => panic!("expected SetConfig, got {other:?}"),
    }
    assert_setconfig_json(&req, "ERRORS", "FILE");
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_fetch_trace_untagged_tracedata_hello() {
    let job = common::connect_to_mock(common::MockBehavior::AcceptAndConnect).await;
    let trace = job.fetch_trace().await.expect("fetch_trace");
    assert_eq!(trace, "hello");
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_fetch_trace_omitted_tracedata_remaps_empty() {
    let job = common::connect_to_mock(common::MockBehavior::GetTraceAsPong).await;
    let trace = job.fetch_trace().await.expect("fetch_trace remap");
    assert_eq!(trace, "");
}

#[cfg(feature = "rustls-tls")]
#[tokio::test]
async fn test_ping_untagged_without_tracedata_stays_pong() {
    let job = common::connect_to_mock(common::MockBehavior::AcceptAndConnect).await;
    job.ping().await.expect("ping");
    let trace = job.fetch_trace().await.expect("fetch_trace after ping");
    assert_eq!(trace, "hello");
}
