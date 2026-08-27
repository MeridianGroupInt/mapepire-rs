//! Mock TLS+WebSocket server for integration tests.
//!
//! The mock binds to `127.0.0.1:0` (OS-assigned port), wraps each
//! accepted TCP stream in TLS using a baked-at-test-time self-signed cert,
//! and completes the WebSocket upgrade. It then reads inbound JSON frames
//! as [`Request`] values and emits predetermined [`Response`] JSON frames.
//!
//! Two flavors are exposed:
//!
//! - [`spawn_mock`] — **single-connection per spawn**, configured by [`MockBehavior`]. Each call
//!   accepts exactly ONE connection and exits. Used by Phase 6 integration tests (Tasks 22–25,
//!   Cleanups D & E) that exercise specific protocol shapes.
//! - [`spawn_pool_mock`] — **multi-connection**, generic happy-path responses, with shared
//!   observation state surfaced through [`MockHandle`]. Used by Phase 9 pool integration tests
//!   (Tasks 26–30) where multiple `Job`s must coexist behind one mock and the test asserts on
//!   cross-connection behavior (which socket saw what, in what order).
//!
//! **No SQL parsing.** Both mocks dispatch on the *type* of the inbound
//! request, not the SQL text. They return canned responses.
//!
//! **Live response dialect.** `Connected`, `QueryResult`, and `Error` are
//! sent untagged (no `"type"` field), matching Mapepire-on-i. Other variants
//! keep internally tagged serde so `Pong` remains `{"type":"pong",...}`.
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
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as WsUpgradeRequest, Response as WsUpgradeResponse,
};
use tokio_tungstenite::tungstenite::http::{Response as HttpResponse, StatusCode};

/// Optional recorder that captures every [`Request`] received by the mock.
///
/// Tests that need to assert "the mock observed a particular request" share
/// the inner `Vec<Request>` between the mock task and the test thread by
/// cloning the `Arc`. Used by Cleanup D's drop-rows tests to confirm that
/// best-effort `SqlClose` requests reached the wire.
pub type RequestRecorder = Arc<Mutex<Vec<Request>>>;

/// Observed WebSocket-upgrade request-target and `Authorization` header.
///
/// Shared between the mock accept callback and the test thread. One probe
/// covers both path and Basic assertions so tests do not need two spawn
/// entry points. Cheap to clone (`Arc`).
///
/// Each integration-test binary compiles `common` independently, so
/// binaries that never record the upgrade see this type as unused.
#[allow(dead_code)]
#[derive(Clone, Default)]
pub struct UpgradeProbe {
    path: Arc<Mutex<Option<String>>>,
    authorization: Arc<Mutex<Option<String>>>,
}

#[allow(dead_code)]
impl UpgradeProbe {
    /// Empty probe — neither field observed yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Request-target the mock saw on the upgrade (`"/db/"`, `"/db2"`, …).
    #[must_use]
    pub fn path(&self) -> Option<String> {
        self.path
            .lock()
            .expect("upgrade probe mutex not poisoned")
            .clone()
    }

    /// Raw `Authorization` header value, if the client sent one.
    #[must_use]
    pub fn authorization(&self) -> Option<String> {
        self.authorization
            .lock()
            .expect("upgrade probe mutex not poisoned")
            .clone()
    }

    fn record(&self, req: &WsUpgradeRequest) {
        let path = req.uri().path().to_owned();
        let authorization = req
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        *self.path.lock().expect("upgrade probe mutex not poisoned") = Some(path);
        *self
            .authorization
            .lock()
            .expect("upgrade probe mutex not poisoned") = authorization;
    }
}

