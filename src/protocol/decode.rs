//! Decode Mapepire daemon frames.
//!
//! Live daemons emit untagged `{id, success, ...}` objects. The v0.1–v0.5
//! mock dialect was internally tagged on `"type"`. We accept both.

use serde::de::{DeserializeOwned, Error as _};
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::response::{ClMessage, ErrorResponse, QueryResult, Response};

impl<'de> Deserialize<'de> for Response {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        decode_value(value).map_err(D::Error::custom)
    }
}

/// Decode a JSON value as a [`Response`].
///
/// Accepts internally tagged mock frames (`type` present) and untagged live
/// daemon frames. Returns an error string when the value is not a recognized
/// response object.
pub(crate) fn decode_value(value: Value) -> Result<Response, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "response is not a JSON object".to_string())?;

    if obj.contains_key("type") {
        return decode_tagged(value);
    }

    let success = obj.get("success").and_then(Value::as_bool);
    if success == Some(false) {
        let err: ErrorResponse = serde_json::from_value(value).map_err(|e| e.to_string())?;
        return Ok(Response::Error(err));
    }

    let has_data = obj.contains_key("data");
    let has_results_key = obj.contains_key("has_results");
    // Live `prepare_sql` success is untagged `{id, success, cont_id, execution_time}`
    // and often `is_done`/`metadata` without `has_results` or `data`. Those keys
    // used to take the QueryResult branch and fail serde.
    if obj.contains_key("cont_id") && !has_data && !has_results_key {
        let body: PreparedStatementBody =
            serde_json::from_value(value).map_err(|e| e.to_string())?;
        return Ok(Response::PreparedStatement {
            id: body.id,
            success: body.success,
            cont_id: body.cont_id,
            execution_time: body.execution_time,
        });
    }

    let looks_like_result = has_data || has_results_key;
    if looks_like_result {
        let q: QueryResult = serde_json::from_value(value).map_err(|e| e.to_string())?;
        return Ok(Response::QueryResult(q));
    }

    if obj.contains_key("job") {
        let id = obj
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let job = obj
            .get("job")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let version = obj
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        return Ok(Response::Connected { id, version, job });
    }

    if obj.contains_key("version") {
        // Live getversion: {id, success, version} without type.
        let id = obj
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let version = obj
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let success = success.unwrap_or(true);
        return Ok(Response::Version {
            id,
            success,
            version,
        });
    }

    // Live ping (and other acks) is `{id, success:true}` with no discriminant.
    // Dispatcher remaps this to SqlClosed/ConfigSet/Exited when the outstanding
    // request is not ping.
    if success == Some(true) {
        let id = obj
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        return Ok(Response::Pong { id });
    }

    Err("unrecognized untagged response object".into())
}

fn decode_tagged(value: Value) -> Result<Response, String> {
    let tag = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing type".to_string())?;
    match tag {
        "connected" => {
            let body: ConnectedBody = payload(value)?;
            Ok(Response::Connected {
                id: body.id,
                version: body.version,
                job: body.job,
            })
        }
        "pong" => {
            let body: IdBody = payload(value)?;
            Ok(Response::Pong { id: body.id })
        }
        "exited" => {
            let body: IdBody = payload(value)?;
            Ok(Response::Exited { id: body.id })
        }
        "query_result" => {
            let q: QueryResult = payload(value)?;
            Ok(Response::QueryResult(q))
        }
        "prepared_statement" => {
            let body: PreparedStatementBody = payload(value)?;
            Ok(Response::PreparedStatement {
                id: body.id,
                success: body.success,
                cont_id: body.cont_id,
                execution_time: body.execution_time,
            })
        }
        "sql_closed" => {
            let body: SuccessBody = payload(value)?;
            Ok(Response::SqlClosed {
                id: body.id,
                success: body.success,
            })
        }
        "cl_result" => {
            let body: ClResultBody = payload(value)?;
            Ok(Response::ClResult {
                id: body.id,
                success: body.success,
                messages: body.messages,
            })
        }
        "version" => {
            let body: VersionBody = payload(value)?;
            Ok(Response::Version {
                id: body.id,
                success: body.success,
                version: body.version,
            })
        }
        "db_job" => {
            let body: DbJobBody = payload(value)?;
            Ok(Response::DbJob {
                id: body.id,
                success: body.success,
                job: body.job,
            })
        }
        "config_set" => {
            let body: SuccessBody = payload(value)?;
            Ok(Response::ConfigSet {
                id: body.id,
                success: body.success,
            })
        }
        "trace_data" => {
            let body: TraceDataBody = payload(value)?;
            Ok(Response::TraceData {
                id: body.id,
                success: body.success,
                tracedata: body.tracedata,
            })
        }
        "dove_result" => {
            let body: DoveResultBody = payload(value)?;
            Ok(Response::DoveResult {
                id: body.id,
                success: body.success,
                result: body.result,
            })
        }
        "error" => {
            let e: ErrorResponse = payload(value)?;
            Ok(Response::Error(e))
        }
        other => Err(format!("unknown response type: {other}")),
    }
}

