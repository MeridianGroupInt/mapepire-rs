//! Helpers for encoding/decoding wire frames and allocating request ids.

use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// A correlation id assigned to one request/response pair.
pub type RequestId = String;

/// Process-wide monotonic instance counter mixed into every [`IdAllocator`]
/// prefix so two allocators constructed in the same clock tick (for example
/// back-to-back `Job`s or concurrent `Rows::stream` cursors sharing one
/// dispatcher) never share a prefix.
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(0);

/// Allocates monotonically increasing request ids with a unique per-instance
/// hex prefix. The prefix prevents collisions across separate `Job` /
/// `Rows::stream` instances, and epoch rollover prevents collisions after
/// `u64` counter wraparound.
#[derive(Debug)]
pub struct IdAllocator {
    prefix: String,
    state: Mutex<(u64, u64)>,
}

impl IdAllocator {
    /// Construct an allocator with a fresh collision-free prefix.
    #[must_use]
    pub fn new() -> Self {
        Self::with_initial_counter(0)
    }

    fn with_initial_counter(initial: u64) -> Self {
        let mut bytes = [0u8; 12];
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let pid = std::process::id();
        let instance = NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed);
        // Mix 4 bytes (nanos XOR rotated pid) + 8 verbatim bytes of the
        // process-wide monotonic `instance` counter → 12 bytes (24 hex chars).
        // Including all 64 bits of `instance` guarantees distinct prefixes
        // across 2^64 allocators in the same process even when the system
        // clock does not advance between calls.
        let salt = nanos ^ pid.rotate_left(16);
        bytes[..4].copy_from_slice(&salt.to_le_bytes());
        bytes[4..12].copy_from_slice(&instance.to_le_bytes());
        let mut prefix = String::with_capacity(24);
        for b in bytes {
            let _ = write!(prefix, "{b:02x}");
        }
        Self {
            prefix,
            state: Mutex::new((0, initial)),
        }
    }

    /// Issue the next id. Format: `<hex-prefix>-<u64>`.
    pub fn next(&self) -> RequestId {
        let (e, c) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (epoch, counter) = *state;
            if counter == u64::MAX {
                *state = (epoch.wrapping_add(1), 0);
            } else {
                *state = (epoch, counter + 1);
            }
            (epoch, counter)
        };
        if e == 0 {
            format!("{}-{c}", self.prefix)
        } else {
            format!("{}{e:x}-{c}", self.prefix)
        }
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
        // Guaranteed deterministic without sleeping: `NEXT_INSTANCE` separates
        // allocators constructed in the exact same clock tick.
        let mut prefixes = HashSet::with_capacity(1_024);
        for _ in 0..1_024 {
            let alloc = IdAllocator::new();
            let id = alloc.next();
            let prefix = id.split_once('-').unwrap().0.to_owned();
            assert!(prefixes.insert(prefix), "allocator prefix collided");
        }
    }

    #[test]
    fn ids_remain_unique_across_counter_wraparound() {
        let alloc = IdAllocator::with_initial_counter(u64::MAX - 4);
        let early_zero = format!("{}-0", alloc.prefix);
        let mut seen = HashSet::new();
        seen.insert(early_zero);
        for _ in 0..10 {
            let id = alloc.next();
            assert!(
                seen.insert(id.clone()),
                "id collided across counter wraparound: {id}"
            );
        }
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

    #[test]
    fn test_next_concurrent_counter_wraparound_has_no_duplicates() {
        let alloc = IdAllocator::with_initial_counter(u64::MAX - 64);
        let barrier = std::sync::Barrier::new(8);
        let all_ids: Vec<RequestId> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    s.spawn(|| {
                        barrier.wait();
                        (0..32).map(|_| alloc.next()).collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect()
        });

        let seen: HashSet<RequestId> = all_ids.into_iter().collect();
        pretty_assertions::assert_eq!(seen.len(), 256);

        let epoch0_prefix = alloc.prefix.as_str();
        let epoch1_prefix = format!("{}1", alloc.prefix);
        let mut epoch0_count = 0usize;
        let mut epoch1_count = 0usize;
        for id in &seen {
            let (pfx, counter_str) = id.split_once('-').unwrap();
            let c: u64 = counter_str.parse().unwrap();
            if pfx == epoch0_prefix {
                assert!(c >= u64::MAX - 64, "unexpected epoch-0 counter value: {c}");
                epoch0_count += 1;
            } else {
                pretty_assertions::assert_eq!(pfx, epoch1_prefix);
                assert!(c < 191, "unexpected epoch-1 counter value: {c}");
                epoch1_count += 1;
            }
        }
        pretty_assertions::assert_eq!(epoch0_count, 65);
        pretty_assertions::assert_eq!(epoch1_count, 191);
    }
}
