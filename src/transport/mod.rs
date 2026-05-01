//! Transport layer — TLS + WebSocket + dispatcher infrastructure.
//!
//! This module is `pub(crate)` rather than `pub` because the abstractions
//! here are implementation details of `Job`. Consumers interact with the
//! transport indirectly via the public `Job` / `Query` / `Rows` APIs.
//!
//! See `docs/superpowers/specs/2026-04-29-mapepire-rust-design.md` §6 for
//! the lifecycle and dispatcher diagrams.

pub(crate) mod dispatcher;
pub(crate) mod socket;
pub(crate) mod tls;

use std::pin::Pin;

use bytes::Bytes;
use futures::{Sink, Stream};

/// Minimal transport abstraction the dispatcher reads/writes against.
/// Real implementations: `tokio_tungstenite::WebSocketStream<TlsStream>`.
/// Test mock: an in-memory channel pair.
///
/// The `Transport` trait keeps the dispatcher decoupled from the concrete
/// WebSocket library so we can substitute a mock in unit tests without
/// spinning a real TLS server.
#[allow(dead_code)]
pub(crate) trait Transport:
    Sink<Bytes, Error = crate::error::TransportError>
    + Stream<Item = Result<Bytes, crate::error::TransportError>>
    + Send
    + Unpin
{
}

impl<T> Transport for T where
    T: Sink<Bytes, Error = crate::error::TransportError>
        + Stream<Item = Result<Bytes, crate::error::TransportError>>
        + Send
        + Unpin
{
}

/// Type alias for a boxed dynamic transport — used by `Dispatcher::spawn`.
#[allow(dead_code)]
pub(crate) type BoxedTransport = Pin<Box<dyn Transport>>;

// Re-exports for callers in Task 6 (handshake) and Task 8 (`Job`). The
// `unused_imports` allow keeps clippy/rustc happy until Task 6 wires
// `Dispatcher::spawn` into a code path that's actually exercised; the
// dispatcher chain otherwise remains dead through the end of Task 5.
#[allow(unused_imports)]
pub(crate) use dispatcher::{Dispatcher, DispatcherHandle};
