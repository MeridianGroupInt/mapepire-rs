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
    }
}
