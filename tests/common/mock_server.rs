//! Mock TLS+WebSocket server for integration tests.
//!
//! The mock binds to `127.0.0.1:0` (OS-assigned port), wraps each
//! accepted TCP stream in TLS using a baked-at-test-time self-signed cert,
//! and completes the WebSocket upgrade. It then reads inbound JSON frames
//! as [`Request`] values and emits predetermined [`Response`] JSON frames
//! based on the [`MockBehavior`] configured at spawn time.
//!
//! **Single-connection per spawn.** Each call to [`spawn_mock`] handles
//! exactly ONE accepted connection. Phase 6 integration tests must call
//! [`spawn_mock`] (or [`super::spawn_mock_and_connect`]) once per test — the
//! mock panics if a second connection arrives.
//!
//! **No SQL parsing.** The mock dispatches on the *type* of the inbound
//! request, not the SQL text. It returns canned responses.
//!
//! **No `unsafe`.** Test-style `.unwrap()` / `.expect()` are used freely
//! throughout since panics become test failures.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use futures::{SinkExt, StreamExt};
use mapepire::protocol::{ErrorResponse, QueryResult, Request, Response};
use rustls::ServerConfig;
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

/// Optional recorder that captures every [`Request`] received by the mock.
///
/// Tests that need to assert "the mock observed a particular request" share
/// the inner `Vec<Request>` between the mock task and the test thread by
/// cloning the `Arc`. Used by Cleanup D's drop-rows tests to confirm that
/// best-effort `SqlClose` requests reached the wire.
pub type RequestRecorder = Arc<Mutex<Vec<Request>>>;

/// Pre-programmed response behavior for a mock server instance.
///
/// Each variant controls what the mock sends back when a client connects
/// and issues requests. The mock echoes each request's `id` field in every
/// response so the client-side dispatcher's correlation logic works correctly.
///
/// Phase 6 integration tests (Tasks 22–30) each use a different variant.
/// Because each test binary compiles `common` independently, the dead-code
/// lint sees variants that are live in other binaries as unused. The enum-level
/// `#[allow(dead_code)]` silences this without per-variant noise.
#[allow(dead_code)]
#[derive(Clone)]
pub enum MockBehavior {
    /// Accept the WebSocket upgrade and respond to a [`Request::Connect`]
    /// with a successful [`Response::Connected`]. After that:
    /// - [`Request::Exit`] causes the mock to send [`Response::Exited`] and close the connection.
    /// - Any other request gets a [`Response::Pong`] (a no-op echo useful for probing ping /
    ///   round-trip behavior in tests).
    AcceptAndConnect,

    /// Accept the WebSocket upgrade but respond to [`Request::Connect`] with
    /// a [`Response::Error`] carrying the provided message. Simulates an
    /// authentication-rejection scenario.
    AuthFail(String),

    /// Accept connect with success, then respond to the first
    /// SQL-variant request (`Sql`, `PrepareSqlExecute`, or `Execute`) with
    /// the first entry in `pages`. Subsequent [`Request::SqlMore`] requests
    /// consume additional entries. [`Request::SqlClose`] is acknowledged
    /// with [`Response::SqlClosed`] (so dispatcher correlation is exercised
    /// rather than falling through to the catch-all Pong arm). Any other
    /// request after connect gets a [`Response::Pong`].
    ///
    /// When `recorder` is `Some`, every received [`Request`] (after the
    /// initial Connect) is appended to the shared `Vec`. Tests retain a
    /// clone of the `Arc` to assert what the mock observed.
    // NOTE: used by Tasks 24 (PRO-420), 26 (PRO-422), and Cleanup D's
    // drop-rows tests for SQL one-shot, paging, and cursor-close
    // observability respectively.
    Pages {
        /// Pre-baked [`QueryResult`] pages drained in order.
        pages: Vec<QueryResult>,
        /// Optional recorder — when `Some`, every [`Request`] (after
        /// connect) is appended to the shared `Vec` for test assertions.
        recorder: Option<RequestRecorder>,
    },

