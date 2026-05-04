//! `FromRow` trait — typed materialization for one row.
//!
//! Default blanket impl for `T: DeserializeOwned` materializes via
//! `serde_json::from_value` against the row's column map. Power users can
//! hand-implement when the default name-matching isn't enough (e.g.,
//! mapping `EMPNO` (Db2 column) → `employee_id` (Rust field)).

use serde::de::DeserializeOwned;

use crate::error::{DecodeError, Error};
use crate::query::Row;

/// Convert one row into a typed value.
///
/// The blanket impl below covers any `T: serde::de::DeserializeOwned`,
/// which is the common path. Hand-implement when the default
/// column-name / field-name match isn't right (e.g. Db2 returns
/// `EMPNO` and your struct field is `employee_id`).
pub trait FromRow: Sized {
    /// Construct `Self` from a row.
    ///
    /// # Errors
    ///
    /// Implementor-defined — typically [`Error::Decode`] when a column
    /// is missing or a value can't be decoded as the target type.
    fn from_row(row: &Row) -> crate::Result<Self>;
}

// Blanket impl for any `serde::Deserialize` type.
//
// Note: `Row` itself does NOT implement `Deserialize` (it derives only
// `Debug, Clone`), so this blanket does NOT match `Row`. A future
// hand-rolled `impl FromRow for Row` would therefore not collide with
// this blanket. Keep it that way — adding `Deserialize` to `Row` would
// silently shadow any such hand impl.
//
// `Rows::into_typed` routes through `T::from_row(&row)`, so this blanket
// covers the common `T: serde::Deserialize` path while hand-rolled
// `impl FromRow for MyType` overrides take precedence (no blanket-impl
// collision because user types pick exactly one impl).
//
// Errors from the blanket impl always set `column: None` because serde_json
// loses per-column context at the whole-object boundary. Hand-rolled FromRow
// impls can populate `column` for finer-grained diagnostics.
impl<T: DeserializeOwned> FromRow for T {
    fn from_row(row: &Row) -> crate::Result<Self> {
        // serde_json::from_value consumes its input; there's no from_value_ref API,
        // so the clone is unavoidable. Cost: O(columns) per row.
        let map = row.map().clone();
        let value = serde_json::Value::Object(map);
        serde_json::from_value(value).map_err(|e| Error::Decode {
            column: None,
            source: DecodeError::Serde(e.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Employee {
        #[serde(rename = "EMPNO")]
        empno: String,
        #[serde(rename = "SALARY")]
        salary: f64,
    }

    #[test]
    fn blanket_impl_decodes_a_row() {
        let serde_json::Value::Object(map) = json!({ "EMPNO": "000010", "SALARY": 52750.0 }) else {
            unreachable!()
        };
        let row = Row::from_map(map);
        let emp = Employee::from_row(&row).expect("FromRow decodes the test row");
        assert_eq!(
            emp,
            Employee {
                empno: "000010".into(),
                salary: 52750.0,
            }
        );
    }

    #[test]
    fn blanket_impl_surfaces_decode_error() {
        let serde_json::Value::Object(map) = json!({ "EMPNO": "000010" }) else {
            unreachable!()
        };
        let row = Row::from_map(map);
        let err = Employee::from_row(&row).expect_err("missing SALARY");
        match err {
            Error::Decode { column, .. } => assert_eq!(column, None),
            other => panic!("expected Error::Decode, got {other:?}"),
        }
    }

    #[test]
    fn hand_rolled_from_row_overrides_blanket() {
        // A type that doesn't implement Deserialize directly — only
        // FromRow. Verifies the trait works without the blanket.
        struct Custom {
            empno_int: i64,
            full_label: String,
        }
        impl FromRow for Custom {
            fn from_row(row: &Row) -> crate::Result<Self> {
                let empno: String = row.get("EMPNO")?;
                let salary: f64 = row.get("SALARY")?;
                Ok(Custom {
                    empno_int: empno.parse().unwrap_or_default(),
                    full_label: format!("emp {empno} @ ${salary}"),
                })
            }
        }

        let mut m = serde_json::Map::new();
        m.insert("EMPNO".into(), json!("12345"));
        m.insert("SALARY".into(), json!(75000.0));
        let row = Row::from_map(m);
        let c: Custom = Custom::from_row(&row).expect("FromRow decodes the test row");
        assert_eq!(c.empno_int, 12345);
        assert_eq!(c.full_label, "emp 12345 @ $75000");
    }
}
