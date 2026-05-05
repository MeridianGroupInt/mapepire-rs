//! Fuzz two related code paths:
//!
//! 1. `serde_json::from_slice::<ErrorResponse>(bytes)` — the wire decode must never panic on
//!    arbitrary bytes; either parses or returns a typed error.
//! 2. The [`mapepire::ServerError`] SQLSTATE-classification predicates (`is_transient`,
//!    `is_constraint_violation`, `is_authorization`, `is_object_not_found`,
//!    `is_data_type_mismatch`) — none may panic on any string a server might emit.
//!
//! The two halves are exercised independently rather than chained
//! because `ServerError` is intentionally non-`Deserialize` (it's a
//! crate-internal type the dispatcher constructs from `ErrorResponse`),
//! so we feed the predicates a directly-fuzzed `sqlstate` string.
//!
//! Run locally: `cargo +nightly fuzz run decode_error_response -- -runs=1000`.

#![no_main]

use libfuzzer_sys::arbitrary::{self, Arbitrary};
use libfuzzer_sys::fuzz_target;
use mapepire::{ErrorResponse, ServerError};

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    /// Bytes for the JSON decode half.
    decode_bytes: &'a [u8],
    /// String fed straight into `ServerError.sqlstate` for the predicate
    /// half. `Option<String>` so we exercise both `Some(...)` and `None`.
    sqlstate: Option<&'a str>,
}

fuzz_target!(|input: Input<'_>| {
    // Half 1: decode never panics on arbitrary bytes.
    let _ = serde_json::from_slice::<ErrorResponse>(input.decode_bytes);

    // Half 2: every SQLSTATE predicate terminates without panic on any
    // string the server might send. Build a minimal ServerError directly
    // — the predicates only inspect `sqlstate`.
    let err = ServerError {
        message: String::new(),
        sqlstate: input.sqlstate.map(str::to_string),
        sqlcode: None,
        job_name: None,
        diagnostics: Vec::new(),
    };

    let _ = err.is_transient();
    let _ = err.is_constraint_violation();
    let _ = err.is_authorization();
    let _ = err.is_object_not_found();
    let _ = err.is_data_type_mismatch();
});
