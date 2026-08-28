//! High-level handshake: TCP → TLS → WebSocket Upgrade → Connect request.
//!
//! Returns a fully-initialized [`Dispatcher`] ready for `Job` to use.
//!
//! # Error mapping
//!
//! | Stage                    | Error variant                          |
//! |--------------------------|----------------------------------------|
//! | TCP + TLS                | `Error::Transport(...)` (via `?`)      |
//! | Upgrade HTTP 401/403     | `Error::Auth(...)`                     |
//! | Other WebSocket upgrade  | `Error::Internal(...)`                 |
//! | Malformed WS request     | `Error::Internal(...)`                 |
//! | Auth rejected by server  | `Error::Auth(...)`                     |
//! | Unexpected response type | `Error::Protocol(CorrelationMismatch)` |

use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use base64::Engine;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::handshake::client::{Request as WsRequest, generate_key};
use zeroize::Zeroizing;

use crate::config::DaemonServer;
use crate::error::{Error, ProtocolError};
use crate::protocol::{IdAllocator, Request, Response};
use crate::transport::dispatcher::Dispatcher;
use crate::transport::socket::WsTransport;
use crate::transport::tls;

/// Live Mapepire daemon request-target (trailing slash). Jetty 404s `/db2`.
const WS_PATH: &str = "/db/";

/// A fully-initialized dispatcher, together with the metadata returned by
/// the daemon's `connected` response.
///
/// `Job` receives one of these from [`connect`] and holds the fields for
/// the lifetime of the connection.
pub(crate) struct ConnectedDispatcher {
    /// Live dispatcher task; owns the WebSocket connection.
    pub(crate) dispatcher: Dispatcher,
    /// Daemon-reported version string (e.g., `"2.3.5"`).
    pub(crate) version: String,
    /// Db2 job name assigned by the server for this session
    /// (e.g., `"QZDASOINIT/QUSER/123456"`).
    pub(crate) initial_job: String,
    /// Id allocator seeded with the prefix established during this handshake.
    /// `Job` reuses it for all subsequent requests so ids stay unique
    /// across the session.
    pub(crate) ids: IdAllocator,
    /// Outstanding-request counter shared with the dispatcher task.
    /// `Job` clones this into `JobInner` so the v0.3 pool router can
    /// observe the count without owning the dispatcher. The dispatcher
    /// task increments after each socket write and decrements on
    /// response routing or socket-close drain.
    pub(crate) in_flight: Arc<AtomicU32>,
}

/// Run the full client handshake.
///
/// Performs TCP connect → TLS handshake → WebSocket upgrade → `connect`
/// wire request, and returns a [`ConnectedDispatcher`] ready for `Job` to
/// use.
///
/// # Errors
///
/// Returns [`Error::Transport`] if the TCP or TLS layer fails,
/// [`Error::Auth`] if the upgrade is rejected with HTTP 401/403 or the
/// daemon rejects the credentials,
/// [`Error::Internal`] if the WebSocket upgrade fails for another reason,
/// or [`Error::Protocol`] if the response does not match the expected shape.
pub(crate) async fn connect(server: &DaemonServer) -> crate::Result<ConnectedDispatcher> {
    // 1. TCP + TLS.
    let tls_stream = tls::connect(server).await?;

    // 2. WebSocket Upgrade. Live Jetty Mapepire serves `/db/` and 403s without HTTP Basic. Password
    //    is not placed on the query string or in the subsequent JSON `Connect` body. URI host and
    //    HTTP Host are the TLS name (`server.host`), not `connect_address`.
    let url = format!("wss://{}:{}{WS_PATH}", server.host, server.port);
    let authorization = basic_authorization(&server.user, server.password.expose());
    let ws_request = WsRequest::builder()
        .uri(&url)
        .header("Host", &server.host)
        .header("Authorization", &authorization)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key())
        .body(())
        .map_err(|e| Error::Internal(format!("malformed ws request: {e}")))?;
    // Copied into the request headers; do not log this value.
    drop(authorization);

    let (ws_stream, _http_response) = match client_async(ws_request, tls_stream).await {
        Ok(pair) => pair,
        Err(e) => return Err(map_upgrade_error(e)),
    };

    // 3. Spawn dispatcher around the now-framed stream. The shared `in_flight` counter starts at
    //    zero; the dispatcher task and `JobInner` each hold an `Arc` clone. The handshake's
    //    `Connect` request below increments to 1, then the matching `Connected` response decrements
    //    back to 0 — so a freshly-returned `ConnectedDispatcher` always reports `in_flight == 0`.
    let transport = WsTransport::new(ws_stream);
    let in_flight = Arc::new(AtomicU32::new(0));
    let dispatcher = Dispatcher::spawn(Box::pin(transport), Arc::clone(&in_flight));
    let handle = dispatcher.handle();

    // 4. Send the Connect request and await the Connected response. Live daemon auth is HTTP Basic
    //    on the upgrade, not this body. Sibling SQLJob: {id, type:connect, technique:tcp,
    //    application, props?}.
    let ids = IdAllocator::new();
    let connect_id = ids.next();
    let request = Request::Connect {
        id: connect_id.clone(),
        technique: "tcp".into(),
        application: server.application.clone(),
        props: server.jdbc_props.clone(),
    };

    let response = handle.send(request).await?;
    let (version, initial_job) = match response {
        Response::Connected { version, job, .. } => (version, job),
        Response::Error(e) => {
            return Err(Error::Auth(
                e.error.unwrap_or_else(|| "connect rejected".into()),
            ));
        }
        other => {
            return Err(Error::from(ProtocolError::CorrelationMismatch {
                expected: connect_id,
                got: format!("{other:?}"),
            }));
        }
    };

    Ok(ConnectedDispatcher {
        dispatcher,
        version,
        initial_job,
        ids,
        in_flight,
    })
}

