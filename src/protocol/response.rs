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
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::Connected { version, job, .. } => {
                assert_eq!(version, "2.3.5");
                assert_eq!(job, "QZDASOINIT/QUSER/123456");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn exited_round_trips() {
        let r = Response::Exited { id: "3".into() };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"type":"exited","id":"3"}"#);
    }
}