/// HTTP-layer gate applied by [`accept_hdr_async`] before the JSON loop.
#[derive(Clone, Copy)]
enum UpgradeGate {
    /// Live Jetty shape: request-target `/db/` (or `/db`) and a `Basic` header.
    RequireDbAndBasic,
    /// Always 403 — missing Authorization or invalid Basic (wrong password).
    Forbidden,
}

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
    /// authentication-rejection scenario (JSON `success: false` after Upgrade).
    AuthFail(String),

    /// Reject the WebSocket upgrade with HTTP 403. Models Jetty's gate on
    /// missing or invalid `Authorization: Basic` (wrong password) before
    /// any JSON frame. Distinct from [`MockBehavior::AuthFail`], which is
    /// a post-upgrade `Error` response.
    HttpForbidden,

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

    /// Accept connect with success, then on the FIRST [`Request::Ping`] received,
    /// fire the contained oneshot signal and DROP the request without responding.
    /// Subsequent requests are answered normally ([`Response::Pong`] for any
    /// non-Exit request; clean close on [`Request::Exit`]).
    ///
    /// This variant powers the deterministic in-flight cancellation test in
    /// `tests/cancellation.rs`. The test races a `biased` `tokio::select!` between
    /// the signal receiver and the in-flight `job.ping()` future: once the mock
    /// reports it has *seen* the request, the test arm is taken and the ping
    /// future is dropped mid-await, exercising the dispatcher's cancellation
    /// path (`oneshot::Receiver` drops; pending `HashMap` entry reaped on next reply).
    ///
    /// The sender is wrapped in `Arc<Mutex<Option<...>>>` so the variant can
    /// still derive `Clone` (needed by the existing `MockBehavior` shape) and so
    /// `run_mock` can `take()` it on first use. The mock takes the sender once;
    /// all later pings see `None` and respond normally.
    // NOTE: used by Cleanup E (PRO-???) deterministic cancellation integration test.
    SwallowFirstPing {
        /// One-shot signal fired by the mock the moment it has read (and is about
        /// to discard) the first `Ping` request. Wrapped to satisfy `Clone` on
        /// the enclosing enum; only the first take consumes it.
        signal_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    },

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

/// Encode a [`Response`] as the live daemon would.
///
/// `Connected` / `QueryResult` / `Error` omit `"type"`. Other variants use
/// tagged `Serialize` so `Pong` stays `{"type":"pong",...}`. Local to the
/// mock: integration tests cannot see `pub(crate)` helpers, and a public
/// `encode_live` would expand the crate API for test-only JSON.
fn encode_live_response(response: &Response) -> String {
    match response {
        Response::Connected { id, version, job } => {
            let mut v = serde_json::json!({
                "id": id,
                "job": job,
                "success": true,
                "execution_time": 0
            });
            if !version.is_empty() {
                v["version"] = serde_json::json!(version);
            }
            v.to_string()
        }
        Response::QueryResult(q) => serde_json::to_string(q).expect("serialize QueryResult"),
        Response::Error(e) => serde_json::to_string(e).expect("serialize ErrorResponse"),
        other => serde_json::to_string(other).expect("serialize tagged response"),
    }
}

/// Spawn a mock TLS+WebSocket server bound to `127.0.0.1:0`.
///
/// Returns the bound [`SocketAddr`] (so tests can connect to
/// `wss://127.0.0.1:<port>/db/`) and the self-signed cert as DER bytes
/// (so tests using [`mapepire::TlsConfig::Ca`] can pin it).
///
/// The spawned task handles exactly **one** TCP connection, then exits.
/// Spawn a fresh mock per test function.
///
/// Upgrade gating (Jetty-shaped):
/// - request-target other than `/db/` or `/db` → HTTP 404
/// - [`MockBehavior::HttpForbidden`] → HTTP 403 (missing or invalid Basic)
/// - otherwise a `Authorization: Basic …` header is required; missing → 403
///
/// # Panics
///
/// Must be called from within a tokio async context (i.e., inside a
/// `#[tokio::test]` function or similar). Panics if called outside a runtime.
pub fn spawn_mock(behavior: MockBehavior) -> (SocketAddr, Vec<u8>) {
    spawn_mock_with_probe(behavior, UpgradeProbe::default())
}