    /// Accept connect with success, then respond to the very next request
    /// (of any type) with the provided [`ErrorResponse`]. After that, exit
    /// cleanly — do not respond to further requests.
    // NOTE: used by Task 29 (PRO-425) integration test for server-side error classification.
    ReturnError(ErrorResponse),

    /// Accept connect with success, then silently drop the request loop
    /// without closing the socket. Simulates a half-open / server-stall
    /// scenario for timeout tests.
    // NOTE: used by Task 30 (PRO-426) integration test for half-open socket.
    HalfOpen,

    /// Accept connect with success, then respond to the protocol sequence for
    /// prepared statements:
    /// - The next [`Request::PrepareSql`] request: emit [`Response::PreparedStatement`] with
    ///   `cont_id`.
    /// - Each subsequent [`Request::Execute`]: pop the next [`QueryResult`] from `results`, stamp
    ///   its `id`, and send as [`Response::QueryResult`].
    /// - Each [`Request::SqlClose`]: emit [`Response::SqlClosed`] and continue (Drop-for-Query may
    ///   fire one per test after assertions).
    /// - [`Request::Exit`]: emit [`Response::Exited`] and close.
    // NOTE: used by Task 25 (PRO-421) integration test for prepared statements.
    PrepareAndExecute {
        /// Server-side prepared-statement handle sent back in `PreparedStatement`.
        cont_id: String,
        /// Canned `QueryResult` values consumed in order by each `Execute`.
        results: Vec<QueryResult>,
    },
}

/// Mock daemon version string echoed in [`Response::Connected`].
const MOCK_VERSION: &str = "0.0.0-mock";
/// Mock Db2 job name echoed in [`Response::Connected`].
const MOCK_JOB: &str = "MOCK/QUSER/000001";

/// Spawn a mock TLS+WebSocket server bound to `127.0.0.1:0`.
///
/// Returns the bound [`SocketAddr`] (so tests can connect to
/// `wss://127.0.0.1:<port>/db2`) and the self-signed cert as DER bytes
/// (so tests using [`mapepire::TlsConfig::Ca`] can pin it).
///
/// The spawned task handles exactly **one** TCP connection, then exits.
/// Spawn a fresh mock per test function.
///
/// # Panics
///
/// Must be called from within a tokio async context (i.e., inside a
/// `#[tokio::test]` function or similar). Panics if called outside a runtime.
pub fn spawn_mock(behavior: MockBehavior) -> (SocketAddr, Vec<u8>) {
    // Mint a self-signed cert for 127.0.0.1. generate_simple_self_signed
    // auto-detects the string as an IP address and emits an IP SAN.
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])
            .expect("rcgen self-signed cert");

    // DER bytes for the cert — returned to the caller for TlsConfig::Ca pinning.
    let cert_der: Vec<u8> = cert.der().as_ref().to_vec();

    // PKCS#8 DER bytes for the private key — used to build the server config.
    let key_der = signing_key.serialize_der();

    // Build rustls ServerConfig with the self-signed cert.
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert_der.clone())],
            PrivatePkcs8KeyDer::from(key_der).into(),
        )
        .expect("rustls ServerConfig");

    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    // Bind using std::net::TcpListener (synchronous — no runtime needed)
    // and immediately convert to tokio for async I/O. This avoids calling
    // block_on inside an already-running tokio runtime.
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock listener");
    std_listener.set_nonblocking(true).expect("set_nonblocking");
    let addr = std_listener.local_addr().expect("mock local_addr");
    let listener = TcpListener::from_std(std_listener).expect("convert to tokio listener");

    tokio::spawn(async move {
        let (tcp_stream, _peer) = listener.accept().await.expect("mock accept");
        let tls_stream = acceptor
            .accept(tcp_stream)
            .await
            .expect("mock TLS handshake");
        let ws_stream = accept_async(tls_stream)
            .await
            .expect("mock WebSocket upgrade");

        run_mock(ws_stream, behavior).await;
    });

    (addr, cert_der)
}

