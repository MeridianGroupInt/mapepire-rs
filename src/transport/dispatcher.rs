//! Per-`Job` dispatcher task.
//!
//! Owns the WebSocket transport for one connection. Callers enqueue
//! requests via `DispatcherHandle::send`, which returns a
//! `oneshot::Receiver<Response>` for the matching reply. The dispatcher
//! runs an event loop that:
//!
//! 1. Pulls the next outgoing request from the send-queue mpsc.
//! 2. Records `pending[id] = oneshot::Sender`.
//! 3. Writes the request to the socket.
//! 4. Reads the next inbound frame.
//! 5. Parses it as `Response`, looks up the id in `pending`, and routes the response through the
//!    matching oneshot.
//! 6. On socket close or local shutdown, drains every pending entry with `Error::Transport(Closed)`
//!    so no caller hangs.

use std::collections::HashMap;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::error::{Error, ProtocolError, TransportError};
use crate::protocol::{Request, Response};
use crate::transport::BoxedTransport;

/// Bounded outbound queue capacity. 64 slots is enough to absorb burst
/// concurrency without unbounded memory growth on a runaway sender.
/// Revisit if real workloads expose contention.
const SEND_QUEUE_CAPACITY: usize = 64;

/// Outbound message: the serialized request bytes + a slot to deliver
/// the response into.
struct Outbound {
    /// Caller-supplied correlation id used to route the response back.
    id: String,
    /// Pre-serialized request bytes ready to write to the socket.
    bytes: Bytes,
    /// One-shot channel the dispatcher uses to deliver the response (or
    /// a transport/protocol error) back to the caller.
    reply: oneshot::Sender<Result<Response, Error>>,
}

/// Caller-facing handle for issuing requests through the dispatcher.
#[derive(Clone, Debug)]
pub(crate) struct DispatcherHandle {
    tx: mpsc::Sender<Outbound>,
}

impl DispatcherHandle {
    /// Send a `Request`. Returns a future that resolves once the matching
    /// `Response` arrives. Dropping the future is cancellation-safe: the
    /// pending entry is removed when the receiver drops; any response
    /// that arrives after is silently discarded.
    pub(crate) async fn send(&self, request: Request) -> crate::Result<Response> {
        let id = request_id(&request).to_string();
        let bytes = serde_json::to_vec(&request)
            .map(Bytes::from)
            .map_err(|e| Error::from(ProtocolError::Json(e)))?;
        let (reply_tx, reply_rx) = oneshot::channel();

        self.tx
            .send(Outbound {
                id,
                bytes,
                reply: reply_tx,
            })
            .await
            .map_err(|_| Error::from(TransportError::Closed))?;

        reply_rx
            .await
            .map_err(|_| Error::from(TransportError::Closed))?
    }
}

/// Dispatcher task handle (for joining on shutdown). `Job` keeps it for
/// the lifetime of the connection; dropping it aborts the dispatcher.
pub(crate) struct Dispatcher {
    handle: DispatcherHandle,
    join: JoinHandle<()>,
}

impl Dispatcher {
    /// Returns a fresh `DispatcherHandle` cloned from the internal one.
    /// Cheap — `mpsc::Sender` is `Arc`-backed.
    pub(crate) fn handle(&self) -> DispatcherHandle {
        self.handle.clone()
    }

    /// Spawn the dispatcher task on the current Tokio runtime.
    pub(crate) fn spawn(transport: BoxedTransport) -> Self {
        // Bounded queue; back-pressure protects against runaway senders.
        let (tx, rx) = mpsc::channel::<Outbound>(SEND_QUEUE_CAPACITY);
        let handle = DispatcherHandle { tx };
        let join = tokio::spawn(run(transport, rx));
        Self { handle, join }
    }
}

impl Drop for Dispatcher {
    fn drop(&mut self) {
        // Aborts the spawned task; pending entries inside the loop drop
        // their oneshot::Sender, which causes any awaiting caller to
        // receive Err(TransportError::Closed) via send().
        self.join.abort();
    }
}

