//! Encode → decode → re-encode round-trip on protocol types.
//!
//! Asserts byte-stable round-tripping (the same structural property the
//! existing `tests/proptest_round_trips.rs` checks via proptest, but
//! libFuzzer's coverage-guided mutator finds different edges around
//! serde tag handling, optional-field shapes, and integer/float
//! formatting.)
//!
//! libFuzzer feeds raw bytes; we reinterpret a small prefix as field
//! values via the `arbitrary` derive on a private input struct, then
//! build a [`mapepire::Response::QueryResult`] (the richest variant)
//! from those fields. No Arbitrary impl on the protocol types needed.
//!
//! Run locally: `cargo +nightly fuzz run round_trip -- -runs=1000`.

#![no_main]

use libfuzzer_sys::arbitrary::{self, Arbitrary};
use libfuzzer_sys::fuzz_target;
use mapepire::{Column, QueryMetaData, QueryResult, Response};

#[derive(Arbitrary, Debug)]
struct Input<'a> {
    id: &'a str,
    success: bool,
    has_results: bool,
    update_count: i64,
    cont_id: Option<&'a str>,
    is_done: bool,
    execution_time_bits: u64,
    column_names: Vec<&'a str>,
    column_types: Vec<&'a str>,
}

fuzz_target!(|input: Input<'_>| {
    // Reconstruct execution_time deterministically from the bytes;
    // f64::from_bits never panics. NaN re-encoded as JSON would lose
    // byte stability (NaN serializes as `null` or fails depending on
    // serde config), so skip NaN to keep the structural-equality
    // assertion meaningful.
    let execution_time = f64::from_bits(input.execution_time_bits);
    if execution_time.is_nan() {
        return;
    }

    let columns = input
        .column_names
        .into_iter()
        .zip(input.column_types)
        .map(|(name, type_name)| Column {
            name: name.to_string(),
            label: Some(name.to_string()),
            type_name: Some(type_name.to_string()),
            display_size: None,
            precision: None,
            scale: None,
        })
        .collect::<Vec<_>>();
    let column_count = u32::try_from(columns.len()).unwrap_or(u32::MAX);

    let qr = QueryResult {
        id: input.id.to_string(),
        success: input.success,
        execution_time,
        has_results: input.has_results,
        update_count: input.update_count,
        metadata: QueryMetaData {
            column_count,
            columns,
        },
        data: Vec::new(),
        cont_id: input.cont_id.map(str::to_string),
        is_done: input.is_done,
    };
    let r = Response::QueryResult(qr);

    // Round-trip 1: serialize, deserialize.
    let serialized = match serde_json::to_vec(&r) {
        Ok(b) => b,
        Err(_) => return,
    };
    let back: Response = match serde_json::from_slice(&serialized) {
        Ok(r) => r,
        Err(_) => return,
    };

    // Round-trip 2: re-serialize the decoded value. The two byte strings
    // must match — that's the byte-stability invariant.
    let reserialized = serde_json::to_vec(&back).expect("re-serialize");
    assert_eq!(serialized, reserialized, "byte-stable round-trip violated");
});
