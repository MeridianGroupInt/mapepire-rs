// TODO(v0.3 Task 17 / PRO-447): delete this file once Executor impls land.
//! Compile-time assertion that the public traits are nameable and
//! object-safe. Deleted in v0.3 Task 17 once real `Executor` impls land
//! and proper integration tests cover the trait surface.

use mapepire::{Executor, FromRow};

#[allow(dead_code)]
fn _executor_object_safety(_: &dyn Executor) {}

#[allow(dead_code)]
fn _from_row_nameable<T: FromRow>(_: &T) {}