/// Pull the `id` field from any `Request` variant. Centralized here so
/// the dispatcher doesn't need to match every variant.
//
// `clippy::match_same_arms` would have us collapse every arm into a
// single OR-pattern. Keep them separate: one arm per variant is a
// compile-time guard against forgetting to extend `request_id` when a
// new `Request` variant lands, and several variants will likely grow
// per-variant id resolution (e.g., a future variant pulling id from a
// nested struct field) which a merged arm would have to be re-split
// for anyway.
#[allow(clippy::match_same_arms)]
fn request_id(request: &Request) -> &str {
    match request {
        Request::Connect { id, .. } => id,
        Request::Sql { id, .. } => id,
        Request::PrepareSql { id, .. } => id,
        Request::PrepareSqlExecute { id, .. } => id,
        Request::Execute { id, .. } => id,
        Request::SqlMore { id, .. } => id,
        Request::SqlClose { id, .. } => id,
        Request::Cl { id, .. } => id,
        Request::GetVersion { id } => id,
        Request::GetDbJob { id } => id,
        Request::SetConfig { id, .. } => id,
        Request::GetTraceData { id } => id,
        Request::Dove { id, .. } => id,
        Request::Ping { id } => id,
        Request::Exit { id } => id,
    }
}

/// Pull the `id` from a `Response` for routing back to the matching
/// `pending` entry. Every `Response` variant in the v0.1-pinned wire
/// protocol carries an id, so this is infallible — confirmed against
/// `src/protocol/response.rs` (each `Response::Foo { id, .. }` and
/// `QueryResult::id` / `ErrorResponse::id`). The plan text returned an
/// `Option` defensively; clippy's `unnecessary_wraps` flagged it, and
/// the v0.1 protocol shape supports tightening to `&str`.
//
// `clippy::match_same_arms`: same rationale as `request_id` — keep one
// arm per variant as a compile-time exhaustiveness reminder.
#[allow(clippy::match_same_arms)]
fn response_id(response: &Response) -> &str {
    match response {
        Response::Connected { id, .. } => id,
        Response::Pong { id } => id,
        Response::Exited { id } => id,
        Response::QueryResult(q) => &q.id,
        Response::PreparedStatement { id, .. } => id,
        Response::SqlClosed { id, .. } => id,
        Response::ClResult { id, .. } => id,
        Response::Version { id, .. } => id,
        Response::DbJob { id, .. } => id,
        Response::ConfigSet { id, .. } => id,
        Response::TraceData { id, .. } => id,
        Response::DoveResult { id, .. } => id,
        Response::Error(e) => &e.id,
    }
}

async fn run(mut transport: BoxedTransport, mut rx: mpsc::Receiver<Outbound>) {
    let mut pending: HashMap<String, oneshot::Sender<Result<Response, Error>>> = HashMap::new();

    loop {
        tokio::select! {
            // New outgoing request from a caller.
            outbound = rx.recv() => {
                match outbound {
                    Some(Outbound { id, bytes, reply }) => {
                        // Body of the resolved arm — runs to completion outside select!'s
                        // polling set; not subject to mid-flush cancellation.
                        if let Err(e) = transport.send(bytes).await {
                            let _ = reply.send(Err(Error::from(e)));
                            // Socket dead — drain everything pending and exit.
                            drain_pending(&mut pending, TransportError::Closed);
                            return;
                        }
                        // Entries for cancelled futures (caller dropped reply_rx) remain
                        // here until the matching response arrives and is silently discarded
                        // or shutdown drains. Acceptable for v0.2; revisit if leak grows.
                        pending.insert(id, reply);
                    }
                    None => {
                        // All handles dropped; exit cleanly.
                        return;
                    }
                }
            }

            // Inbound frame from the daemon.
            frame = transport.next() => {
                match frame {
                    Some(Ok(bytes)) => match serde_json::from_slice::<Response>(&bytes) {
                        Ok(response) => {
                            let id = response_id(&response).to_owned();
                            if let Some(reply) = pending.remove(&id) {
                                let _ = reply.send(Ok(response));
                            }
                            // No pending match → caller dropped the
                            // future before the response arrived; discard
                            // the response silently.
                        }
                        Err(e) => {
                            // Malformed JSON from the server — fatal for
                            // this dispatcher; drain and exit.
                            drain_pending_with_error(
                                &mut pending,
                                Error::from(ProtocolError::Json(e)),
                            );
                            return;
                        }
                    },
                    Some(Err(e)) => {
                        drain_pending_with_error(&mut pending, Error::from(e));
                        return;
                    }
                    None => {
                        // Peer closed cleanly.
                        drain_pending(&mut pending, TransportError::Closed);
                        return;
                    }
                }
            }
        }
    }
}

fn drain_pending(
    pending: &mut HashMap<String, oneshot::Sender<Result<Response, Error>>>,
    closed: TransportError,
) {
    let mut iter = pending.drain();
    if let Some((_id, reply)) = iter.next() {
        let _ = reply.send(Err(Error::from(closed)));
    }
    for (_id, reply) in iter {
        let _ = reply.send(Err(Error::from(TransportError::Closed)));
    }
}

