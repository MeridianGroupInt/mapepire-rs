//! Request messages — outgoing wire types. Variants added in subsequent tasks.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Discriminated union of all request types the client can send.
///
/// Tagged on the wire by the `type` field. Variants are added in
/// subsequent protocol tasks.
///
/// [`Debug`] is hand-written, not derived, and exhaustive with **no
/// wildcard arm**. Adding a variant (or a secret field on an existing
/// one) fails to compile until this impl is updated — the canary that a
/// new credential cannot silently print. `Connect` carries no secrets;
/// credentials travel as HTTP Basic on the WebSocket upgrade, not in
/// this JSON.
#[non_exhaustive]
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Establish a daemon session.
    ///
    /// Live daemon authentication is HTTP Basic on the WebSocket
    /// upgrade, not this JSON body. Sibling `SQLJob` sends
    /// `{id, type: "connect", technique: "tcp", application, props?}`.
    ///
    /// # Example
    ///
    /// ```
    /// use mapepire::protocol::request::Request;
    ///
    /// let r = Request::Connect {
    ///     id: "1".into(),
    ///     technique: "tcp".into(),
    ///     application: "mapepire-rs".into(),
    ///     props: None,
    /// };
    /// let json = serde_json::to_string(&r).expect("Request serializes");
    /// assert_eq!(
    ///     json,
    ///     r#"{"type":"connect","id":"1","technique":"tcp","application":"mapepire-rs"}"#
    /// );
    /// assert!(!json.contains("password"));
    /// assert!(!json.contains("user"));
    /// ```
    Connect {
        /// Caller-supplied correlation id.
        id: String,
        /// Connection technique. Handshake always sends `"tcp"`.
        technique: String,
        /// Client identifier reported to the daemon. Defaults to `"mapepire-rs"`.
        application: String,
        /// Optional JDBC properties string (semicolon-delimited).
        ///
        /// Omitted from the wire when `None`.
        #[serde(skip_serializing_if = "Option::is_none")]
        props: Option<String>,
    },

    /// Execute a SQL statement (DML, DDL, or query) without preparing it.
    Sql {
        /// Caller-supplied correlation id.
        id: String,
        /// SQL text.
        sql: String,
        /// Initial page size; `None` lets the server pick.
        #[serde(skip_serializing_if = "Option::is_none")]
        rows: Option<u32>,
        /// Optional bound parameters (one set).
        #[serde(skip_serializing_if = "Option::is_none")]
        parameters: Option<Vec<serde_json::Value>>,
        /// When `Some(true)`, the daemon returns each row as a JSON array
        /// in column order. Omitted (`None`) keeps object rows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terse: Option<bool>,
    },

    /// Prepare a SQL statement without executing.
    PrepareSql {
        /// Caller-supplied correlation id.
        id: String,
        /// SQL text.
        sql: String,
        /// When `Some(true)`, subsequent result rows are arrays in column
        /// order. Omitted (`None`) keeps object rows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terse: Option<bool>,
    },

    /// Prepare and execute in one round-trip; supports batched parameters.
    PrepareSqlExecute {
        /// Caller-supplied correlation id.
        id: String,
        /// SQL text.
        sql: String,
        /// One or more parameter sets.
        ///
        /// In memory this is always a vector of sets. On the wire a single
        /// set serializes as a flat JSON array (`[7]`, matching mapepire-js
        /// and PROTOCOL.md); two or more sets stay nested
        /// (`[[1,"a"],[2,"b"]]`).
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "parameter_sets"
        )]
        parameters: Option<Vec<Vec<serde_json::Value>>>,
        /// Initial page size for the resulting cursor (per execution).
        #[serde(skip_serializing_if = "Option::is_none")]
        rows: Option<u32>,
        /// When `Some(true)`, result rows are arrays in column order.
        /// Omitted (`None`) keeps object rows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terse: Option<bool>,
    },

    /// Execute a previously prepared statement.
    Execute {
        /// Caller-supplied correlation id.
        id: String,
        /// Server-side prepared-statement handle from a prior `prepare_sql`.
        cont_id: String,
        /// Parameter set for this execution.
        #[serde(skip_serializing_if = "Option::is_none")]
        parameters: Option<Vec<serde_json::Value>>,
        /// Page size for this execution; `None` lets the server pick.
        #[serde(skip_serializing_if = "Option::is_none")]
        rows: Option<u32>,
        /// When `Some(true)`, result rows are arrays in column order.
        /// Omitted (`None`) keeps object rows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terse: Option<bool>,
    },

    /// Fetch the next page of rows from an open cursor.
    #[serde(rename = "sqlmore")]
    SqlMore {
        /// Caller-supplied correlation id.
        id: String,
        /// Cursor / continuation handle from a prior `sql` or `execute`.
        cont_id: String,
        /// Number of additional rows to fetch.
        rows: u32,
    },

    /// Close a server-side cursor.
    #[serde(rename = "sqlclose")]
    SqlClose {
        /// Caller-supplied correlation id.
        id: String,
        /// Cursor / continuation handle.
        cont_id: String,
    },

    /// Run an IBM i CL command.
    Cl {
        /// Caller-supplied correlation id.
        id: String,
        /// CL command text — e.g., `WRKACTJOB`.
        cmd: String,
        /// When `Some(true)`, job-log rows are arrays in column order.
        /// Omitted (`None`) keeps object rows. [`crate::Job::cl`] always
        /// omits this so job-log mapping stays named columns.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terse: Option<bool>,
    },

    /// Retrieve the daemon version.
    #[serde(rename = "getversion")]
    GetVersion {
        /// Caller-supplied correlation id.
        id: String,
    },

    /// Retrieve the current Db2 job name.
    #[serde(rename = "getdbjob")]
    GetDbJob {
        /// Caller-supplied correlation id.
        id: String,
    },

    /// Configure server-side tracing.
    #[serde(rename = "setconfig")]
    SetConfig {
        /// Caller-supplied correlation id.
        id: String,
        /// Tracing level — `OFF`, `ON`, `ERRORS`, `DATASTREAM`, or
        /// `INPUT_AND_ERRORS`. Omitted or empty leaves the current level.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tracelevel: String,
        /// Trace destination — `FILE` or `IN_MEM`. Never `""` (Jetty
        /// `Tracer.Dest` has no empty constant). Omitted leaves the current dest.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        tracedest: String,
    },

    /// Retrieve accumulated trace data.
    #[serde(rename = "gettracedata")]
    GetTraceData {
        /// Caller-supplied correlation id.
        id: String,
    },

    /// Visual Explain — request an execution-plan tree for a SQL statement.
    Dove {
        /// Caller-supplied correlation id.
        id: String,
        /// SQL statement to explain.
        sql: String,
        /// When `Some(true)`, result rows (if `run` is set by the daemon)
        /// are arrays in column order. Omitted (`None`) keeps object rows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terse: Option<bool>,
    },

    /// Health check.
    Ping {
        /// Caller-supplied correlation id.
        id: String,
    },

    /// Terminate the session and close the connection.
    Exit {
        /// Caller-supplied correlation id.
        id: String,
    },
}

