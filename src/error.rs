//! Crate-wide error types. Expanded in Task 7.

/// Placeholder error. Replaced in Task 7.
#[derive(Debug)]
pub struct Error;

/// Crate result alias. Replaced in Task 7 with the real `Error` enum.
pub type Result<T> = std::result::Result<T, Error>;
