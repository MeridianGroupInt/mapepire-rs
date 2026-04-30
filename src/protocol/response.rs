//! Response messages — incoming wire types. Variants added in subsequent tasks.

use serde::{Deserialize, Serialize};

/// Discriminated union of all response types the server may send.
///
/// Tagged on the wire by the `type` field. Variants are added in
/// subsequent protocol tasks.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    /// Health-check response.
    Pong {
        /// Echoes the request id.
        id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pong_round_trips() {
        let r = Response::Pong {
            id: "test-1".into(),
        };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"pong","id":"test-1"}"#);
        let back: Response = serde_json::from_str(&json).unwrap();
        let Response::Pong { id } = back;
        assert_eq!(id, "test-1");
    }
}