/// Hand-written so a new secret field cannot silently print.
///
/// The match is exhaustive with **no wildcard arm** on purpose. `Request` is
/// `#[non_exhaustive]`, which constrains downstream crates but not this one,
/// so adding a variant breaks this `impl` at compile time and forces a
/// deliberate decision about whether the new variant carries a secret. A
/// `_ => ...` arm would silently print one. `Connect` currently has no
/// secrets; print its fields faithfully.
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect {
                id,
                technique,
                application,
                props,
            } => f
                .debug_struct("Connect")
                .field("id", id)
                .field("technique", technique)
                .field("application", application)
                .field("props", props)
                .finish(),
            Self::Sql {
                id,
                sql,
                rows,
                parameters,
                terse,
            } => f
                .debug_struct("Sql")
                .field("id", id)
                .field("sql", sql)
                .field("rows", rows)
                .field("parameters", parameters)
                .field("terse", terse)
                .finish(),
            Self::PrepareSql { id, sql, terse } => f
                .debug_struct("PrepareSql")
                .field("id", id)
                .field("sql", sql)
                .field("terse", terse)
                .finish(),
            Self::PrepareSqlExecute {
                id,
                sql,
                parameters,
                rows,
                terse,
            } => f
                .debug_struct("PrepareSqlExecute")
                .field("id", id)
                .field("sql", sql)
                .field("parameters", parameters)
                .field("rows", rows)
                .field("terse", terse)
                .finish(),
            Self::Execute {
                id,
                cont_id,
                parameters,
                rows,
                terse,
            } => f
                .debug_struct("Execute")
                .field("id", id)
                .field("cont_id", cont_id)
                .field("parameters", parameters)
                .field("rows", rows)
                .field("terse", terse)
                .finish(),
            Self::SqlMore { id, cont_id, rows } => f
                .debug_struct("SqlMore")
                .field("id", id)
                .field("cont_id", cont_id)
                .field("rows", rows)
                .finish(),
            Self::SqlClose { id, cont_id } => f
                .debug_struct("SqlClose")
                .field("id", id)
                .field("cont_id", cont_id)
                .finish(),
            Self::Cl { id, cmd, terse } => f
                .debug_struct("Cl")
                .field("id", id)
                .field("cmd", cmd)
                .field("terse", terse)
                .finish(),
            Self::GetVersion { id } => f.debug_struct("GetVersion").field("id", id).finish(),
            Self::GetDbJob { id } => f.debug_struct("GetDbJob").field("id", id).finish(),
            Self::GetTraceData { id } => f.debug_struct("GetTraceData").field("id", id).finish(),
            Self::SetConfig {
                id,
                tracelevel,
                tracedest,
            } => f
                .debug_struct("SetConfig")
                .field("id", id)
                .field("tracelevel", tracelevel)
                .field("tracedest", tracedest)
                .finish(),
            Self::Dove { id, sql, terse } => f
                .debug_struct("Dove")
                .field("id", id)
                .field("sql", sql)
                .field("terse", terse)
                .finish(),
            Self::Ping { id } => f.debug_struct("Ping").field("id", id).finish(),
            Self::Exit { id } => f.debug_struct("Exit").field("id", id).finish(),
        }
    }
}

