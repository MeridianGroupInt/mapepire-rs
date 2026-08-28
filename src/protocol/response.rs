//! Response messages — incoming wire types.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Discriminated union of all response types the server may send.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Successful authentication.
    Connected {
        /// Echoes request id.
        id: String,
        /// Reported daemon version string.
        ///
        /// Empty when the daemon omits `version` (live connect frames).
        #[serde(default)]
        version: String,
        /// Initial Db2 job name on the server.
        job: String,
    },

    /// Health-check echo.
    Pong {
        /// Echoes request id.
        id: String,
    },

    /// Acknowledges `exit`; socket closes immediately after.
    Exited {
        /// Echoes request id.
        id: String,
    },

    /// Result of `sql`, `execute`, `prepare_sql_execute`, or `sqlmore`.
    QueryResult(QueryResult),

    /// Acknowledges `prepare_sql`; provides the continuation handle for
    /// later `execute` or `sqlclose` calls.
    PreparedStatement {
        /// Echoes request id.
        id: String,
        /// `true` on success.
        success: bool,
        /// Server-side prepared-statement handle.
        cont_id: String,
        /// Wall-clock execution time on the server, in milliseconds.
        execution_time: f64,
    },

    /// Acknowledges `sqlclose`.
    SqlClosed {
        /// Echoes request id.
        id: String,
        /// `true` on success.
        success: bool,
    },

    /// Result of `cl`.
    ClResult {
        /// Echoes request id.
        id: String,
        /// `true` on success.
        success: bool,
        /// CPF / Db2 messages emitted by the command.
        messages: Vec<ClMessage>,
    },

    /// Result of `getversion`.
    Version {
        /// Echoes request id.
        id: String,
        /// `true` on success.
        success: bool,
        /// Daemon version string.
        version: String,
    },

    /// Result of `getdbjob`.
    DbJob {
        /// Echoes request id.
        id: String,
        /// `true` on success.
        success: bool,
        /// Db2 job name.
        job: String,
    },

    /// Result of `setconfig`.
    ConfigSet {
        /// Echoes request id.
        id: String,
        /// `true` on success.
        success: bool,
    },

    /// Result of `gettracedata`.
    TraceData {
        /// Echoes request id.
        id: String,
        /// `true` on success.
        success: bool,
        /// Accumulated trace text.
        tracedata: String,
    },

    /// Result of `dove` (Visual Explain). Inner shape is server-defined JSON.
    DoveResult {
        /// Echoes request id.
        id: String,
        /// `true` on success.
        success: bool,
        /// Plan tree as JSON.
        result: serde_json::Value,
    },

    /// Server-side error response.
    Error(ErrorResponse),
}

// NOTE(task-14 / v0.2): Several Response variant names are CamelCase
// compounds (PreparedStatement, SqlClosed, ClResult, DbJob, ConfigSet,
// TraceData, DoveResult). The enum-level `rename_all = "snake_case"` will
// emit them as prepared_statement / sql_closed / cl_result / db_job /
// config_set / trace_data / dove_result. The Mapepire daemon's actual
// response tags may use bare-form (sqlclosed, dbjob, configset,
// tracedata) consistent with the request side (sqlmore, sqlclose,
// getdbjob, setconfig, gettracedata). Task 14's insta snapshots against
// a live daemon — or v0.2 integration testing — will surface the
// divergences; per-variant `#[serde(rename = "...")]` overrides land
// then. Keeping snake_case defaults for now since the plan author
// didn't pre-pin these tags and we don't want to guess wrong.

