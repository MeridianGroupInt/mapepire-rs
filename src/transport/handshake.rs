//! High-level handshake: TCP → TLS → WebSocket Upgrade → Connect request.
//!
//! Returns a fully-initialized [`Dispatcher`] ready for `Job` to use.
//!
//! # Error mapping
//!
//! | Stage                    | Error variant                            |
//! |--------------------------|------------------------------------------|
//! | TCP + TLS                | `Error::Transport(...)` (via `?`)        |
//! | WebSocket upgrade        | `Error::Internal(...)`                   |
//! | Malformed WS request     | `Error::Internal(...)`                   |
//! | Auth rejected by server  | `Error::Auth(...)`                       |
//! | Unexpected response type | `Error::Protocol(CorrelationMismatch)`   |

use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::handshake::client::{Request as WsRequest, generate_key};

use crate::config::DaemonServer;
use crate::error::{Error, ProtocolError};
use crate::protocol::{IdAllocator, Request, Response};
use crate::transport::dispatcher::Dispatcher;
use crate::transport::socket::WsTransport;
use crate::transport::tls;

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
/// [`Error::Internal`] if the WebSocket upgrade fails,
/// [`Error::Auth`] if the daemon rejects the credentials, or
/// [`Error::Protocol`] if the response does not match the expected shape.
pub(crate) async fn connect(server: &DaemonServer) -> Result<ConnectedDispatcher, Error> {
    // 1. TCP + TLS.
    let tls_stream = tls::connect(server).await?;

    // 2. WebSocket Upgrade.
    let url = format!("wss://{}:{}/db2", server.host, server.port);
    let ws_request = WsRequest::builder()
        .uri(&url)
        .header("Host", &server.host)
        .header("Upgrade", "websocket")
        .header("Connection", "Upgrade")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key())
        .body(())
        .map_err(|e| Error::Internal(format!("malformed ws request: {e}")))?;

    let (ws_stream, _http_response) = client_async(ws_request, tls_stream)
        .await
        .map_err(|e| Error::Internal(format!("websocket upgrade failed: {e}")))?;

    // 3. Spawn dispatcher around the now-framed stream.
    let transport = WsTransport::new(ws_stream);
    let dispatcher = Dispatcher::spawn(Box::pin(transport));
    let handle = dispatcher.handle();

    // 4. Send the Connect request and await the Connected response.
    let ids = IdAllocator::new();
    let connect_id = ids.next();
    let request = Request::Connect {
        id: connect_id.clone(),
        user: server.user.clone(),
        // Security note: `.to_string()` clones the plaintext into a
        // non-zeroizing `String` field of `Request::Connect`. The bytes
        // sit in heap memory until the allocator reuses the page after
        // the `Request` is dropped post-serialization. Accepted tradeoff
        // at the wire-protocol boundary; see SECURITY.md and the
        // `Password::expose` doc.
        password: server.password.expose().to_string(),
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
    })
}
