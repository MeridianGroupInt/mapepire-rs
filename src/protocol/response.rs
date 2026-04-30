//! Response messages — incoming wire types.

use serde::{Deserialize, Serialize};

/// Discriminated union of all response types the server may send.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Successful authentication.
    Connected {
        /// Echoes request id.
        id: String,
        /// Reported daemon version string.
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// Echoes request id.
    pub id: String,
    /// `true` on success.
    pub success: bool,
    /// `true` when the statement produced a result set (SELECT).
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
    #[serde(default)]
    pub data: Vec<serde_json::Map<String, serde_json::Value>>,
    /// Wall-clock execution time on the server, in milliseconds.
    #[serde(default)]
    pub execution_time: f64,
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

/// One CPF / Db2 message returned by a `cl` request.
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

/// Error response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Echoes request id.
    pub id: String,
    /// Always `false`.
    pub success: bool,
    /// Five-character SQLSTATE.
    #[serde(default)]
    pub sqlstate: Option<String>,
    /// Db2-native code.
    #[serde(default)]
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
            },
            data: vec![{
                let mut m = serde_json::Map::new();
                m.insert("ID".into(), serde_json::json!(42));
                m
            }],
            execution_time: 1.23,
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
}