/// Body of a `QueryResult` response.
#[derive(Debug, Clone, Serialize)]
pub struct QueryResult {
    /// Echoes request id.
    pub id: String,
    /// `true` on success.
    pub success: bool,
    /// `true` when the statement produced a result set (SELECT).
    ///
    /// Live daemons omit this on some success frames; default `false`.
    #[serde(default)]
    pub has_results: bool,
    /// Rows affected for INSERT/UPDATE/DELETE; `-1` (or absent) for SELECT.
    #[serde(default)]
    pub update_count: i64,
    /// Server-assigned cursor handle for paging via `sqlmore`.
    #[serde(default)]
    pub cont_id: Option<String>,
    /// `true` when no further pages remain.
    #[serde(default = "default_true")]
    pub is_done: bool,
    /// Column metadata.
    #[serde(default)]
    pub metadata: QueryMetaData,
    /// Row data — each row is a map of column name to JSON value.
    ///
    /// The wire may send object rows (`[{"EMPNO":"000010"}]`) or terse
    /// array rows (`[["000010"]]`). Decode normalizes arrays using
    /// `metadata.columns[i].name` so [`crate::Row::get`] still works.
    /// Array rows with no column names are a protocol error.
    ///
    /// For `type: cl` this is the job log (`MESSAGE_ID`, `SEVERITY`, …).
    #[serde(default)]
    pub data: Vec<serde_json::Map<String, serde_json::Value>>,
    /// Wall-clock execution time on the server, in milliseconds.
    #[serde(default)]
    pub execution_time: f64,
    /// Human-readable error when `success` is false (CL / SQL failures).
    ///
    /// Live failed-CL frames keep this alongside `data` rather than as a
    /// bare [`ErrorResponse`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Db2-native SQL code. Live frames send `sql_rc`.
    #[serde(default, alias = "sql_rc", skip_serializing_if = "Option::is_none")]
    pub sqlcode: Option<i32>,
    /// Five-character SQLSTATE. Live frames send `sql_state`.
    #[serde(default, alias = "sql_state", skip_serializing_if = "Option::is_none")]
    pub sqlstate: Option<String>,
    /// Number of parameter markers, when the daemon reports it.
    ///
    /// Present on `prepare_sql` / `prepare_sql_execute` / CALL responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter_count: Option<u32>,
    /// OUT / INOUT values from a stored-procedure `CALL`.
    ///
    /// Empty when the statement is not a procedure or the daemon omitted
    /// the field. `IN` entries typically have [`ParameterResult::value`]
    /// `None`. There is no separate CALL opcode — use
    /// [`crate::Job::execute_with`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_parms: Vec<ParameterResult>,
}

/// Wire DTO: `data` is objects or arrays. Normalized in [`QueryResult`]'s
/// [`Deserialize`] impl.
#[derive(Deserialize)]
struct QueryResultDe {
    id: String,
    success: bool,
    #[serde(default)]
    has_results: bool,
    #[serde(default)]
    update_count: i64,
    #[serde(default)]
    cont_id: Option<String>,
    #[serde(default = "default_true")]
    is_done: bool,
    #[serde(default)]
    metadata: QueryMetaData,
    #[serde(default)]
    data: Vec<Value>,
    #[serde(default)]
    execution_time: f64,
    #[serde(default)]
    error: Option<String>,
    #[serde(default, alias = "sql_rc")]
    sqlcode: Option<i32>,
    #[serde(default, alias = "sql_state")]
    sqlstate: Option<String>,
    #[serde(default)]
    parameter_count: Option<u32>,
    #[serde(default)]
    output_parms: Vec<ParameterResult>,
}

impl<'de> Deserialize<'de> for QueryResult {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = QueryResultDe::deserialize(deserializer)?;
        let data = named_rows_from_wire(&raw.metadata, raw.data).map_err(D::Error::custom)?;
        Ok(Self {
            id: raw.id,
            success: raw.success,
            has_results: raw.has_results,
            update_count: raw.update_count,
            cont_id: raw.cont_id,
            is_done: raw.is_done,
            metadata: raw.metadata,
            data,
            execution_time: raw.execution_time,
            error: raw.error,
            sqlcode: raw.sqlcode,
            sqlstate: raw.sqlstate,
            parameter_count: raw.parameter_count,
            output_parms: raw.output_parms,
        })
    }
}

/// Turn object rows or terse array rows into named maps.
///
/// Array rows use `metadata.columns[i].name`. Missing columns is a
/// protocol error (never panic on a short `columns` vec).
fn named_rows_from_wire(
    metadata: &QueryMetaData,
    data: Vec<Value>,
) -> Result<Vec<serde_json::Map<String, Value>>, String> {
    let mut out = Vec::with_capacity(data.len());
    for (idx, row) in data.into_iter().enumerate() {
        match row {
            Value::Object(map) => out.push(map),
            Value::Array(cells) => {
                if metadata.columns.is_empty() {
                    return Err(crate::error::ProtocolError::TerseRowsWithoutColumns.to_string());
                }
                if cells.len() > metadata.columns.len() {
                    return Err(format!(
                        "terse row {idx} has {} values but metadata.columns has {}",
                        cells.len(),
                        metadata.columns.len()
                    ));
                }
                let mut map = serde_json::Map::with_capacity(cells.len());
                for (i, cell) in cells.into_iter().enumerate() {
                    let name =
                        metadata
                            .columns
                            .get(i)
                            .map(|c| c.name.clone())
                            .ok_or_else(|| {
                                format!("terse row {idx} is missing metadata.columns[{i}]")
                            })?;
                    map.insert(name, cell);
                }
                out.push(map);
            }
            other => {
                return Err(format!(
                    "query row {idx} must be a JSON object or array, got {other}"
                ));
            }
        }
    }
    Ok(out)
}

