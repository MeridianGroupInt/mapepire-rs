//! Mapepire wire protocol.
//!
//! The protocol is JSON over `WebSockets`. Each request carries a caller-supplied
//! `id` (string), and each response echoes the same `id` so they can be
//! correlated. Multiple requests may be in flight on one socket.
//!
//! Variants are filled in across Tasks 9–14; this file lays the discriminated-
//! union scaffolding (`Request`, `Response`, `IdAllocator`).

pub mod codec;
pub mod request;
pub mod response;

pub use crate::protocol::codec::{IdAllocator, RequestId};
pub use crate::protocol::request::Request;
pub use crate::protocol::response::Response;