fn payload<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(strip_type(value)).map_err(|e| e.to_string())
}

fn strip_type(mut value: Value) -> Value {
    if let Some(obj) = value.as_object_mut() {
        obj.remove("type");
    }
    value
}

#[derive(Deserialize)]
struct IdBody {
    id: String,
}

#[derive(Deserialize)]
struct SuccessBody {
    id: String,
    success: bool,
}

#[derive(Deserialize)]
struct ConnectedBody {
    id: String,
    #[serde(default)]
    version: String,
    job: String,
}

#[derive(Deserialize)]
struct PreparedStatementBody {
    id: String,
    success: bool,
    cont_id: String,
    #[serde(default)]
    execution_time: f64,
}

#[derive(Deserialize)]
struct ClResultBody {
    id: String,
    success: bool,
    messages: Vec<ClMessage>,
}

#[derive(Deserialize)]
struct VersionBody {
    id: String,
    success: bool,
    version: String,
}

#[derive(Deserialize)]
struct DbJobBody {
    id: String,
    success: bool,
    job: String,
}

#[derive(Deserialize)]
struct TraceDataBody {
    id: String,
    success: bool,
    tracedata: String,
}

#[derive(Deserialize)]
struct DoveResultBody {
    id: String,
    success: bool,
    result: Value,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::super::response::Response;

    #[test]
    fn test_decode_live_connect_without_type() {
        let json =
            r#"{"id":"abc","job":"123456/QUSER/QZDASOINIT","success":true,"execution_time":12.5}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::Connected { .. }));
        if let Response::Connected { id, job, version } = r {
            assert_eq!(id, "abc");
            assert_eq!(job, "123456/QUSER/QZDASOINIT");
            assert_eq!(version, "");
        }
    }

