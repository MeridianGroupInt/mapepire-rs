//! Request messages — outgoing wire types. Variants added in subsequent tasks.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Discriminated union of all request types the client can send.
///
/// Tagged on the wire by the `type` field. Variants are added in
/// subsequent protocol tasks.
///
/// [`Debug`] is hand-written, not derived: it renders `Connect`'s password
/// as `[REDACTED]` so a downstream `tracing::debug!("{req:?}")` cannot put
/// an IBM i password in a log. `Serialize` is untouched — the plaintext
/// password is what goes on the wire, inside TLS.
#[non_exhaustive]
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Establish a daemon session and authenticate.
    Connect {
        /// Caller-supplied correlation id.
        id: String,
        /// IBM i user profile.
        user: String,
        /// IBM i password (plain — the WebSocket is TLS).
        password: String,
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
    },

    /// Prepare a SQL statement without executing.
    PrepareSql {
        /// Caller-supplied correlation id.
        id: String,
        /// SQL text.
        sql: String,
    },

    /// Prepare and execute in one round-trip; supports batched parameters.
    PrepareSqlExecute {
        /// Caller-supplied correlation id.
        id: String,
        /// SQL text.
        sql: String,
        /// One or more parameter sets. A vector of vectors yields one
        /// execution per inner set.
        #[serde(skip_serializing_if = "Option::is_none")]
        parameters: Option<Vec<Vec<serde_json::Value>>>,
        /// Initial page size for the resulting cursor (per execution).
        #[serde(skip_serializing_if = "Option::is_none")]
        rows: Option<u32>,
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
        /// Tracing level — opaque server-defined string.
        tracelevel: String,
        /// Trace destination — opaque server-defined string.
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

/// Stand-in for a secret field in [`Request`]'s [`Debug`] output.
///
/// Renders bare `[REDACTED]` rather than the `"[REDACTED]"` a `&str` would
/// print, matching [`Password`](crate::Password)'s `Password([REDACTED])`.
struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Hand-written to redact `Connect { password }`.
///
/// The match is exhaustive with **no wildcard arm** on purpose. `Request` is
/// `#[non_exhaustive]`, which constrains downstream crates but not this one,
/// so adding a variant breaks this `impl` at compile time and forces a
/// deliberate decision about whether the new variant carries a secret. A
/// `_ => ...` arm would silently print one.
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect {
                id,
                user,
                password: _,
            } => f
                .debug_struct("Connect")
                .field("id", id)
                .field("user", user)
                .field("password", &Redacted)
                .finish(),
            Self::Sql {
                id,
                sql,
                rows,
                parameters,
            } => f
                .debug_struct("Sql")
                .field("id", id)
                .field("sql", sql)
                .field("rows", rows)
                .field("parameters", parameters)
                .finish(),
            Self::PrepareSql { id, sql } => f
                .debug_struct("PrepareSql")
                .field("id", id)
                .field("sql", sql)
                .finish(),
            Self::PrepareSqlExecute {
                id,
                sql,
                parameters,
                rows,
            } => f
                .debug_struct("PrepareSqlExecute")
                .field("id", id)
                .field("sql", sql)
                .field("parameters", parameters)
                .field("rows", rows)
                .finish(),
            Self::Execute {
                id,
                cont_id,
                parameters,
            } => f
                .debug_struct("Execute")
                .field("id", id)
                .field("cont_id", cont_id)
                .field("parameters", parameters)
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
            Self::Cl { id, cmd } => f
                .debug_struct("Cl")
                .field("id", id)
                .field("cmd", cmd)
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
            Self::Dove { id, sql } => f
                .debug_struct("Dove")
                .field("id", id)
                .field("sql", sql)
                .finish(),
            Self::Ping { id } => f.debug_struct("Ping").field("id", id).finish(),
            Self::Exit { id } => f.debug_struct("Exit").field("id", id).finish(),
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
    fn connect_round_trips() {
        let r = Request::Connect {
            id: "2".into(),
            user: "DCURTIS".into(),
            password: "hunter2".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            json,
            r#"{"type":"connect","id":"2","user":"DCURTIS","password":"hunter2"}"#
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Connect { user, .. } if user == "DCURTIS"));
    }

    #[test]
    fn connect_debug_redacts_password() {
        let r = Request::Connect {
            id: "2".into(),
            user: "DCURTIS".into(),
            password: "hunter2".into(),
        };
        let s = format!("{r:?}");
        assert!(!s.contains("hunter2"), "password leaked into Debug: {s}");
        assert!(s.contains("[REDACTED]"), "missing redaction marker: {s}");
        // Non-secret fields still render faithfully.
        assert!(s.contains("DCURTIS"), "user missing from Debug: {s}");
        assert!(s.contains(r#"id: "2""#), "id missing from Debug: {s}");
    }

    #[test]
    fn non_secret_variants_debug_faithfully() {
        let r = Request::SqlMore {
            id: "20".into(),
            cont_id: "cur-1".into(),
            rows: 100,
        };
        assert_eq!(
            format!("{r:?}"),
            r#"SqlMore { id: "20", cont_id: "cur-1", rows: 100 }"#
        );
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
        };
        let json = serde_json::to_string(&r).unwrap();
        // Optional fields elided.
        assert!(!json.contains(r#""rows""#));
        assert!(!json.contains(r#""parameters""#));
        let _back: Request = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn prepare_sql_round_trips() {
        let r = Request::PrepareSql {
            id: "12".into(),
            sql: "SELECT * FROM T WHERE A = ?".into(),
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
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Execute { cont_id, .. } if cont_id == "stmt-7"));
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
            matches!(back, Request::SetConfig { tracelevel, .. } if tracelevel == "DATASTREAM")
        );
    }

    #[test]
    fn dove_round_trips() {
        let r = Request::Dove {
            id: "60".into(),
            sql: "SELECT * FROM T".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"dove","id":"60","sql":"SELECT * FROM T"}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Request::Dove { id, .. } if id == "60"));
    }
}
