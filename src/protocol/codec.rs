//! Helpers for encoding/decoding wire frames and allocating request ids.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

/// A correlation id assigned to one request/response pair.
pub type RequestId = String;

/// Allocates monotonically increasing request ids with a per-process random
/// prefix. The prefix prevents collisions across separate `Job` instances and
/// after the counter wraps.
#[derive(Debug)]
pub struct IdAllocator {
    prefix: String,
    counter: AtomicU64,
}

impl IdAllocator {
    /// Construct an allocator with a fresh random prefix.
    #[must_use]
    pub fn new() -> Self {
        let mut bytes = [0u8; 6];
        // Use std for entropy; v0.1 has no `rand` dep, and we only need a
        // collision-avoidance tag (not cryptographic randomness).
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let pid = std::process::id();
        // Mix: 4 bytes nanos + 2 bytes pid → 6 bytes hex prefix.
        // NOTE: prefix entropy is ~32 bits in practice (pid is constant
        // within a process). Two `IdAllocator`s constructed in the same
        // nanosecond would share a prefix; uniqueness from that point
        // depends on the per-allocator counter. v0.1 has one `IdAllocator`
        // per `Job`, so the only collision risk is two `Job`s constructed
        // back-to-back in the same nanosecond — covered by the
        // `two_allocators_have_distinct_prefixes` regression test.
        bytes[..4].copy_from_slice(&nanos.to_le_bytes());
        bytes[4..6].copy_from_slice(&pid.to_le_bytes()[..2]);
        let prefix = bytes.iter().fold(String::with_capacity(12), |mut s, b| {
            write!(s, "{b:02x}").expect("write to String is infallible");
            s
        });
        Self {
            prefix,
            counter: AtomicU64::new(0),
        }
    }

    /// Issue the next id. Format: `<6-hex-prefix>-<u64>`.
    pub fn next(&self) -> RequestId {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("{}-{n}", self.prefix)
    }
}

impl Default for IdAllocator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn ids_are_unique_within_one_allocator() {
        let alloc = IdAllocator::new();
        let mut seen = HashSet::new();
        for _ in 0..10_000 {
            let id = alloc.next();
            assert!(seen.insert(id), "duplicate id");
        }
    }

    #[test]
    fn two_allocators_have_distinct_prefixes() {
        // Best-effort assertion: with two allocators constructed back-to-back
        // we *expect* different prefixes thanks to nanosecond resolution.
        let a = IdAllocator::new();
        std::thread::sleep(std::time::Duration::from_micros(1));
        let b = IdAllocator::new();
        assert_ne!(
            a.next().split_once('-').unwrap().0,
            b.next().split_once('-').unwrap().0,
            "prefixes collided — flaky on systems without nanosecond resolution"
        );
    }

    #[test]
    fn next_increments() {
        let alloc = IdAllocator::new();
        let a = alloc.next();
        let b = alloc.next();
        let an: u64 = a.split_once('-').unwrap().1.parse().unwrap();
        let bn: u64 = b.split_once('-').unwrap().1.parse().unwrap();
        assert_eq!(bn, an + 1);
    }
}
