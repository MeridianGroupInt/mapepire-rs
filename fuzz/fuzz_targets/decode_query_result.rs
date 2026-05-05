//! Fuzz `serde_json::from_slice::<QueryResult>` with arbitrary bytes.
//!
//! `QueryResult` is the richest variant in the protocol — column metadata,
//! data rows (each a `serde_json::Value` map), continuation ids for paging.
//! The decode path here is the most likely place for malformed daemon
//! responses to land deeply-nested or surprise-shaped data.
//!
//! Run locally: `cargo +nightly fuzz run decode_query_result -- -runs=1000`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use mapepire::QueryResult;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<QueryResult>(data);
});
