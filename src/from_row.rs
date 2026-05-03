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
impl<T: DeserializeOwned> FromRow for T {
    fn from_row(row: &Row) -> crate::Result<Self> {
        let value = serde_json::Value::Object(row.map().clone());
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
        let row = Row::from_map_for_test(map);
        let emp = Employee::from_row(&row).expect("decode");
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
        let row = Row::from_map_for_test(map);
        let err = Employee::from_row(&row).expect_err("missing SALARY");
        match err {
            Error::Decode { column, .. } => assert_eq!(column, None),
            other => panic!("expected Error::Decode, got {other:?}"),
        }
    }
}
