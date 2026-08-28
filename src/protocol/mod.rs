//! Mapepire wire protocol.
//!
//! The protocol is JSON over `WebSockets`. Each request carries a caller-supplied
//! `id` (string), and each response echoes the same `id` so they can be
//! correlated. Multiple requests may be in flight on one socket.
//!
//! Variants are added in subsequent protocol tasks; this file lays the
//! discriminated-union scaffolding (`Request`, `Response`, `IdAllocator`).

pub(crate) mod codec;
mod decode;
pub(crate) mod request;
pub(crate) mod response;

pub use crate::protocol::codec::{IdAllocator, RequestId};
pub use crate::protocol::request::Request;
pub use crate::protocol::response::{
    ClMessage, Column, ErrorResponse, JobLogEntry, ParameterDetail, ParameterResult, QueryMetaData,
    QueryResult, Response,
};
