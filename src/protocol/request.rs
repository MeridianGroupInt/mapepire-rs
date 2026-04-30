//! Request messages — outgoing wire types. Variants added in subsequent tasks.

use serde::{Deserialize, Serialize};

/// Discriminated union of all request types the client can send.
///
/// Tagged on the wire by the `type` field. Variants are added in
/// subsequent protocol tasks.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}