fn default_true() -> bool {
    true
}

/// Result-set column metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryMetaData {
    /// Number of columns in each row.
    #[serde(default)]
    pub column_count: u32,
    /// Per-column metadata.
    #[serde(default)]
    pub columns: Vec<Column>,
    /// IBM i job that produced the result set, when the daemon reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    /// Parameter-marker metadata (`IN` / `OUT` / `INOUT`).
    ///
    /// Empty on ordinary SELECT/DML. Present on `prepare_sql` and CALL.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ParameterDetail>,
}

/// Metadata for one result-set column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    /// Server-reported column name.
    pub name: String,
    /// Optional column label (alias).
    #[serde(default)]
    pub label: Option<String>,
    /// Db2 type name.
    #[serde(rename = "type", default)]
    pub type_name: Option<String>,
    /// Display size, when reported.
    #[serde(default)]
    pub display_size: Option<u32>,
    /// Precision, when reported.
    #[serde(default)]
    pub precision: Option<u32>,
    /// Scale, when reported.
    #[serde(default)]
    pub scale: Option<u32>,
}

/// Describes a parameter marker in a prepared statement or `CALL`.
///
/// Wire names follow PROTOCOL.md `ParameterDetail`. `mode` is `IN`, `OUT`,
/// `INOUT`, or `UNKNOWN`.
///
/// # Example
///
/// ```
/// use mapepire::ParameterDetail;
///
/// let json = r#"{"type":"INTEGER","mode":"IN","precision":10,"scale":0,"name":"P1"}"#;
/// let p: ParameterDetail = serde_json::from_str(json).expect("parameter");
/// assert_eq!(p.name, "P1");
/// assert_eq!(p.mode, "IN");
/// assert_eq!(p.type_name, "INTEGER");
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterDetail {
    /// Db2 type name. JSON field `type`.
    #[serde(rename = "type", default)]
    pub type_name: String,
    /// Direction: `IN`, `OUT`, `INOUT`, or `UNKNOWN`.
    #[serde(default)]
    pub mode: String,
    /// Numeric precision or character length.
    #[serde(default)]
    pub precision: u32,
    /// Digits after the decimal point, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
    /// Parameter name (may be empty).
    #[serde(default)]
    pub name: String,
}

/// Output parameter value from a stored-procedure `CALL`.
///
/// `index` is 1-based. `value` is `None` for `IN` parameters (the daemon
/// omits it or sends `null`) and `Some` for `OUT` / `INOUT`.
///
/// # Example
///
/// ```
/// use mapepire::ParameterResult;
/// use serde_json::json;
///
/// let json = r#"{"index":3,"type":"INTEGER","precision":10,"scale":0,"name":"P3","value":10}"#;
/// let p: ParameterResult = serde_json::from_str(json).expect("parm result");
/// assert_eq!(p.index, 3);
/// assert_eq!(p.name, "P3");
/// assert_eq!(p.value, Some(json!(10)));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParameterResult {
    /// 1-based parameter index.
    #[serde(default)]
    pub index: u32,
    /// Db2 type name. JSON field `type`.
    #[serde(rename = "type", default)]
    pub type_name: String,
    /// Numeric precision or character length.
    #[serde(default)]
    pub precision: u32,
    /// Digits after the decimal point, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
    /// Parameter name.
    #[serde(default)]
    pub name: String,
    /// Character set ID, when reported for character types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ccsid: Option<u32>,
    /// Output value. `None` for `IN` (omitted or JSON `null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
}

/// One CPF / Db2 message returned by a tagged `cl_result` frame.
///
/// Live daemons emit job-log rows as [`JobLogEntry`] objects in
/// [`QueryResult::data`], not this shape. [`ClMessage`] is kept so tagged
/// mock frames still deserialize.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClMessage {
    /// e.g., `CPF1234`.
    #[serde(default)]
    pub id: Option<String>,
    /// Severity / type.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    /// Message text.
    #[serde(default)]
    pub text: Option<String>,
}