/// Wire shape for `prepare_sql_execute.parameters`.
///
/// PROTOCOL.md / mapepire-js: one set is a JSON array of values (`[7]`);
/// a batch is an array of those arrays (`[[1,"a"],[2,"b"]]`). The in-memory
/// type stays `Vec<Vec<Value>>` so `Job` / `Query` can treat every call as
/// a list of sets.
mod parameter_sets {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    // serde `serialize_with` receives `&FieldType`; the field is `Option<_>`.
    #[allow(clippy::ref_option)]
    pub fn serialize<S>(sets: &Option<Vec<Vec<Value>>>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match sets {
            None => serializer.serialize_none(),
            Some(sets) if sets.len() == 1 => sets[0].serialize(serializer),
            Some(sets) => sets.serialize(serializer),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Vec<Vec<Value>>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let Some(value) = Option::<Value>::deserialize(deserializer)? else {
            return Ok(None);
        };
        match value {
            Value::Null => Ok(None),
            Value::Array(items) => {
                if items.iter().all(Value::is_array) {
                    Ok(Some(
                        items
                            .into_iter()
                            .filter_map(|v| match v {
                                Value::Array(inner) => Some(inner),
                                _ => None,
                            })
                            .collect(),
                    ))
                } else {
                    Ok(Some(vec![items]))
                }
            }
            _ => Err(D::Error::custom("parameters must be a JSON array")),
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn ping_round_trips() {
        let r = Request::Ping { id: "1".into() };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"ping","id":"1"}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Ping { id } if id == "1"));
    }

