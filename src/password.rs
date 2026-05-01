//! Password handling — zeroize on drop, no Clone, no Serialize.
//!
//! [`Password`] is intentionally **not** [`Clone`], **not** [`serde::Serialize`],
//! and zeroizes its inner buffer on drop. Use it for the daemon password and
//! any other PCI-grade secret the crate handles.

// The test-only zeroization regression guard reads through a raw pointer
// while the buffer is still allocated (see the `zeroize_clears_buffer` test).
// `unsafe_code = "warn"` is intentional crate-wide; this attribute scopes
// the suppression to the test build only and only for this module.
#![cfg_attr(test, allow(unsafe_code))]

use std::fmt;

use zeroize::Zeroizing;

/// IBM i password held in a buffer that is zeroized on drop.
///
/// Construct with [`Password::new`]. There is **no** `Clone` impl — share via
/// [`std::sync::Arc`] if you need multiple owners. Debug formatting prints
/// `Password([REDACTED])` and never the inner bytes.
pub struct Password(Zeroizing<Box<str>>);

impl Password {
    /// Create a new password from an owned `String`.
    ///
    /// The original `String`'s heap buffer moves into a `Box<str>` and will
    /// be zeroized when this `Password` drops. Callers should not keep a
    /// separate reference to the source string.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value.into_boxed_str()))
    }

    /// Borrow the plaintext for wire serialization.
    ///
    /// This is the only escape hatch out of the `Zeroize` guarantee.
    /// Returning `&str` keeps the borrow tied to `self`'s lifetime, so
    /// the bytes are not extended past the `Password` itself — **as
    /// long as the caller does not derive an owned copy**. Calling
    /// `.to_string()` / `.into()` / `.as_bytes().to_vec()` on the
    /// returned slice creates a non-zeroizing copy on the heap that
    /// outlives the source `Password`.
    ///
    /// The only call site in v0.2 is `transport::handshake::connect`,
    /// which clones the bytes into the `Connect` wire request via
    /// `.to_string()`. The cloned `String` lives until the request is
    /// serialized into outgoing JSON and dropped. The bytes sit in
    /// unreclaimed heap memory until the allocator reuses the page.
    /// This is an accepted tradeoff at the wire-protocol boundary,
    /// documented in `SECURITY.md`. A future revision could thread
    /// `Zeroizing<String>` through `Request::Connect` to close the gap.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_does_not_leak() {
        let p = Password::new("super-secret".to_string());
        let s = format!("{p:?}");
        assert_eq!(s, "Password([REDACTED])");
        assert!(!s.contains("super-secret"));
    }

    #[test]
    fn expose_returns_inner() {
        let p = Password::new("abc123".to_string());
        assert_eq!(p.expose(), "abc123");
    }

    /// Verify that the inner buffer's `Zeroize` impl actually zeroes the
    /// bytes. Using `ManuallyDrop` to suspend the buffer's deallocation
    /// lets us read the bytes back **while they're still allocated**, so
    /// no use-after-free read is required and the test is portable across
    /// allocators (notably stable on Linux/glibc, which scribbles freed
    /// memory with bookkeeping data and would defeat a post-drop read).
    ///
    /// This is a runtime check that calling `zeroize()` zeros the bytes.
    /// The static guarantee that `Zeroizing<Box<str>>` runs `zeroize()` on
    /// drop comes from the `Zeroizing` wrapper itself, exercised in
    /// production via the `Drop` impl whenever a real `Password` falls
    /// out of scope.
    #[test]
    fn zeroize_clears_buffer() {
        use std::mem::ManuallyDrop;

        use zeroize::Zeroize;

        let mut p = ManuallyDrop::new(Password::new("ABCDEFGH".to_string()));
        let len = p.0.len();
        let ptr = p.0.as_ptr();

        // Sanity check: bytes are present before zeroize.
        // Safety: pointer is to a live, owned, non-empty buffer that this
        // function has exclusive access to via `p`.
        let before = unsafe { std::slice::from_raw_parts(ptr, len) };
        assert_eq!(before, b"ABCDEFGH");

        // Invoke the same operation `Zeroizing<Box<str>>::drop` runs.
        p.0.zeroize();

        // Buffer is still allocated (ManuallyDrop suppresses Box::drop).
        // Safety: same pointer, same exclusive access, just-zeroed.
        let after = unsafe { std::slice::from_raw_parts(ptr, len) };
        assert!(
            after.iter().all(|&b| b == 0),
            "expected zeroized buffer, got {after:?}"
        );

        // Release the allocation cleanly.
        // Safety: we own the only handle to the inner value and do not
        // access it after this call.
        unsafe { ManuallyDrop::drop(&mut p) };
    }
}