/// One job-log row from a live `cl` reply ([`QueryResult::data`]).
///
/// Wire names are the IBM i column names. `SEVERITY` may be a JSON number
/// or a string (JS types it as string; the protocol describes an integer).
///
/// # Example
///
/// ```
/// use mapepire::JobLogEntry;
///
/// let json = r#"{"MESSAGE_ID":"CPF0006","SEVERITY":40}"#;
/// let e: JobLogEntry = serde_json::from_str(json).expect("job-log row");
/// assert_eq!(e.message_id.as_deref(), Some("CPF0006"));
/// assert_eq!(e.severity.as_deref(), Some("40"));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLogEntry {
    /// CPF / SQL message identifier (e.g. `CPF0006`).
    #[serde(
        default,
        rename = "MESSAGE_ID",
        skip_serializing_if = "Option::is_none"
    )]
    pub message_id: Option<String>,
    /// Severity. Protocol may send an integer or a string.
    #[serde(
        default,
        rename = "SEVERITY",
        deserialize_with = "deserialize_optional_string_or_number",
        skip_serializing_if = "Option::is_none"
    )]
    pub severity: Option<String>,
    /// Timestamp when the message was generated.
    #[serde(
        default,
        rename = "MESSAGE_TIMESTAMP",
        skip_serializing_if = "Option::is_none"
    )]
    pub message_timestamp: Option<String>,
    /// Library from which the message originated.
    #[serde(
        default,
        rename = "FROM_LIBRARY",
        skip_serializing_if = "Option::is_none"
    )]
    pub from_library: Option<String>,
    /// Program from which the message originated.
    #[serde(
        default,
        rename = "FROM_PROGRAM",
        skip_serializing_if = "Option::is_none"
    )]
    pub from_program: Option<String>,
    /// Message type (e.g. `ESCAPE`, `COMPLETION`).
    #[serde(
        default,
        rename = "MESSAGE_TYPE",
        skip_serializing_if = "Option::is_none"
    )]
    pub message_type: Option<String>,
    /// First-level message text.
    #[serde(
        default,
        rename = "MESSAGE_TEXT",
        skip_serializing_if = "Option::is_none"
    )]
    pub message_text: Option<String>,
    /// Second-level message text, when present.
    #[serde(
        default,
        rename = "MESSAGE_SECOND_LEVEL_TEXT",
        skip_serializing_if = "Option::is_none"
    )]
    pub message_second_level_text: Option<String>,
}

/// Accept `SEVERITY` as a JSON string, number, or null.
fn deserialize_optional_string_or_number<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                Ok(Some(i.to_string()))
            } else if let Some(u) = n.as_u64() {
                Ok(Some(u.to_string()))
            } else {
                Ok(Some(n.to_string()))
            }
        }
        Some(other) => Err(D::Error::custom(format!(
            "SEVERITY must be a string or number, got {other}"
        ))),
    }
}