/// Build `Authorization: Basic …` from `user:password`.
///
/// Concatenates into a [`zeroize::Zeroizing<String>`] so the plaintext pair
/// is wiped after Base64 encoding. The returned header value is not logged.
fn basic_authorization(user: &str, password: &str) -> String {
    let material = Zeroizing::new(format!("{user}:{password}"));
    let encoded = base64::engine::general_purpose::STANDARD.encode(&*material);
    format!("Basic {encoded}")
}

/// Map a tungstenite upgrade failure onto crate [`Error`].
///
/// tungstenite 0.30's `Error::Http` holds `Box<http::Response<Option<Vec<u8>>>>`
/// (not a bare `Response`). HTTP 401/403 are Jetty's credential gate and
/// become [`Error::Auth`]; every other upgrade failure is [`Error::Internal`].
fn map_upgrade_error(e: tokio_tungstenite::tungstenite::Error) -> Error {
    use tokio_tungstenite::tungstenite::Error as WsError;
    match e {
        WsError::Http(res) if matches!(res.status().as_u16(), 401 | 403) => {
            Error::Auth(format!("websocket upgrade rejected: HTTP {}", res.status()))
        }
        other => Error::Internal(format!("websocket upgrade failed: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tokio_tungstenite::tungstenite::Error as WsError;
    use tokio_tungstenite::tungstenite::http::{Response, StatusCode};

    use super::*;

    fn http_error(status: StatusCode) -> WsError {
        let res = Response::builder()
            .status(status)
            .body(None)
            .expect("status-only HTTP response");
        WsError::Http(Box::new(res))
    }

    #[test]
    fn test_basic_authorization_encodes_user_password() {
        let header = basic_authorization("USER", "s3cret");
        assert!(
            header.starts_with("Basic "),
            "header should be Basic, got {header:?}"
        );
        let b64 = header.trim_start_matches("Basic ");
        let raw = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .expect("standard base64");
        assert_eq!(raw, b"USER:s3cret");
    }

    #[test]
    fn test_map_upgrade_error_http_403_is_auth() {
        match map_upgrade_error(http_error(StatusCode::FORBIDDEN)) {
            Error::Auth(msg) => {
                assert!(
                    msg.contains("403"),
                    "Auth message should mention 403, got {msg}"
                );
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn test_map_upgrade_error_http_401_is_auth() {
        match map_upgrade_error(http_error(StatusCode::UNAUTHORIZED)) {
            Error::Auth(msg) => {
                assert!(
                    msg.contains("401"),
                    "Auth message should mention 401, got {msg}"
                );
            }
            other => panic!("expected Auth, got {other:?}"),
        }
    }

    #[test]
    fn test_map_upgrade_error_http_404_is_internal() {
        match map_upgrade_error(http_error(StatusCode::NOT_FOUND)) {
            Error::Internal(msg) => {
                assert!(
                    msg.contains("404"),
                    "Internal message should mention 404, got {msg}"
                );
            }
            other => panic!("expected Internal, got {other:?}"),
        }
    }
}
