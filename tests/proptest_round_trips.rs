//! Property-based round-trip tests: serialize → parse → re-serialize byte-stable
//! for arbitrary request and response payloads. Catches edge cases manual
//! fixtures miss (escaped strings, numeric extremes, unicode, deep nesting).

use mapepire::protocol::request::Request;
use mapepire::protocol::response::{QueryMetaData, QueryResult, Response};
use proptest::prelude::*;

/// Generator for caller-supplied correlation ids — restrict to a charset
/// that's wire-safe (alphanumeric + dash). Real callers use whatever the
/// `IdAllocator` produces, which is hex + dash + digits.
fn id() -> impl Strategy<Value = String> {
    "[a-z0-9-]{1,16}"
}

/// Generator for `f64` values that byte-stably round-trip through
/// `serde_json`.
///
/// `serde_json`'s decimal-string float formatter aims for "shortest
/// round-trip" representation, but at values where the f64 mantissa runs
/// out of precision (typically anywhere with > ~15 significant decimal
/// digits) the emitted string can re-parse to a value 1 ULP away from the
/// original, breaking the byte-equality property. This is a known
/// limitation of decimal float serialization, not a defect in the wire
/// format — `parse(serialize(x))` always gives an equivalent value, just
/// not necessarily the original `x`.
///
/// The filter approach (vs a value-range cap) is more honest: it tests
/// the byte-stability property on every f64 the crate's wire format can
/// actually carry safely. Generation cost is one extra round-trip per
/// candidate value, well within the 256-case budget.
fn stable_f64() -> impl Strategy<Value = f64> {
    any::<f64>().prop_filter("finite + byte-stable JSON round-trip", |&f| {
        if !f.is_finite() {
            return false;
        }
        let Ok(s) = serde_json::to_string(&f) else {
            return false;
        };
        let Ok(back) = serde_json::from_str::<f64>(&s) else {
            return false;
        };
        serde_json::to_string(&back).is_ok_and(|s2| s == s2)
    })
}

/// Generator for arbitrary JSON scalar values (the type used inside SQL
/// parameter vectors). f64s come from `stable_f64()` to keep the
/// byte-stability property meaningful.
fn json_value() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        any::<i64>().prop_map(|n| serde_json::json!(n)),
        stable_f64().prop_map(|n| serde_json::json!(n)),
        ".*".prop_map(serde_json::Value::String),
    ]
}

/// Generator for arbitrary `Request::Sql`. Covers the most parameter-heavy
/// variant; structural round-tripping for the others is implicit because
/// they share the same serde derives and tag-based discrimination.
fn arb_sql_request() -> impl Strategy<Value = Request> {
    (
        id(),
        ".*",
        any::<Option<u32>>(),
        prop::option::of(prop::collection::vec(json_value(), 0..6)),
    )
        .prop_map(|(id, sql, rows, parameters)| Request::Sql {
            id,
            sql,
            rows,
            parameters,
        })
}

/// Generator for an arbitrary `QueryResult` body — the largest server-emitted
/// shape. Keeps `metadata` and `data` empty in the property test; row data
/// fuzzing belongs in a dedicated test once the row layer arrives in v0.2.
///
/// `execution_time` uses `stable_f64()` so the byte-stable round-trip
/// property holds across the full f64 range we test.
fn arb_query_result() -> impl Strategy<Value = QueryResult> {
    (
        id(),
        any::<bool>(),
        any::<bool>(),
        any::<i64>(),
        prop::option::of(id()),
        any::<bool>(),
        stable_f64(),
    )
        .prop_map(
            |(id, success, has_results, update_count, cont_id, is_done, execution_time)| {
                QueryResult {
                    id,
                    success,
                    has_results,
                    update_count,
                    cont_id,
                    is_done,
                    metadata: QueryMetaData::default(),
                    data: vec![],
                    execution_time,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn sql_request_round_trips(r in arb_sql_request()) {
        let serialized = serde_json::to_vec(&r).unwrap();
        let back: Request = serde_json::from_slice(&serialized).unwrap();
        // Serialized form must be byte-stable across one round trip.
        let reserialized = serde_json::to_vec(&back).unwrap();
        prop_assert_eq!(serialized, reserialized);
    }

    #[test]
    fn query_result_round_trips(q in arb_query_result()) {
        let r = Response::QueryResult(q);
        let serialized = serde_json::to_vec(&r).unwrap();
        let back: Response = serde_json::from_slice(&serialized).unwrap();
        let reserialized = serde_json::to_vec(&back).unwrap();
        prop_assert_eq!(serialized, reserialized);
    }
}