/// [`spawn_mock`] plus an [`UpgradeProbe`] that records the upgrade path and
/// `Authorization` header (including on 403/404 rejects).
#[allow(dead_code)]
pub fn spawn_mock_with_probe(behavior: MockBehavior, probe: UpgradeProbe) -> (SocketAddr, Vec<u8>) {
    let (acceptor, cert_der) = mint_localhost_tls();
    let (listener, addr) = bind_loopback();

    tokio::spawn(async move {
        let (tcp_stream, _peer) = listener.accept().await.expect("mock accept");
        let tls_stream = acceptor
            .accept(tcp_stream)
            .await
            .expect("mock TLS handshake");
        let gate = match &behavior {
            MockBehavior::HttpForbidden => UpgradeGate::Forbidden,
            _ => UpgradeGate::RequireDbAndBasic,
        };
        // 403/404: the client maps `WsError::Http`; the mock's job is done.
        if let Ok(ws_stream) = accept_hdr_async(tls_stream, upgrade_callback(probe, gate)).await {
            run_mock(ws_stream, behavior).await;
        }
    });

    (addr, cert_der)
}

/// Mint a self-signed cert for `127.0.0.1` and a rustls acceptor.
fn mint_localhost_tls() -> (TlsAcceptor, Vec<u8>) {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()])
            .expect("rcgen self-signed cert");
    let cert_der: Vec<u8> = cert.der().as_ref().to_vec();
    let key_der = signing_key.serialize_der();
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert_der.clone())],
            PrivatePkcs8KeyDer::from(key_der).into(),
        )
        .expect("rustls ServerConfig");
    (TlsAcceptor::from(Arc::new(server_config)), cert_der)
}

/// Bind `127.0.0.1:0` without `block_on` inside an already-running runtime.
fn bind_loopback() -> (TcpListener, SocketAddr) {
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock listener");
    std_listener.set_nonblocking(true).expect("set_nonblocking");
    let addr = std_listener.local_addr().expect("mock local_addr");
    let listener = TcpListener::from_std(std_listener).expect("convert to tokio listener");
    (listener, addr)
}

fn http_error(status: StatusCode, body: &str) -> HttpResponse<Option<String>> {
    let mut res = HttpResponse::new(Some(body.to_string()));
    *res.status_mut() = status;
    res
}