    #[test]
    fn test_decode_live_query_result_without_type() {
        let json = r#"{
            "id":"query3","has_results":true,"update_count":-1,
            "metadata":{"column_count":1,"job":"123456/QUSER/QZDASOINIT",
                "columns":[{"name":"READY","type":"INTEGER","display_size":11,
                            "label":"READY","precision":10,"scale":0}]},
            "data":[{"READY":1}],"is_done":true,"success":true,"execution_time":215
        }"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::QueryResult(_)));
        if let Response::QueryResult(q) = r {
            assert!(q.success && q.has_results && q.is_done);
            assert_eq!(q.data[0]["READY"], 1);
            assert_eq!(q.metadata.job.as_deref(), Some("123456/QUSER/QZDASOINIT"));
        }
    }

    #[test]
    fn test_decode_live_error_sql_rc_alias() {
        let json = r#"{"id":"x","success":false,"error":"nope","sql_rc":-803,"sql_state":"23505"}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::Error(_)));
        if let Response::Error(e) = r {
            assert_eq!(e.sqlcode, Some(-803));
            assert_eq!(e.sqlstate.as_deref(), Some("23505"));
        }
    }

    #[test]
    fn test_decode_tagged_connected_still_works() {
        let json =
            r#"{"type":"connected","id":"2","version":"2.3.5","job":"QZDASOINIT/QUSER/123456"}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::Connected { version, .. } if version == "2.3.5"));
    }

    #[test]
    fn test_decode_live_version_without_type() {
        let json = r#"{"id":"v1","success":true,"version":"2.3.5"}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::Version { .. }));
        if let Response::Version {
            id,
            success,
            version,
        } = r
        {
            assert_eq!(id, "v1");
            assert!(success);
            assert_eq!(version, "2.3.5");
        }
    }

    #[test]
    fn test_decode_unknown_type_is_error() {
        let json = r#"{"type":"not_a_real_type","id":"1","job":"x","success":true}"#;
        let err = serde_json::from_str::<Response>(json).unwrap_err();
        assert!(
            err.to_string().contains("unknown response type"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_decode_garbage_does_not_panic() {
        for json in ["", "null", "[]", "\"x\"", "1", "{", "{}", r#"{"type":1}"#] {
            let parsed = serde_json::from_str::<Response>(json);
            assert!(
                parsed.is_err(),
                "expected error for {json:?}, got {parsed:?}"
            );
        }
    }

    #[test]
    fn test_decode_tagged_fallback_covers_all_current_variants() {
        let samples = [
            r#"{"type":"connected","id":"1","version":"v","job":"j"}"#,
            r#"{"type":"pong","id":"1"}"#,
            r#"{"type":"exited","id":"1"}"#,
            r#"{"type":"query_result","id":"1","success":true,"has_results":false}"#,
            r#"{"type":"prepared_statement","id":"1","success":true,"cont_id":"c","execution_time":1.0}"#,
            r#"{"type":"sql_closed","id":"1","success":true}"#,
            r#"{"type":"cl_result","id":"1","success":true,"messages":[]}"#,
            r#"{"type":"version","id":"1","success":true,"version":"2.3.5"}"#,
            r#"{"type":"db_job","id":"1","success":true,"job":"j"}"#,
            r#"{"type":"config_set","id":"1","success":true}"#,
            r#"{"type":"trace_data","id":"1","success":true,"tracedata":"t"}"#,
            r#"{"type":"dove_result","id":"1","success":true,"result":{}}"#,
            r#"{"type":"error","id":"1","success":false,"error":"nope"}"#,
        ];
        for json in samples {
            let parsed = serde_json::from_str::<Response>(json);
            assert!(parsed.is_ok(), "tagged {json} failed: {parsed:?}");
        }
    }

    #[test]
    fn test_decode_tagged_payload_error() {
        let json = r#"{"type":"connected","id":"1"}"#;
        assert!(
            serde_json::from_str::<Response>(json).is_err(),
            "connected without job must fail"
        );
    }

    #[test]
    fn test_decode_untagged_error_missing_id() {
        let json = r#"{"success":false,"error":"x"}"#;
        assert!(
            serde_json::from_str::<Response>(json).is_err(),
            "error frame without id must fail"
        );
    }

    #[test]
    fn test_decode_untagged_result_malformed_data() {
        let json = r#"{"has_results":true,"data":"not-an-array"}"#;
        assert!(
            serde_json::from_str::<Response>(json).is_err(),
            "result with non-array data must fail"
        );
    }

    #[test]
    fn test_decode_live_pong_id_success_only() {
        let json = r#"{"id":"p1","success":true}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::Pong { id } if id == "p1"));
    }

    #[test]
    fn test_decode_live_prepare_without_has_results() {
        let json =
            r#"{"id":"pr1","success":true,"cont_id":"stmt-7","execution_time":1.5,"is_done":true}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(
            r,
            Response::PreparedStatement {
                id,
                cont_id,
                success,
                ..
            } if id == "pr1" && cont_id == "stmt-7" && success
        ));
    }

    #[test]
    fn test_decode_query_result_without_has_results_when_data_present() {
        let json = r#"{"id":"q","success":true,"data":[{"X":1}],"is_done":true}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::QueryResult(_)));
        if let Response::QueryResult(q) = r {
            assert!(!q.has_results);
            assert_eq!(q.data[0]["X"], 1);
        }
    }

    #[test]
    fn test_decode_live_connect_coerces_non_string_id_job() {
        let json = r#"{"job":null,"id":null,"success":true}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::Connected { .. }));
        if let Response::Connected { id, job, version } = r {
            assert_eq!(id, "");
            assert_eq!(job, "");
            assert_eq!(version, "");
        }
    }
}
