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
    let has_data = obj.contains_key("data");
    let has_results_key = obj.contains_key("has_results");
    let has_dove = obj.contains_key("vedata") || obj.contains_key("vemetadata");
    if success == Some(false) {
        // Live dove 42505 is `{success:false, error, sql_state}` with no
        // vedata — keep that as Error. A failed frame that still carries
        // the explain tree is DoveResult.
        if has_dove {
            return decode_dove(value);
        }
        // Live `cl` (and any result-shaped failure) still carries `data`
        // or `has_results`. Classifying those as `Error` dropped the job
        // log. Bare `{success:false}` without those keys stays `Error`.
        if has_data || has_results_key {
            let q: QueryResult = serde_json::from_value(value).map_err(|e| e.to_string())?;
            return Ok(Response::QueryResult(q));
        }
        let err: ErrorResponse = serde_json::from_value(value).map_err(|e| e.to_string())?;
        return Ok(Response::Error(err));
    }
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

    // Live dove: `{id, success, vemetadata, vedata}` and, when run=true,
    // also `data` / `is_done`. Classify as DoveResult first so vedata is
    // not dropped into QueryResult.
    if has_dove {
        return decode_dove(value);
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

    // Live gettracedata: `{id, success, tracedata, jtopentracedata?}`.
    // Either key (including `""` / `null`) is enough — do not drop the
    // buffer by classifying as Pong. Ping `{id, success}` with neither
    // key stays Pong.
    if obj.contains_key("tracedata") || obj.contains_key("jtopentracedata") {
        let id = obj
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        return Ok(Response::TraceData {
            id,
            success: success.unwrap_or(true),
            tracedata: json_string_or_empty(obj.get("tracedata")),
        });
    }

    // Live ping (and other acks) is `{id, success:true}` with no discriminant.
    // Dispatcher remaps this to SqlClosed/ConfigSet/Exited/TraceData when the
    // outstanding request is not ping.
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
            Ok(dove_from_body(body))
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

/// `tracedata` / similar: missing, JSON null, or non-string → empty.
fn json_string_or_empty(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn decode_dove(value: Value) -> Result<Response, String> {
    let body: DoveResultBody = serde_json::from_value(value).map_err(|e| e.to_string())?;
    Ok(dove_from_body(body))
}

fn dove_from_body(body: DoveResultBody) -> Response {
    let vedata = body.vedata.or(body.result).unwrap_or(Value::Null);
    Response::DoveResult {
        id: body.id,
        success: body.success,
        vedata,
        vemetadata: body.vemetadata,
    }
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
    /// Live / tagged frames may omit the field, send `""`, or send JSON
    /// `null`. Extra `jtopentracedata` is ignored (unknown-field default).
    #[serde(default, deserialize_with = "deserialize_null_as_empty")]
    tracedata: String,
}

fn deserialize_null_as_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Deserialize)]
struct DoveResultBody {
    id: String,
    success: bool,
    #[serde(default)]
    vedata: Option<Value>,
    #[serde(default)]
    vemetadata: Option<Value>,
    /// Tagged mock dialect (`type: dove_result`).
    #[serde(default)]
    result: Option<Value>,
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
    fn test_decode_live_query_result_omitted_is_done_and_cont_id() {
        let json = r#"{
            "id":"q1","has_results":true,"update_count":-1,
            "metadata":{"column_count":1,"columns":[{"name":"n"}]},
            "data":[{"n":1},{"n":2}],"success":true,"execution_time":1
        }"#;
        let r: Response = serde_json::from_str(json).unwrap();
        let Response::QueryResult(q) = r else {
            panic!("expected QueryResult, got {r:?}");
        };
        assert!(!q.is_done);
        assert!(q.cont_id.is_none());
        assert_eq!(q.cursor_handle(), Some("q1"));
        assert_eq!(q.data.len(), 2);
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
    fn test_decode_live_failed_cl_with_data_is_query_result() {
        let json = r#"{
            "id":"cl1","success":false,"sql_rc":-443,"sql_state":"38501",
            "error":"[CPF0006] Errors occurred in command.",
            "data":[{"MESSAGE_ID":"CPF0006","SEVERITY":40,"MESSAGE_TEXT":"Errors occurred in command."}],
            "is_done":true
        }"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::QueryResult(_)));
        if let Response::QueryResult(q) = r {
            assert!(!q.success);
            assert_eq!(q.sqlcode, Some(-443));
            assert_eq!(q.sqlstate.as_deref(), Some("38501"));
            assert_eq!(
                q.error.as_deref(),
                Some("[CPF0006] Errors occurred in command.")
            );
            assert_eq!(q.data.len(), 1);
            assert_eq!(q.data[0]["MESSAGE_ID"], "CPF0006");
        }
    }

    #[test]
    fn test_decode_live_failed_with_has_results_is_query_result() {
        let json = r#"{"id":"cl2","success":false,"has_results":true,"data":[]}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::QueryResult(_)));
        if let Response::QueryResult(q) = r {
            assert!(!q.success);
            assert!(q.has_results);
            assert!(q.data.is_empty());
        }
    }

    #[test]
    fn test_decode_live_failed_without_data_stays_error() {
        let json = r#"{"id":"x","success":false,"error":"nope","sql_rc":-803,"sql_state":"23505"}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::Error(_)));
    }

    #[test]
    fn test_decode_tagged_connected_still_works() {
        let json =
            r#"{"type":"connected","id":"2","version":"2.3.5","job":"QZDASOINIT/QUSER/123456"}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::Connected { version, .. } if version == "2.3.5"));
    }

    #[test]
    fn test_decode_live_setconfig_with_dest_is_pong() {
        // PROTOCOL.md §13 success: `{id, success, tracedest, tracelevel, …}`.
        // Extra dest/level keys still land on Pong; dispatcher remaps
        // outstanding SetConfig → ConfigSet (OSS-1).
        let json = r#"{"id":"t1","success":true,"tracedest":"IN_MEM","tracelevel":"OFF","execution_time":1}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::Pong { id } if id == "t1"));
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
            r#"{"type":"dove_result","id":"1","success":true,"vedata":[]}"#,
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
    fn test_decode_live_dove_vedata_without_data() {
        let json = r#"{"id":"d1","success":true,"vemetadata":{"v":1},"vedata":[{"op":"TBSCAN"}]}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        match r {
            Response::DoveResult {
                id,
                success,
                vedata,
                vemetadata,
            } => {
                assert_eq!(id, "d1");
                assert!(success);
                assert_eq!(vedata, serde_json::json!([{"op":"TBSCAN"}]));
                assert_eq!(vemetadata, Some(serde_json::json!({"v":1})));
            }
            other => panic!("expected DoveResult, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_live_dove_with_data_is_not_query_result() {
        let json = r#"{
            "id":"d1","success":true,"is_done":true,
            "vemetadata":{"v":1},"vedata":[{"op":"TBSCAN"}],
            "data":[{"X":1}]
        }"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(
            matches!(r, Response::DoveResult { .. }),
            "run=true dove with data must not become QueryResult: {r:?}"
        );
        if let Response::DoveResult { vedata, .. } = r {
            assert_eq!(vedata, serde_json::json!([{"op":"TBSCAN"}]));
        }
    }

    #[test]
    fn test_decode_dove_42505_without_vedata_is_error() {
        let json = r#"{"id":"d1","success":false,"error":"not authorized","sql_state":"42505"}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        match r {
            Response::Error(e) => {
                assert_eq!(e.sqlstate.as_deref(), Some("42505"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_tagged_dove_result_still_accepts_result() {
        let json = r#"{"type":"dove_result","id":"1","success":true,"result":{"operator":"scan"}}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        match r {
            Response::DoveResult { vedata, .. } => {
                assert_eq!(vedata, serde_json::json!({"operator":"scan"}));
            }
            other => panic!("expected DoveResult, got {other:?}"),
        }
    }

    #[test]
    fn test_decode_live_gettracedata_keeps_tracedata() {
        let json = r#"{"id":"t1","success":true,"tracedata":"hello","execution_time":0}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(
            r,
            Response::TraceData {
                id,
                success: true,
                tracedata,
            } if id == "t1" && tracedata == "hello"
        ));
    }

    #[test]
    fn test_decode_live_gettracedata_empty_or_null_tracedata() {
        let empty = r#"{"id":"t1","success":true,"tracedata":""}"#;
        let r: Response = serde_json::from_str(empty).unwrap();
        assert!(matches!(
            r,
            Response::TraceData {
                tracedata,
                ..
            } if tracedata.is_empty()
        ));

        let null = r#"{"id":"t1","success":true,"tracedata":null}"#;
        let r: Response = serde_json::from_str(null).unwrap();
        assert!(matches!(
            r,
            Response::TraceData {
                tracedata,
                ..
            } if tracedata.is_empty()
        ));
    }

    #[test]
    fn test_decode_live_gettracedata_jtopentracedata_does_not_fail() {
        let json = r#"{"id":"t1","success":true,"tracedata":"buf","jtopentracedata":"jt"}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(
            r,
            Response::TraceData {
                tracedata,
                ..
            } if tracedata == "buf"
        ));

        let only_jt = r#"{"id":"t1","success":true,"jtopentracedata":""}"#;
        let r: Response = serde_json::from_str(only_jt).unwrap();
        assert!(matches!(
            r,
            Response::TraceData {
                id,
                tracedata,
                ..
            } if id == "t1" && tracedata.is_empty()
        ));

        let jt_null = r#"{"id":"t1","success":true,"jtopentracedata":null}"#;
        let r: Response = serde_json::from_str(jt_null).unwrap();
        assert!(matches!(r, Response::TraceData { .. }));
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

    #[test]
    fn test_decode_terse_array_rows_named_from_columns() {
        let json = r#"{
            "id":"q1","success":true,"has_results":true,"is_done":true,
            "metadata":{"column_count":1,"columns":[{"name":"1","type":"INTEGER"}]},
            "data":[[7]]
        }"#;
        let r: Response = serde_json::from_str(json).unwrap();
        let Response::QueryResult(q) = r else {
            panic!("expected QueryResult, got {r:?}");
        };
        assert_eq!(q.data.len(), 1);
        assert_eq!(q.data[0]["1"], 7);
        let row = crate::query::Row::from_map(q.data[0].clone());
        let n: i64 = row.get("1").expect("column 1");
        assert_eq!(n, 7);
    }

    #[test]
    fn test_decode_object_rows_still_named() {
        let json = r#"{
            "id":"q1","success":true,"has_results":true,"is_done":true,
            "metadata":{"column_count":1,"columns":[{"name":"READY"}]},
            "data":[{"READY":1}]
        }"#;
        let r: Response = serde_json::from_str(json).unwrap();
        let Response::QueryResult(q) = r else {
            panic!("expected QueryResult, got {r:?}");
        };
        assert_eq!(q.data[0]["READY"], 1);
    }

    #[test]
    fn test_decode_terse_without_columns_is_protocol_error() {
        let json = r#"{"id":"q1","success":true,"has_results":true,"data":[[7]]}"#;
        let err = serde_json::from_str::<Response>(json).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("terse row data requires metadata.columns"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn test_decode_live_version_missing_success_defaults_true() {
        let json = r#"{"id":"v1","version":"2.3.5"}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(
            r,
            Response::Version {
                id,
                success: true,
                version,
            } if id == "v1" && version == "2.3.5"
        ));
    }

    #[test]
    fn test_decode_live_pong_missing_id_is_empty() {
        let json = r#"{"success":true}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::Pong { id } if id.is_empty()));
    }

    #[test]
    fn test_decode_live_version_non_string_fields_coerce_empty() {
        let json = r#"{"id":1,"version":2}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(
            r,
            Response::Version {
                id,
                success: true,
                version,
            } if id.is_empty() && version.is_empty()
        ));
    }
}
