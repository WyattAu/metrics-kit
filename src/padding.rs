//! Cache-line padding to prevent false sharing between worker threads.

/// Wraps a value in a 64-byte-aligned cell so independently updated metrics
/// on the same cache line do not invalidate each other's cache lines.
///
/// The hot path only ever performs relaxed atomic fetch-adds; the padding
/// guarantees each metric owns its own cache line on x86-64 and aarch64.
#[repr(align(64))]
#[derive(Debug, Default)]
pub struct CachePadded<T>(pub T);

impl<T> CachePadded<T> {
    /// Creates a new padded cell.
    pub const fn new(value: T) -> Self {
        Self(value)
    }
}

impl<T> std::ops::Deref for CachePadded<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn padded_size_is_at_least_one_cache_line() {
        assert!(std::mem::size_of::<CachePadded<u64>>() >= 64);
    }

    #[test]
    fn deref_reads_inner() {
        let cell = CachePadded::new(7u64);
        assert_eq!(*cell, 7);
    }
}