// tungstenite's `Callback` returns `ErrorResponse = http::Response<Option<String>>`
// (~136 bytes). Boxing would not match the trait; allow the large `Err`.
#[allow(clippy::result_large_err)]
fn upgrade_callback(
    probe: UpgradeProbe,
    gate: UpgradeGate,
) -> impl FnOnce(
    &WsUpgradeRequest,
    WsUpgradeResponse,
) -> Result<WsUpgradeResponse, HttpResponse<Option<String>>> {
    move |req, response| {
        probe.record(req);
        let path = req.uri().path();
        if path != "/db/" && path != "/db" {
            return Err(http_error(StatusCode::NOT_FOUND, "not found"));
        }
        let authorization = req
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok());
        match gate {
            UpgradeGate::Forbidden => {
                let body = if authorization.is_none() {
                    "Authorization header missing"
                } else {
                    "invalid credentials"
                };
                Err(http_error(StatusCode::FORBIDDEN, body))
            }
            UpgradeGate::RequireDbAndBasic => match authorization {
                Some(value) if value.starts_with("Basic ") => Ok(response),
                Some(_) => Err(http_error(StatusCode::FORBIDDEN, "invalid credentials")),
                None => Err(http_error(
                    StatusCode::FORBIDDEN,
                    "Authorization header missing",
                )),
            },
        }
    }
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

    // Helper: serialize a Response in the live dialect and send it as a text frame.
    macro_rules! send_response {
        ($resp:expr) => {{
            let json = encode_live_response(&$resp);
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

        MockBehavior::HttpForbidden => {
            panic!("HttpForbidden must reject at HTTP upgrade, not reach the JSON loop");
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

        MockBehavior::SwallowFirstPing { signal_tx } => {
            send_response!(Response::Connected {
                id: connect_id,
                version: MOCK_VERSION.into(),
                job: MOCK_JOB.into(),
            });
            // Take the sender out of the Arc<Mutex<Option<...>>>. After the
            // first ping fires it, every subsequent ping sees None and gets
            // a normal Pong.
            let signal_slot = signal_tx;
            loop {
                match recv_request!() {
                    None => break,
                    Some(Request::Exit { id }) => {
                        send_response!(Response::Exited { id });
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    }
                    Some(Request::Ping { id }) => {
                        let taken = signal_slot
                            .lock()
                            .expect("SwallowFirstPing signal mutex poisoned")
                            .take();
                        match taken {
                            Some(tx) => {
                                // Fire the signal AFTER reading the frame off
                                // the wire. The test's biased select uses this
                                // edge to cancel the in-flight ping future.
                                // Send-failure is benign: it just means the
                                // test already moved on (e.g., dropped the rx).
                                let _ = tx.send(());
                                // Intentionally DO NOT respond; the dispatcher's
                                // pending entry will be reaped when a later
                                // (matched) response arrives or on shutdown.
                            }
                            None => {
                                send_response!(Response::Pong { id });
                            }
                        }
                    }
                    Some(req) => {
                        let id = request_id(&req);
                        send_response!(Response::Pong { id });
                    }
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

/// Map a [`Request`] variant to its wire-tag discriminator string.
///
/// Mirrors the `#[serde(tag = "type", rename_all = "snake_case")]` plus the
/// per-variant `#[serde(rename = "...")]` overrides on the inbound enum so
/// [`MockHandle::observed_request_types`] reports the same string the daemon
/// would see on the wire.
//
// `tests/common` is compiled as part of every integration-test binary;
// binaries that don't use the multi-connection mock will see this helper
// as unused, so suppress dead-code at the function level rather than the
// crate level.
#[allow(dead_code)]
fn request_type(req: &Request) -> &'static str {
    match req {
        Request::Connect { .. } => "connect",
        Request::Sql { .. } => "sql",
        Request::PrepareSql { .. } => "prepare_sql",
        Request::PrepareSqlExecute { .. } => "prepare_sql_execute",
        Request::Execute { .. } => "execute",
        Request::SqlMore { .. } => "sqlmore",
        Request::SqlClose { .. } => "sqlclose",
        Request::Cl { .. } => "cl",
        Request::GetVersion { .. } => "getversion",
        Request::GetDbJob { .. } => "getdbjob",
        Request::SetConfig { .. } => "setconfig",
        Request::GetTraceData { .. } => "gettracedata",
        Request::Dove { .. } => "dove",
        Request::Ping { .. } => "ping",
        Request::Exit { .. } => "exit",
        // The enum is `#[non_exhaustive]`; surface unknown variants as a
        // dedicated tag so test failures point at *adding a variant* rather
        // than crashing on a `_` panic. Tests that assert on specific tags
        // won't see this in practice.
        _ => "other",
    }
}

// `tests/common` is compiled once per integration-test binary; the binaries
// that don't use the multi-connection mock would see the new types and
// methods as unused. The crate-level dead-code suppression on
// [`MockBehavior`] is per-variant and doesn't extend here, so apply
// per-item allows below.

/// Shared per-mock state captured across every accepted connection.
///
/// Used by [`MockHandle`] to expose multi-connection observability: which
/// socket saw which request, in what order, and (when [`MockState::pause`]
/// is `Some`) how long every per-connection task should sleep before
/// emitting its next response.
///
/// Locked under a `std::sync::Mutex` deliberately:
/// - Critical sections are short (push one entry, read one `Option`, etc.).
/// - The lock is **never held across `.await`** (per AGENTS.md / `clippy::await_holding_lock`), so
///   an async-aware `tokio::sync::Mutex` would only buy contention overhead with no scheduling
///   benefit.
/// - `MockHandle` lock taps and per-connection lock taps both stay self-contained: lock, mutate /
///   clone, drop, then await.
#[allow(dead_code)]
pub(crate) struct MockState {
    /// Append-only history of `(socket_id, request)` pairs across every
    /// accepted connection. New entries land via [`MockState::push_request`]
    /// in the per-connection task immediately after a request is read off
    /// the wire and before the canned response is built.
    pub(crate) requests: Vec<(u64, Request)>,
    /// Monotonically incremented for each accepted TCP connection. The
    /// listener task takes a fresh id from this counter and hands it to
    /// the per-connection task, so connection ids are stable for the
    /// lifetime of the mock.
    pub(crate) socket_count: u64,
    /// When `Some(d)`, every per-connection task sleeps for `d` *before*
    /// sending each response (after recording the inbound request, so the
    /// observation hooks remain prompt). Used by [`MockHandle::pause_responses`]
    /// to slow the mock without dropping connections — Task 30's pool
    /// saturation test gates the second `acquire()` on a paused first
    /// response.
    pub(crate) pause: Option<std::time::Duration>,
}

#[allow(dead_code)]
impl MockState {
    /// Append `(socket_id, req)` to the request history. Cheap critical
    /// section — clone happens before the lock is taken so the lock is
    /// held only for the `Vec::push`.
    pub(crate) fn push_request(state: &Arc<Mutex<Self>>, socket_id: u64, req: Request) {
        let mut g = state.lock().expect("mock state mutex not poisoned");
        g.requests.push((socket_id, req));
    }

    /// Read the current pause window, if any. Returns a clone of the
    /// `Option<Duration>` so the caller can `tokio::time::sleep` without
    /// holding the lock across the `.await`.
    pub(crate) fn pause(state: &Arc<Mutex<Self>>) -> Option<std::time::Duration> {
        state.lock().expect("mock state mutex not poisoned").pause
    }
}

/// Multi-connection observation handle returned alongside [`spawn_pool_mock`].
///
/// The handle wraps the same `Arc<Mutex<MockState>>` that the mock's
/// per-connection tasks write to, so `MockHandle::observed_*` methods see
/// requests live as the dispatcher emits them. Pool tests typically:
///
/// 1. `spawn_pool_mock_and_server()` (or `spawn_mock_pool(N)`) → get the `MockHandle`.
/// 2. Drive concurrent acquires on the pool.
/// 3. Assert on `handle.observed_socket_ids()`, `handle.last_socket_for_sql("UPDATE")`, etc.
///
/// All accessors are sync — they take the std mutex, snapshot the data,
/// and release before returning. No `.await` is held across the lock.
/// Tests can call them from anywhere (including outside a tokio context)
/// because the mutex is `std::sync::Mutex`.
///
/// Cheap to clone — it's a single `Arc`. Tests that fan out spawn a clone
/// per task without touching the lock until they need to read.
#[allow(dead_code)]
#[derive(Clone)]
pub struct MockHandle {
    state: Arc<Mutex<MockState>>,
}

#[allow(dead_code)]
impl MockHandle {
    /// All socket ids that have appeared in the request history, sorted &
    /// deduped. Empty vec means no requests landed yet (or no connection
    /// was accepted).
    ///
    /// Note: a socket's id is only minted into the history once that
    /// socket's per-connection task receives at least one request *after*
    /// the initial `Connect`. A connection that opens and immediately
    /// stalls without issuing a follow-up request will not appear here —
    /// use [`MockHandle::open_socket_count`] for "how many TCP accepts".
    pub fn observed_socket_ids(&self) -> Vec<u64> {
        let g = self.state.lock().expect("mock state mutex not poisoned");
        let mut ids: Vec<u64> = g.requests.iter().map(|(sid, _)| *sid).collect();
        drop(g);
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Every SQL text the mock has observed across every connection, in
    /// the order it landed on the wire. Pulls from [`Request::Sql`] and
    /// [`Request::PrepareSqlExecute`] only — `PrepareSql` (no execute) and
    /// `Execute` (no SQL text, just `cont_id`) are excluded.
    pub fn observed_sql(&self) -> Vec<String> {
        let g = self.state.lock().expect("mock state mutex not poisoned");
        g.requests
            .iter()
            .filter_map(|(_, r)| match r {
                Request::Sql { sql, .. } | Request::PrepareSqlExecute { sql, .. } => {
                    Some(sql.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// Every wire-tag string the mock has observed, in order. Mirrors the
    /// `#[serde(tag = "type")]` discriminator that the daemon would see —
    /// useful for asserting that (e.g.) a recycle ping landed before a SQL
    /// statement on a particular socket.
    pub fn observed_request_types(&self) -> Vec<String> {
        let g = self.state.lock().expect("mock state mutex not poisoned");
        g.requests
            .iter()
            .map(|(_, r)| request_type(r).to_owned())
            .collect()
    }

    /// Number of TCP connections the mock has accepted, *whether or not*
    /// any post-connect request landed on them. Reads the listener-task's
    /// counter directly.
    pub fn open_socket_count(&self) -> usize {
        let g = self.state.lock().expect("mock state mutex not poisoned");
        usize::try_from(g.socket_count).unwrap_or(usize::MAX)
    }

    /// Locate the most-recent socket id whose request history contains a
    /// SQL statement matching `needle` (case-insensitive substring search).
    /// Returns `None` if no `Sql` or `PrepareSqlExecute` request matched.
    ///
    /// Task 28's reserved-transaction test asserts that BEGIN, an UPDATE,
    /// and COMMIT all share the same socket — this helper is the lookup
    /// primitive for that assertion.
    pub fn last_socket_for_sql(&self, needle: &str) -> Option<u64> {
        let needle_upper = needle.to_ascii_uppercase();
        let g = self.state.lock().expect("mock state mutex not poisoned");
        g.requests.iter().rev().find_map(|(sid, r)| match r {
            Request::Sql { sql, .. } | Request::PrepareSqlExecute { sql, .. } => {
                if sql.to_ascii_uppercase().contains(&needle_upper) {
                    Some(*sid)
                } else {
                    None
                }
            }
            _ => None,
        })
    }

    /// Slow every subsequent mock response by `dur`. The pause persists
    /// for the lifetime of the returned [`ResponsePauseGuard`]; dropping
    /// the guard restores normal speed.
    ///
    /// Used by Task 30's saturation test: hold the pause across two
    /// `pool.acquire()` calls so the second one races against
    /// `acquire_timeout` while the first connection's recycle ping is
    /// still in flight.
    ///
    /// **Note on semantics.** The pause is read inside each per-connection
    /// task's run-loop *between* recording an inbound request and emitting
    /// its response. Requests still reach the observation hooks promptly;
    /// only the response is delayed. This keeps `observed_*` lookups
    /// deterministic even while the dispatcher is blocked on a pending
    /// reply.
    pub fn pause_responses(&self, dur: std::time::Duration) -> ResponsePauseGuard {
        let state = Arc::clone(&self.state);
        {
            let mut g = state.lock().expect("mock state mutex not poisoned");
            g.pause = Some(dur);
        }
        ResponsePauseGuard { state }
    }

    /// Wait until at least one observed request matches `needle` (case-
    /// insensitive substring match against `Request::Sql { sql, .. }` and
    /// `Request::PrepareSqlExecute { sql, .. }`), or until `timeout` elapses.
    ///
    /// Returns `true` if the SQL arrived within the budget, `false` on
    /// timeout. Polls every 10 ms — coarse enough to be cheap, fine enough
    /// that test wall-clock matches actual arrival within ~10 ms.
    ///
    /// Used by tests waiting on fire-and-forget Drop side effects (e.g.,
    /// `Reserved::rollback_on_drop`'s spawned ROLLBACK).
    pub async fn wait_for_sql(&self, needle: &str, timeout: std::time::Duration) -> bool {
        let needle_upper = needle.to_ascii_uppercase();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.last_socket_for_sql(&needle_upper).is_some() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

/// RAII guard returned by [`MockHandle::pause_responses`]. Dropping it
/// clears the per-mock pause window, restoring normal-speed responses on
/// every per-connection task.
///
/// `#[must_use]` because forgetting to bind the guard means the pause
/// applies for a single statement (drop happens at end of expression),
/// which is almost never the test's intent.
#[allow(dead_code)]
#[must_use = "ResponsePauseGuard restores normal mock speed only when dropped"]
pub struct ResponsePauseGuard {
    state: Arc<Mutex<MockState>>,
}

impl Drop for ResponsePauseGuard {
    fn drop(&mut self) {
        let mut g = self
            .state
            .lock()
            .expect("mock state mutex not poisoned (Drop)");
        g.pause = None;
    }
}

/// Spawn a multi-connection mock TLS+WebSocket server bound to `127.0.0.1:0`.
///
/// Unlike [`spawn_mock`] (single-connection per spawn), the listener task
/// here runs `accept().await` in a loop and dispatches each accepted TCP
/// stream to a fresh per-connection task. All tasks share an
/// `Arc<Mutex<MockState>>` so [`MockHandle`] can observe the request
/// history across every connection.
///
/// Each per-connection task speaks a generic happy-path protocol — accept
/// `Connect`, ack with `Connected`; for every subsequent request, record
/// it in `MockState::requests`, optionally sleep for `MockState::pause`,
/// then emit a canned response (empty `QueryResult` for SQL-shaped
/// requests, `Pong` for `Ping`, `SqlClosed` for `SqlClose`, `Exited` for
/// `Exit`). This is enough for pool integration tests, which care about
/// *which socket* saw a request, not the row data the daemon would have
/// returned.
///
/// Returns the bound [`SocketAddr`], the self-signed cert as DER bytes
/// (for `TlsConfig::Ca` pinning), and the [`MockHandle`] for observation.
///
/// # Panics
///
/// Must be called from within a tokio async context. Panics if called
/// outside a runtime.
#[allow(dead_code)]
pub fn spawn_pool_mock() -> (SocketAddr, Vec<u8>, MockHandle) {
    let (acceptor, cert_der) = mint_localhost_tls();
    let (listener, addr) = bind_loopback();

    let state = Arc::new(Mutex::new(MockState {
        requests: Vec::new(),
        socket_count: 0,
        pause: None,
    }));
    let handle = MockHandle {
        state: Arc::clone(&state),
    };

    // Top-level accept loop. Runs as long as the listener is live; the
    // listener drops when the test runtime shuts down. Each accepted
    // socket is handed to a fresh `tokio::spawn` so the accept loop is
    // never blocked by a stuck per-connection task.
    let acceptor_state = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            // listener closed → runtime shutting down → exit accept loop.
            let Ok((tcp_stream, _peer)) = listener.accept().await else {
                break;
            };
            // Mint a socket id under the lock, then drop the lock before
            // any await. Ids start at 1 so a `0` from a default
            // initialization never collides with a real connection.
            let socket_id = {
                let mut g = acceptor_state
                    .lock()
                    .expect("mock state mutex not poisoned (accept loop)");
                g.socket_count += 1;
                g.socket_count
            };
            let acceptor = acceptor.clone();
            let conn_state = Arc::clone(&acceptor_state);
            tokio::spawn(async move {
                // TLS handshake failure → drop connection silently. The
                // test runtime's listener may shut down mid-handshake on
                // teardown; that's a benign drop, not a test failure.
                let Ok(tls_stream) = acceptor.accept(tcp_stream).await else {
                    return;
                };
                // WS upgrade failure (wrong path, missing Basic, teardown) → drop.
                let Ok(ws_stream) = accept_hdr_async(
                    tls_stream,
                    upgrade_callback(UpgradeProbe::default(), UpgradeGate::RequireDbAndBasic),
                )
                .await
                else {
                    return;
                };
                run_pool_connection(ws_stream, socket_id, conn_state).await;
            });
        }
    });

    (addr, cert_der, handle)
}

/// Drive one connection's request/response loop for the multi-connection
/// mock. Counterpart to [`run_mock`] but with shared-state observation
/// instead of a behavior enum.
///
/// Behavior:
/// - Wait for `Connect`, respond with `Connected` (so the dispatcher's handshake completes).
/// - For every subsequent request: record it in `state.requests`, read the current pause window,
///   sleep that long if any, then emit a canned response keyed off the variant.
/// - `Exit` triggers a clean close.
async fn run_pool_connection<S>(
    ws_stream: tokio_tungstenite::WebSocketStream<S>,
    socket_id: u64,
    state: Arc<Mutex<MockState>>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws_stream.split();

    macro_rules! send_response {
        ($resp:expr) => {{
            let json = encode_live_response(&$resp);
            // If a response can't be sent (peer dropped, etc.), the test
            // harness has already lost interest — exit cleanly rather than
            // panicking, which would surface as a noisy task-panic in
            // unrelated tests.
            if sink.send(Message::Text(json.into())).await.is_err() {
                return;
            }
        }};
    }

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
                        if sink.send(Message::Pong(data)).await.is_err() {
                            break None;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break None,
                    Some(Ok(_)) => continue,    // binary, pong, frame
                    Some(Err(_)) => break None, // peer reset / decode error
                }
            }
        }};
    }

    // Step 1: handshake — `Connect` → `Connected`.
    let connect_id = match recv_request!() {
        Some(Request::Connect { id, .. }) => id,
        // Surface non-`Connect` first frames as a hard test failure: pool
        // integration tests assume the dispatcher always opens with
        // `Connect`. A misordered open frame would silently mask a real
        // bug otherwise.
        other => panic!("pool mock expected Connect, got {other:?}"),
    };
    send_response!(Response::Connected {
        id: connect_id,
        version: MOCK_VERSION.into(),
        job: MOCK_JOB.into(),
    });

    // Step 2: request loop.
    loop {
        match recv_request!() {
            None => break,
            Some(req) => {
                // Record before responding so the test can observe the
                // request even if the response is paused or the connection
                // is about to drop.
                MockState::push_request(&state, socket_id, req.clone());

                // Honor the current pause window. Read & release the lock
                // before sleeping — never hold a std::sync::Mutex across
                // an await.
                if let Some(dur) = MockState::pause(&state) {
                    tokio::time::sleep(dur).await;
                }

                match req {
                    Request::Exit { id } => {
                        send_response!(Response::Exited { id });
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    }
                    Request::Ping { id } => {
                        send_response!(Response::Pong { id });
                    }
                    Request::Sql { id, .. }
                    | Request::PrepareSqlExecute { id, .. }
                    | Request::Execute { id, .. }
                    | Request::SqlMore { id, .. } => {
                        // Generic empty success — no rows, terminal page.
                        // Pool tests don't depend on row content; tests
                        // that need specific rows use the v0.2 `Pages`
                        // mock instead.
                        send_response!(Response::QueryResult(canned_empty_query_result(id)));
                    }
                    Request::PrepareSql { id, .. } => {
                        // Synthetic prepared-statement handle — distinct
                        // per socket so test assertions on `cont_id`
                        // don't collide across connections.
                        send_response!(Response::PreparedStatement {
                            id,
                            success: true,
                            cont_id: format!("pool-mock-stmt-{socket_id}"),
                            execution_time: 0.0,
                        });
                    }
                    Request::SqlClose { id, .. } => {
                        send_response!(Response::SqlClosed { id, success: true });
                    }
                    other => {
                        // Catch-all for non-SQL request shapes (Cl,
                        // GetVersion, GetDbJob, SetConfig, GetTraceData,
                        // Dove, etc.). Pong is a safe echo — the
                        // dispatcher routes by `id`, not by response
                        // variant, so this completes the round-trip.
                        let id = request_id(&other);
                        send_response!(Response::Pong { id });
                    }
                }
            }
        }
    }
}

/// Build an empty terminal `QueryResult` stamped with the given request
/// id. Helper for the multi-connection mock — keeps the per-variant
/// match arms readable.
fn canned_empty_query_result(id: String) -> QueryResult {
    use mapepire::protocol::{Column, QueryMetaData};
    QueryResult {
        id,
        success: true,
        has_results: false,
        update_count: 0,
        cont_id: None,
        is_done: true,
        metadata: QueryMetaData {
            column_count: 0,
            columns: Vec::<Column>::new(),
            job: None,
        },
        data: Vec::new(),
        execution_time: 0.0,
    }
}