fn drain_pending_with_error(
    pending: &mut HashMap<String, oneshot::Sender<Result<Response, Error>>>,
    err: Error,
) {
    let mut iter = pending.drain();
    if let Some((_id, reply)) = iter.next() {
        let _ = reply.send(Err(err));
    }
    for (_id, reply) in iter {
        let _ = reply.send(Err(Error::from(TransportError::Closed)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ClMessage, ErrorResponse, QueryMetaData, QueryResult};

    /// Build a fresh `Request` of every variant and check `request_id`
    /// returns the carried `id`. If a new variant is added without
    /// updating `request_id`, the match will fail to compile and this
    /// test won't even build — which is the intended canary.
    #[test]
    fn test_request_id_returns_carried_id_for_every_variant() {
        let id = "test-id".to_string();
        let cases: &[Request] = &[
            Request::Connect {
                id: id.clone(),
                user: "u".into(),
                password: "p".into(),
            },
            Request::Sql {
                id: id.clone(),
                sql: "SELECT 1".into(),
                rows: None,
                parameters: None,
            },
            Request::PrepareSql {
                id: id.clone(),
                sql: "SELECT 1".into(),
            },
            Request::PrepareSqlExecute {
                id: id.clone(),
                sql: "SELECT 1".into(),
                parameters: None,
                rows: None,
            },
            Request::Execute {
                id: id.clone(),
                cont_id: "c".into(),
                parameters: None,
            },
            Request::SqlMore {
                id: id.clone(),
                cont_id: "c".into(),
                rows: 10,
            },
            Request::SqlClose {
                id: id.clone(),
                cont_id: "c".into(),
            },
            Request::Cl {
                id: id.clone(),
                cmd: "DSPLIB".into(),
            },
            Request::GetVersion { id: id.clone() },
            Request::GetDbJob { id: id.clone() },
            Request::SetConfig {
                id: id.clone(),
                tracedest: "FILE".into(),
                tracelevel: "ERRORS".into(),
            },
            Request::GetTraceData { id: id.clone() },
            Request::Dove {
                id: id.clone(),
                sql: "SELECT 1".into(),
            },
            Request::Ping { id: id.clone() },
            Request::Exit { id: id.clone() },
        ];
        for req in cases {
            assert_eq!(request_id(req), id.as_str());
        }
    }

    /// Build a fresh `Response` of every variant and check `response_id`
    /// returns the carried `id`. Same compile-time canary intent.
    #[test]
    fn test_response_id_returns_carried_id_for_every_variant() {
        let id = "test-id".to_string();
        let qr = QueryResult {
            id: id.clone(),
            success: true,
            has_results: false,
            update_count: -1,
            cont_id: None,
            is_done: true,
            metadata: QueryMetaData {
                column_count: 0,
                columns: vec![],
            },
            data: vec![],
            execution_time: 0.0,
        };
        let err = ErrorResponse {
            id: id.clone(),
            success: false,
            sqlstate: None,
            sqlcode: None,
            error: None,
            job: None,
        };
        let cases: &[Response] = &[
            Response::Connected {
                id: id.clone(),
                version: "1".into(),
                job: "J".into(),
            },
            Response::Pong { id: id.clone() },
            Response::Exited { id: id.clone() },
            Response::QueryResult(qr),
            Response::PreparedStatement {
                id: id.clone(),
                success: true,
                cont_id: "c".into(),
                execution_time: 0.0,
            },
            Response::SqlClosed {
                id: id.clone(),
                success: true,
            },
            Response::ClResult {
                id: id.clone(),
                success: true,
                messages: vec![ClMessage {
                    id: None,
                    kind: None,
                    text: None,
                }],
            },
            Response::Version {
                id: id.clone(),
                success: true,
                version: "1".into(),
            },
            Response::DbJob {
                id: id.clone(),
                success: true,
                job: "J".into(),
            },
            Response::ConfigSet {
                id: id.clone(),
                success: true,
            },
            Response::TraceData {
                id: id.clone(),
                success: true,
                tracedata: String::new(),
            },
            Response::DoveResult {
                id: id.clone(),
                success: true,
                result: serde_json::json!({}),
            },
            Response::Error(err),
        ];
        for resp in cases {
            assert_eq!(response_id(resp), id.as_str());
        }
    }
}
