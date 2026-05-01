//! WebSocket framing on top of the TLS layer.
//!
//! Wraps `tokio_tungstenite::WebSocketStream` so the dispatcher sees a
//! `Sink<Bytes, ...>` + `Stream<Item = Result<Bytes, ...>>`.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures::{Sink, Stream};
use pin_project_lite::pin_project;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::error::TransportError;
use crate::transport::tls::TlsStream;

pin_project! {
    /// Adapter that exposes a `tokio_tungstenite::WebSocketStream` as a
    /// `Sink<Bytes>` + `Stream<Item = Result<Bytes>>` so the dispatcher
    /// stays oblivious to WebSocket framing details.
    pub(crate) struct WsTransport {
        #[pin]
        inner: WebSocketStream<TlsStream>,
    }
}

impl WsTransport {
    /// Wrap an established `WebSocketStream`. Called by the handshake
    /// helper (Task 6) once the TLS upgrade completes.
    pub(crate) fn new(inner: WebSocketStream<TlsStream>) -> Self {
        Self { inner }
    }
}

impl Sink<Bytes> for WsTransport {
    type Error = TransportError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_ready(cx).map_err(map_ws_err)
    }

    fn start_send(self: Pin<&mut Self>, item: Bytes) -> Result<(), Self::Error> {
        // Use TryFrom<Bytes> for Utf8Bytes (0.27) to validate UTF-8 and avoid
        // the Vec allocation that String::from_utf8(item.to_vec()) would incur.
        use tokio_tungstenite::tungstenite::protocol::frame::Utf8Bytes;
        let utf8 = Utf8Bytes::try_from(item).map_err(|_| {
            TransportError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "non-UTF-8 bytes handed to WsTransport (caller contract violation)",
            ))
        })?;
        self.project()
            .inner
            .start_send(Message::Text(utf8))
            .map_err(map_ws_err)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_flush(cx).map_err(map_ws_err)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.project().inner.poll_close(cx).map_err(map_ws_err)
    }
}

impl Stream for WsTransport {
    type Item = Result<Bytes, TransportError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let frame = match self.as_mut().project().inner.poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Ready(Some(Ok(msg))) => msg,
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(map_ws_err(e)))),
            };
            match frame {
                // Utf8Bytes implements From<Utf8Bytes> for Bytes (0.27) — zero-copy.
                Message::Text(s) => return Poll::Ready(Some(Ok(Bytes::from(s)))),
                // Binary payload is already Bytes in 0.27 — return directly.
                Message::Binary(b) => return Poll::Ready(Some(Ok(b))),
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {
                    // Tungstenite handles ping/pong automatically; nothing for the
                    // application layer to do. Loop and read the next frame.
                }
                Message::Close(_) => return Poll::Ready(None),
            }
        }
    }
}

fn map_ws_err(e: tokio_tungstenite::tungstenite::Error) -> TransportError {
    use tokio_tungstenite::tungstenite::Error as T;
    match e {
        T::ConnectionClosed | T::AlreadyClosed => TransportError::Closed,
        T::Io(io) => TransportError::Io(io),
        other => TransportError::Io(std::io::Error::other(other.to_string())),
    }
}