    #[test]
    fn test_connect_serializes_live_shape_without_password() {
        let r = Request::Connect {
            id: "2".into(),
            technique: "tcp".into(),
            application: "mapepire-rs".into(),
            props: Some("access=read only;auto commit=true".into()),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"type":"connect","id":"2","technique":"tcp","application":"mapepire-rs","props":"access=read only;auto commit=true"}"#
        );
        assert!(!json.contains("password"));
        assert!(!json.contains("user"));
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back,
            Request::Connect {
                technique,
                application,
                props: Some(p),
                ..
            } if technique == "tcp"
                && application == "mapepire-rs"
                && p == "access=read only;auto commit=true"
        ));
        let debug = format!("{r:?}");
        assert!(
            debug.contains(r#"technique: "tcp""#),
            "technique missing from Debug: {debug}"
        );
        assert!(
            debug.contains(r#"application: "mapepire-rs""#),
            "application missing from Debug: {debug}"
        );
        assert!(
            !debug.contains("password"),
            "password leaked into Debug: {debug}"
        );
        assert!(!debug.contains("user"), "user leaked into Debug: {debug}");
    }

    #[test]
    fn test_connect_omits_empty_props() {
        let r = Request::Connect {
            id: "2".into(),
            technique: "tcp".into(),
            application: "mapepire-rs".into(),
            props: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("props"));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn non_secret_variants_debug_faithfully() {
        // Every variant, so each arm of the hand-written `Debug` is exercised
        // and none of them silently stops rendering a field. `Connect` has no
        // secrets; JSON regression in
        // `test_connect_serializes_live_shape_without_password` is the other
        // canary that credentials do not reappear on the body.
        //
        // A new variant cannot slip past this unnoticed: `Debug`'s match has no
        // wildcard arm, so adding one fails to compile until it is handled.
        let cases: Vec<(Request, &str)> = vec![
            (
                Request::Connect {
                    id: "0".into(),
                    technique: "tcp".into(),
                    application: "mapepire-rs".into(),
                    props: None,
                },
                r#"Connect { id: "0", technique: "tcp", application: "mapepire-rs", props: None }"#,
            ),
            (
                Request::Sql {
                    id: "1".into(),
                    sql: "SELECT 1".into(),
                    rows: Some(10),
                    parameters: None,
                    terse: None,
                },
                r#"Sql { id: "1", sql: "SELECT 1", rows: Some(10), parameters: None, terse: None }"#,
            ),
            (
                Request::PrepareSql {
                    id: "2".into(),
                    sql: "SELECT ?".into(),
                    terse: None,
                },
                r#"PrepareSql { id: "2", sql: "SELECT ?", terse: None }"#,
            ),
            (
                Request::PrepareSqlExecute {
                    id: "3".into(),
                    sql: "SELECT ?".into(),
                    parameters: None,
                    rows: None,
                    terse: None,
                },
                r#"PrepareSqlExecute { id: "3", sql: "SELECT ?", parameters: None, rows: None, terse: None }"#,
            ),
            (
                Request::Execute {
                    id: "4".into(),
                    cont_id: "cur-1".into(),
                    parameters: Some(vec![serde_json::Value::from("x")]),
                    rows: None,
                    terse: None,
                },
                r#"Execute { id: "4", cont_id: "cur-1", parameters: Some([String("x")]), rows: None, terse: None }"#,
            ),
            (
                Request::SqlMore {
                    id: "20".into(),
                    cont_id: "cur-1".into(),
                    rows: 100,
                },
                r#"SqlMore { id: "20", cont_id: "cur-1", rows: 100 }"#,
            ),
            (
                Request::SqlClose {
                    id: "5".into(),
                    cont_id: "cur-1".into(),
                },
                r#"SqlClose { id: "5", cont_id: "cur-1" }"#,
            ),
            (
                Request::Cl {
                    id: "6".into(),
                    cmd: "WRKACTJOB".into(),
                    terse: None,
                },
                r#"Cl { id: "6", cmd: "WRKACTJOB", terse: None }"#,
            ),
            (
                Request::GetVersion { id: "7".into() },
                r#"GetVersion { id: "7" }"#,
            ),
            (
                Request::GetDbJob { id: "8".into() },
                r#"GetDbJob { id: "8" }"#,
            ),
            (
                Request::SetConfig {
                    id: "9".into(),
                    tracelevel: "ERRORS".into(),
                    tracedest: "IN_MEM".into(),
                },
                r#"SetConfig { id: "9", tracelevel: "ERRORS", tracedest: "IN_MEM" }"#,
            ),
            (
                Request::GetTraceData { id: "10".into() },
                r#"GetTraceData { id: "10" }"#,
            ),
            (
                Request::Dove {
                    id: "11".into(),
                    sql: "SELECT 1".into(),
                    terse: None,
                },
                r#"Dove { id: "11", sql: "SELECT 1", terse: None }"#,
            ),
            (Request::Ping { id: "12".into() }, r#"Ping { id: "12" }"#),
            (Request::Exit { id: "13".into() }, r#"Exit { id: "13" }"#),
        ];

        for (request, expected) in cases {
            assert_eq!(format!("{request:?}"), expected);
        }
    }

    #[test]
    fn exit_round_trips() {
        let r = Request::Exit { id: "3".into() };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"exit","id":"3"}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Exit { id } if id == "3"));
    }

    #[test]
    fn sql_round_trips_with_params() {
        let r = Request::Sql {
            id: "10".into(),
            sql: "SELECT * FROM ORDERS WHERE ID = ?".into(),
            rows: Some(100),
            parameters: Some(vec![serde_json::json!(42)]),
            terse: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"type":"sql","id":"10","sql":"SELECT * FROM ORDERS WHERE ID = ?","rows":100,"parameters":[42]}"#
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Sql { id, .. } if id == "10"));
    }

    #[test]
    fn sql_round_trips_minimal() {
        let r = Request::Sql {
            id: "11".into(),
            sql: "SELECT 1 FROM SYSIBM.SYSDUMMY1".into(),
            rows: None,
            parameters: None,
            terse: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        // Optional fields elided.
        assert!(!json.contains(r#""rows""#));
        assert!(!json.contains(r#""parameters""#));
        assert!(!json.contains(r#""terse""#));
        let _back: Request = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn sql_serializes_terse_true_and_omits_false() {
        let with_terse = Request::Sql {
            id: "12".into(),
            sql: "SELECT EMPNO, LASTNAME FROM EMPLOYEE".into(),
            rows: Some(5),
            parameters: None,
            terse: Some(true),
        };
        let json = serde_json::to_string(&with_terse).unwrap();
        assert_eq!(
            json,
            r#"{"type":"sql","id":"12","sql":"SELECT EMPNO, LASTNAME FROM EMPLOYEE","rows":5,"terse":true}"#
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back,
            Request::Sql {
                terse: Some(true),
                ..
            }
        ));
    }

    #[test]
    fn prepare_sql_round_trips() {
        let r = Request::PrepareSql {
            id: "12".into(),
            sql: "SELECT * FROM T WHERE A = ?".into(),
            terse: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::PrepareSql { id, .. } if id == "12"));
    }

    #[test]
    fn prepare_sql_execute_round_trips_batched() {
        let r = Request::PrepareSqlExecute {
            id: "13".into(),
            sql: "INSERT INTO T VALUES(?,?)".into(),
            parameters: Some(vec![
                vec![serde_json::json!(1), serde_json::json!("a")],
                vec![serde_json::json!(2), serde_json::json!("b")],
            ]),
            rows: None,
            terse: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        // `rows` is None → elided per skip_serializing_if; full shape pinned.
        assert_eq!(
            json,
            r#"{"type":"prepare_sql_execute","id":"13","sql":"INSERT INTO T VALUES(?,?)","parameters":[[1,"a"],[2,"b"]]}"#
        );
        let _back: Request = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn execute_round_trips() {
        let r = Request::Execute {
            id: "14".into(),
            cont_id: "stmt-7".into(),
            parameters: Some(vec![serde_json::json!("hello")]),
            rows: Some(100),
            terse: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"type":"execute","id":"14","cont_id":"stmt-7","parameters":["hello"],"rows":100}"#
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Execute { cont_id, .. } if cont_id == "stmt-7"));
    }

    #[test]
    fn prepare_sql_execute_single_set_flattens_on_wire() {
        let r = Request::PrepareSqlExecute {
            id: "13".into(),
            sql: "VALUES (CAST(? AS INTEGER))".into(),
            parameters: Some(vec![vec![serde_json::json!(7)]]),
            rows: Some(100),
            terse: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"type":"prepare_sql_execute","id":"13","sql":"VALUES (CAST(? AS INTEGER))","parameters":[7],"rows":100}"#
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        match back {
            Request::PrepareSqlExecute {
                parameters, rows, ..
            } => {
                assert_eq!(parameters, Some(vec![vec![serde_json::json!(7)]]));
                assert_eq!(rows, Some(100));
            }
            other => panic!("expected PrepareSqlExecute, got {other:?}"),
        }
    }

    #[test]
    fn prepare_sql_execute_accepts_nested_single_set() {
        let json =
            r#"{"type":"prepare_sql_execute","id":"13","sql":"VALUES (?)","parameters":[[7]]}"#;
        let back: Request = serde_json::from_str(json).unwrap();
        match back {
            Request::PrepareSqlExecute { parameters, .. } => {
                assert_eq!(parameters, Some(vec![vec![serde_json::json!(7)]]));
            }
            other => panic!("expected PrepareSqlExecute, got {other:?}"),
        }
    }

    #[test]
    fn sqlmore_round_trips() {
        let r = Request::SqlMore {
            id: "20".into(),
            cont_id: "cur-1".into(),
            rows: 100,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"type":"sqlmore","id":"20","cont_id":"cur-1","rows":100}"#
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::SqlMore { rows, .. } if rows == 100));
    }

    #[test]
    fn sqlclose_round_trips() {
        let r = Request::SqlClose {
            id: "21".into(),
            cont_id: "cur-1".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"sqlclose","id":"21","cont_id":"cur-1"}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::SqlClose { cont_id, .. } if cont_id == "cur-1"));
    }

    #[test]
    fn cl_round_trips() {
        let r = Request::Cl {
            id: "30".into(),
            cmd: "WRKACTJOB".into(),
            terse: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"cl","id":"30","cmd":"WRKACTJOB"}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Cl { cmd, .. } if cmd == "WRKACTJOB"));
    }

    #[test]
    fn metadata_requests_round_trip_with_bare_tags() {
        // Pin the bare-form wire tags for each metadata variant — the
        // per-variant #[serde(rename)] overrides exist precisely so these
        // serialize as `getversion`/`getdbjob`/`gettracedata` rather than
        // the snake_case auto-rename's `get_version`/etc.
        let cases: [(Request, &str); 3] = [
            (
                Request::GetVersion { id: "40".into() },
                r#"{"type":"getversion","id":"40"}"#,
            ),
            (
                Request::GetDbJob { id: "41".into() },
                r#"{"type":"getdbjob","id":"41"}"#,
            ),
            (
                Request::GetTraceData { id: "42".into() },
                r#"{"type":"gettracedata","id":"42"}"#,
            ),
        ];
        for (r, expected) in cases {
            let json = serde_json::to_string(&r).unwrap();
            assert_eq!(json, expected);
            // Round-trip back through the wire and confirm the tag still parses.
            let _back: Request = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn setconfig_round_trips() {
        let r = Request::SetConfig {
            id: "50".into(),
            tracelevel: "DATASTREAM".into(),
            tracedest: "FILE".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"type":"setconfig","id":"50","tracelevel":"DATASTREAM","tracedest":"FILE"}"#
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(back, Request::SetConfig { tracelevel, tracedest, .. }
                if tracelevel == "DATASTREAM" && tracedest == "FILE")
        );
    }

    #[test]
    fn setconfig_omits_empty_tracedest() {
        let r = Request::SetConfig {
            id: "50".into(),
            tracelevel: "OFF".into(),
            tracedest: String::new(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"setconfig","id":"50","tracelevel":"OFF"}"#);
        assert!(!json.contains("tracedest"));
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back,
            Request::SetConfig { tracedest, .. } if tracedest.is_empty()
        ));
    }

    #[test]
    fn setconfig_in_mem_is_present() {
        let r = Request::SetConfig {
            id: "50".into(),
            tracelevel: "ON".into(),
            tracedest: "IN_MEM".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"type":"setconfig","id":"50","tracelevel":"ON","tracedest":"IN_MEM"}"#
        );
        assert!(!json.contains(r#""tracedest":"""#));
    }

    #[test]
    fn dove_round_trips() {
        let r = Request::Dove {
            id: "60".into(),
            sql: "SELECT * FROM T".into(),
            terse: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"dove","id":"60","sql":"SELECT * FROM T"}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Dove { id, .. } if id == "60"));
    }
}