/// Error response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Echoes request id.
    pub id: String,
    /// Always `false`.
    pub success: bool,
    /// Five-character SQLSTATE.
    #[serde(default, alias = "sql_state")]
    pub sqlstate: Option<String>,
    /// Db2-native code.
    #[serde(default, alias = "sql_rc")]
    pub sqlcode: Option<i32>,
    /// Human-readable text.
    #[serde(default)]
    pub error: Option<String>,
    /// IBM i job that produced the error.
    #[serde(default)]
    pub job: Option<String>,
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn pong_round_trips() {
        let r = Response::Pong { id: "1".into() };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"pong","id":"1"}"#);
    }

    #[test]
    fn connected_round_trips() {
        let r = Response::Connected {
            id: "2".into(),
            version: "2.3.5".into(),
            job: "QZDASOINIT/QUSER/123456".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"type":"connected","id":"2","version":"2.3.5","job":"QZDASOINIT/QUSER/123456"}"#
        );
        let _: Response = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn exited_round_trips() {
        let r = Response::Exited { id: "3".into() };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"exited","id":"3"}"#);
    }

    #[test]
    fn query_result_select_round_trips() {
        let q = QueryResult {
            id: "10".into(),
            success: true,
            has_results: true,
            update_count: -1,
            cont_id: Some("cur-1".into()),
            is_done: false,
            metadata: QueryMetaData {
                column_count: 1,
                columns: vec![Column {
                    name: "ID".into(),
                    label: None,
                    type_name: Some("INTEGER".into()),
                    display_size: Some(11),
                    precision: Some(10),
                    scale: Some(0),
                }],
                job: None,
                parameters: vec![],
            },
            data: vec![{
                let mut m = serde_json::Map::new();
                m.insert("ID".into(), serde_json::json!(42));
                m
            }],
            execution_time: 1.23,
            error: None,
            sqlcode: None,
            sqlstate: None,
            parameter_count: None,
            output_parms: vec![],
        };
        let r = Response::QueryResult(q);
        let json = serde_json::to_string(&r).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::QueryResult(q2) => {
                assert!(q2.has_results);
                assert!(!q2.is_done);
                assert_eq!(q2.data.len(), 1);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn query_result_dml_round_trips() {
        let q = QueryResult {
            id: "11".into(),
            success: true,
            has_results: false,
            update_count: 3,
            cont_id: None,
            is_done: true,
            metadata: QueryMetaData::default(),
            data: vec![],
            execution_time: 0.5,
            error: None,
            sqlcode: None,
            sqlstate: None,
            parameter_count: None,
            output_parms: vec![],
        };
        let r = Response::QueryResult(q);
        let json = serde_json::to_string(&r).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::QueryResult(q2) => {
                assert!(!q2.has_results);
                assert_eq!(q2.update_count, 3);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn prepared_statement_round_trips() {
        let r = Response::PreparedStatement {
            id: "20".into(),
            success: true,
            cont_id: "stmt-7".into(),
            execution_time: 0.3,
        };
        let json = serde_json::to_string(&r).unwrap();
        let _: Response = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn cl_result_round_trips() {
        let r = Response::ClResult {
            id: "30".into(),
            success: true,
            messages: vec![ClMessage {
                id: Some("CPF1234".into()),
                kind: Some("INFO".into()),
                text: Some("Job started".into()),
            }],
        };
        let json = serde_json::to_string(&r).unwrap();
        let _: Response = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn error_response_round_trips() {
        let r = Response::Error(ErrorResponse {
            id: "40".into(),
            success: false,
            sqlstate: Some("23505".into()),
            sqlcode: Some(-803),
            error: Some("duplicate key".into()),
            job: Some("QZDASOINIT/QUSER/123456".into()),
        });
        let json = serde_json::to_string(&r).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::Error(e) => {
                assert_eq!(e.sqlstate.as_deref(), Some("23505"));
                assert_eq!(e.sqlcode, Some(-803));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_error_response_sql_state_and_sql_rc_aliases() {
        let json = r#"{"id":"x","success":false,"error":"nope","sql_rc":-803,"sql_state":"23505"}"#;
        let e: ErrorResponse = serde_json::from_str(json).unwrap();
        assert_eq!(e.sqlcode, Some(-803));
        assert_eq!(e.sqlstate.as_deref(), Some("23505"));
    }

    #[test]
    fn test_query_metadata_omits_absent_job_on_serialize() {
        let m = QueryMetaData::default();
        let json = serde_json::to_value(&m).unwrap();
        assert!(
            json.get("job").is_none(),
            "absent job must be omitted: {json}"
        );
        assert!(
            json.get("parameters").is_none(),
            "empty parameters must be omitted: {json}"
        );
        let back: QueryMetaData = serde_json::from_value(json).unwrap();
        assert!(back.job.is_none());
        assert!(back.parameters.is_empty());
    }

    #[test]
    fn test_query_result_sql_rc_and_sql_state_aliases() {
        let json = r#"{"id":"cl1","success":false,"data":[],"sql_rc":-443,"sql_state":"38501","error":"CPF0006"}"#;
        let q: QueryResult = serde_json::from_str(json).unwrap();
        assert!(!q.success);
        assert_eq!(q.sqlcode, Some(-443));
        assert_eq!(q.sqlstate.as_deref(), Some("38501"));
        assert_eq!(q.error.as_deref(), Some("CPF0006"));
    }

    #[test]
    fn test_job_log_entry_severity_number_or_string() {
        let as_number: JobLogEntry =
            serde_json::from_str(r#"{"MESSAGE_ID":"CPF0006","SEVERITY":40}"#).unwrap();
        assert_eq!(as_number.message_id.as_deref(), Some("CPF0006"));
        assert_eq!(as_number.severity.as_deref(), Some("40"));

        let as_string: JobLogEntry =
            serde_json::from_str(r#"{"MESSAGE_ID":"CPC2102","SEVERITY":"0"}"#).unwrap();
        assert_eq!(as_string.severity.as_deref(), Some("0"));
    }

    #[test]
    fn test_connected_version_defaults_when_absent() {
        let json = r#"{"type":"connected","id":"2","job":"QZDASOINIT/QUSER/123456"}"#;
        let r: Response = serde_json::from_str(json).unwrap();
        assert!(matches!(r, Response::Connected { .. }));
        if let Response::Connected { version, .. } = r {
            assert_eq!(version, "");
        }
    }

    #[test]
    fn test_query_result_decodes_terse_arrays_via_column_names() {
        let json = r#"{
            "id":"q1","success":true,"has_results":true,
            "metadata":{"column_count":1,"columns":[{"name":"1"}]},
            "data":[[7]]
        }"#;
        let q: QueryResult = serde_json::from_str(json).unwrap();
        assert_eq!(q.data.len(), 1);
        assert_eq!(q.data[0]["1"], 7);
    }

    #[test]
    fn test_query_result_terse_without_columns_errors() {
        let json = r#"{"id":"q1","success":true,"data":[[7]]}"#;
        let err = serde_json::from_str::<QueryResult>(json).unwrap_err();
        assert!(
            err.to_string()
                .contains("terse row data requires metadata.columns"),
            "unexpected error: {err}"
        );
    }

    /// JS `procedures.test.ts` integer IN/INOUT/OUT: CALL with `[6, 4, 0]`
    /// returns `output_parms` values `[undefined, 0, 10]`.
    const CALL_OUT_JSON: &str = r#"{
        "id":"call1","success":true,"has_results":false,"update_count":0,
        "is_done":true,"parameter_count":3,
        "metadata":{
            "column_count":0,"columns":[],
            "parameters":[
                {"type":"INTEGER","mode":"IN","precision":10,"scale":0,"name":"P1"},
                {"type":"INTEGER","mode":"INOUT","precision":10,"scale":0,"name":"P2"},
                {"type":"INTEGER","mode":"OUT","precision":10,"scale":0,"name":"P3"}
            ]
        },
        "data":[],
        "output_parms":[
            {"index":1,"type":"INTEGER","precision":10,"scale":0,"name":"P1"},
            {"index":2,"type":"INTEGER","precision":10,"scale":0,"name":"P2","value":0},
            {"index":3,"type":"INTEGER","precision":10,"scale":0,"name":"P3","value":10}
        ],
        "execution_time":5
    }"#;

    #[test]
    fn test_query_result_decodes_call_output_parms() {
        let q: QueryResult = serde_json::from_str(CALL_OUT_JSON).unwrap();
        assert!(!q.has_results);
        assert_eq!(q.update_count, 0);
        assert!(q.data.is_empty());
        assert_eq!(q.parameter_count, Some(3));
        assert_eq!(
            q.metadata
                .parameters
                .iter()
                .map(|p| (p.name.as_str(), p.mode.as_str(), p.type_name.as_str()))
                .collect::<Vec<_>>(),
            [
                ("P1", "IN", "INTEGER"),
                ("P2", "INOUT", "INTEGER"),
                ("P3", "OUT", "INTEGER")
            ]
        );
        assert_eq!(q.output_parms.len(), 3);
        assert_eq!(q.output_parms[0].value, None);
        assert_eq!(q.output_parms[1].value, Some(serde_json::json!(0)));
        assert_eq!(q.output_parms[2].value, Some(serde_json::json!(10)));
        assert_eq!(q.output_parms[0].index, 1);
        assert_eq!(q.output_parms[2].name, "P3");
    }

    #[test]
    fn test_response_decodes_untagged_call_as_query_result() {
        let r: Response = serde_json::from_str(CALL_OUT_JSON).unwrap();
        match r {
            Response::QueryResult(q) => {
                assert_eq!(q.parameter_count, Some(3));
                assert_eq!(q.output_parms[0].value, None);
                assert_eq!(q.output_parms[1].value, Some(serde_json::json!(0)));
                assert_eq!(q.output_parms[2].value, Some(serde_json::json!(10)));
            }
            other => panic!("expected QueryResult, got {other:?}"),
        }
    }

    #[test]
    fn test_query_result_omits_empty_output_parms_on_serialize() {
        let q = QueryResult {
            id: "q1".into(),
            success: true,
            has_results: false,
            update_count: 0,
            cont_id: None,
            is_done: true,
            metadata: QueryMetaData::default(),
            data: vec![],
            execution_time: 0.0,
            error: None,
            sqlcode: None,
            sqlstate: None,
            parameter_count: None,
            output_parms: vec![],
        };
        let json = serde_json::to_value(&q).unwrap();
        assert!(
            json.get("output_parms").is_none(),
            "empty output_parms must be omitted: {json}"
        );
        assert!(
            json.get("parameter_count").is_none(),
            "absent parameter_count must be omitted: {json}"
        );
    }
}
