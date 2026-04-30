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
    /// Health-check request. Server echoes `pong`.
    Ping {
        /// Caller-supplied correlation id.
        id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_round_trips() {
        let r = Request::Ping {
            id: "test-1".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"ping","id":"test-1"}"#);
        let back: Request = serde_json::from_str(&json).unwrap();
        let Request::Ping { id } = back;
        assert_eq!(id, "test-1");
    }
}