/// Drive the mock request/response loop for one connection.
// run_mock uses two local macros (send_response!, recv_request!) that borrow
// both `sink` and `stream` from the enclosing scope. Extracting sub-behaviors
// into helper functions would require passing both halves as parameters,
// making the API noisier than the long-function version. The length is
// structural, not complexity creep.
#[allow(clippy::too_many_lines)]
async fn run_mock<S>(ws_stream: tokio_tungstenite::WebSocketStream<S>, behavior: MockBehavior)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws_stream.split();

    // Helper: serialize a Response and send it as a text frame.
    macro_rules! send_response {
        ($resp:expr) => {{
            let json = serde_json::to_string(&$resp).expect("serialize response");
            sink.send(Message::Text(json.into()))
                .await
                .expect("send response frame");
        }};
    }

    // Helper: read the next text frame and deserialize as a Request.
    // Returns None if the stream is closed.
    macro_rules! recv_request {
        () => {{
            loop {
                match stream.next().await {
                    Some(Ok(Message::Text(t))) => {
                        break Some(
                            serde_json::from_str::<Request>(&t).expect("deserialize request"),
                        );
                    }
                    Some(Ok(Message::Ping(data))) => {
                        // Respond to WebSocket-level pings (not Mapepire pings).
                        sink.send(Message::Pong(data)).await.expect("send ws pong");
                    }
                    Some(Ok(Message::Close(_))) | None => break None,
                    // Binary, Pong, Frame — skip.
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => panic!("mock recv error: {e}"),
                }
            }
        }};
    }

    // Step 1: wait for the Connect request (required by all behaviors).
    let connect_id = match recv_request!() {
        Some(Request::Connect { id, .. }) => id,
        other => panic!("mock expected Connect, got {other:?}"),
    };

    match behavior {
        MockBehavior::AuthFail(msg) => {
            send_response!(Response::Error(ErrorResponse {
                id: connect_id,
                success: false,
                sqlstate: None,
                sqlcode: None,
                error: Some(msg),
                job: None,
            }));
            // Close after auth failure.
            let _ = sink.send(Message::Close(None)).await;
        }

        MockBehavior::AcceptAndConnect => {
            send_response!(Response::Connected {
                id: connect_id,
                version: MOCK_VERSION.into(),
                job: MOCK_JOB.into(),
            });
            // Request loop: Exit closes cleanly; anything else gets Pong.
            loop {
                match recv_request!() {
                    None => break,
                    Some(Request::Exit { id }) => {
                        send_response!(Response::Exited { id });
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    }
                    Some(req) => {
                        let id = request_id(&req);
                        send_response!(Response::Pong { id });
                    }
                }
            }
        }

        MockBehavior::Pages {
            pages: mut pages_vec,
            recorder,
        } => {
            send_response!(Response::Connected {
                id: connect_id,
                version: MOCK_VERSION.into(),
                job: MOCK_JOB.into(),
            });
            let mut pages_iter = pages_vec.drain(..);
            loop {
                match recv_request!() {
                    None => break,
                    Some(req) => {
                        if let Some(rec) = &recorder {
                            // Test holds the read end of this Mutex and may
                            // be polling concurrently — push, then release
                            // the lock immediately. Clone is cheap; Request
                            // is one heap allocation per text/SQL field.
                            rec.lock()
                                .expect("recorder mutex not poisoned")
                                .push(req.clone());
                        }
                        match req {
                            Request::Exit { id } => {
                                send_response!(Response::Exited { id });
                                let _ = sink.send(Message::Close(None)).await;
                                break;
                            }
                            Request::Sql { id, .. }
                            | Request::PrepareSqlExecute { id, .. }
                            | Request::Execute { id, .. }
                            | Request::SqlMore { id, .. } => {
                                let mut page = pages_iter
                                    .next()
                                    .expect("mock Pages ran out of pre-baked pages");
                                page.id = id;
                                send_response!(Response::QueryResult(page));
                            }
                            Request::SqlClose { id, .. } => {
                                // Explicit ack so the dispatcher's
                                // correlation logic isn't relying on the
                                // Pong fallback.
                                send_response!(Response::SqlClosed { id, success: true });
                            }
                            other => {
                                let id = request_id(&other);
                                send_response!(Response::Pong { id });
                            }
                        }
                    }
                }
            }
        }

        MockBehavior::ReturnError(mut err) => {
            send_response!(Response::Connected {
                id: connect_id,
                version: MOCK_VERSION.into(),
                job: MOCK_JOB.into(),
            });
            // Wait for the first request after connect.
            // If it is Exit, close normally; otherwise send the canned error
            // and exit cleanly — do not respond to further requests.
            match recv_request!() {
                None => {}
                Some(Request::Exit { id }) => {
                    send_response!(Response::Exited { id });
                    let _ = sink.send(Message::Close(None)).await;
                }
                Some(req) => {
                    err.id = request_id(&req);
                    send_response!(Response::Error(err.clone()));
                    // Exit cleanly per doc — do not respond to further requests.
                }
            }
        }

        MockBehavior::HalfOpen => {
            send_response!(Response::Connected {
                id: connect_id,
                version: MOCK_VERSION.into(),
                job: MOCK_JOB.into(),
            });
            // Drain incoming frames and discard them — never respond.
            // The socket stays open until the test runtime shuts down.
            while let Some(msg) = stream.next().await {
                match msg {
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {} // silently discard
                }
            }
        }

        MockBehavior::PrepareAndExecute {
            cont_id,
            mut results,
        } => {
            send_response!(Response::Connected {
                id: connect_id,
                version: MOCK_VERSION.into(),
                job: MOCK_JOB.into(),
            });
            let mut results_iter = results.drain(..);
            loop {
                match recv_request!() {
                    None => break,
                    Some(Request::Exit { id }) => {
                        send_response!(Response::Exited { id });
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    }
                    Some(Request::PrepareSql { id, .. }) => {
                        send_response!(Response::PreparedStatement {
                            id,
                            success: true,
                            cont_id: cont_id.clone(),
                            execution_time: 0.0,
                        });
                    }
                    Some(Request::Execute { id, .. }) => {
                        let mut qr = results_iter
                            .next()
                            .expect("mock PrepareAndExecute ran out of pre-baked results");
                        qr.id = id;
                        send_response!(Response::QueryResult(qr));
                    }
                    Some(Request::SqlClose { id, .. }) => {
                        // Continue rather than break — Drop for Query fires SqlClose
                        // after the test's assertions and must not stall the server.
                        send_response!(Response::SqlClosed { id, success: true });
                    }
                    Some(req) => {
                        let id = request_id(&req);
                        send_response!(Response::Pong { id });
                    }
                }
            }
        }
    }
}

/// Extract the correlation id from any [`Request`] variant.
fn request_id(req: &Request) -> String {
    match req {
        Request::Connect { id, .. }
        | Request::Sql { id, .. }
        | Request::PrepareSql { id, .. }
        | Request::PrepareSqlExecute { id, .. }
        | Request::Execute { id, .. }
        | Request::SqlMore { id, .. }
        | Request::SqlClose { id, .. }
        | Request::Cl { id, .. }
        | Request::GetVersion { id }
        | Request::GetDbJob { id }
        | Request::SetConfig { id, .. }
        | Request::GetTraceData { id }
        | Request::Dove { id, .. }
        | Request::Ping { id }
        | Request::Exit { id } => id.clone(),
        // The enum is #[non_exhaustive]; catch any future variants.
        _ => "unknown".into(),
    }
}
