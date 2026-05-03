//! Pool routing registry — full impl in Task 23 / PRO-453.
//!
//! For Tasks 10–22 this is a no-op unit struct that satisfies the type
//! checker so `Pool::execute` (Task 11) and `Pool::acquire` (Task 13)
//! can compile against the future routing surface without behavior
//! coupling. Task 23 / PRO-453 fills in the weak-handle registry.

#[derive(Default)]
pub(crate) struct Registry;
