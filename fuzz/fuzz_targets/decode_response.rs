//! Fuzz `serde_json::from_slice::<Response>` with arbitrary bytes.
//!
//! Coverage goal: any byte sequence either parses successfully into a
//! [`mapepire::Response`] OR returns a typed [`serde_json::Error`].
//! No panic, no UB, no infinite loop.
//!
//! Run locally: `cargo +nightly fuzz run decode_response -- -runs=1000`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use mapepire::Response;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<Response>(data);
});
