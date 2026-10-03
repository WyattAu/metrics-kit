//! Lock-free last-writer-wins exemplar storage.
//!
//! One fixed-capacity slot per [`Counter`](crate::Counter), written by the
//! record path and read by the scrape path. Writers follow a seqlock
//! protocol (odd sequence = mid-write); readers validate via the sequence
//! counter and retry under contention, exactly like the estate's
//! percentile ring. Every string is packed into fixed `u64` words with a
//! stored length, so neither recording nor rendering ever allocates on the
//! record path and no lock is ever taken.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::padding::CachePadded;

/// Additional exemplar label pairs beyond the primary pair
/// (`Counter::with_exemplar` takes the primary pair plus up to three
/// context pairs).
pub const MAX_EXEMPLAR_EXTRA_PAIRS: usize = 3;
/// Maximum bytes retained per exemplar label key or value; longer strings
/// are truncated at a UTF-8 char boundary.
pub const MAX_EXEMPLAR_STR_BYTES: usize = 40;

/// Total pairs stored: the primary pair plus [`MAX_EXEMPLAR_EXTRA_PAIRS`].
pub(crate) const MAX_PAIRS: usize = 1 + MAX_EXEMPLAR_EXTRA_PAIRS;

const STR_WORDS: usize = MAX_EXEMPLAR_STR_BYTES / 8;

/// A consistent snapshot of an exemplar, taken at render time (the only
/// allocation on the exemplar path; scrapes are not the hot path).
pub(crate) struct ExemplarSnapshot {
    pub(crate) labels: Vec<(String, String)>,
    pub(crate) value: f64,
}

/// The per-series exemplar slot. Whole-slot cache padding keeps the
/// recording writer from invalidating the counter's own line.
pub(crate) struct ExemplarSlot {
    seq: CachePadded<AtomicU64>,
    value_bits: CachePadded<AtomicU64>,
    pair_count: CachePadded<AtomicU64>,
    lens: CachePadded<[AtomicU64; MAX_PAIRS * 2]>,
    words: CachePadded<[[AtomicU64; STR_WORDS]; MAX_PAIRS * 2]>,
}

impl ExemplarSlot {
    pub(crate) fn new() -> Self {
        Self {
            seq: CachePadded::new(AtomicU64::new(0)),
            value_bits: CachePadded::new(AtomicU64::new(0)),
            pair_count: CachePadded::new(AtomicU64::new(0)),
            lens: CachePadded::new(std::array::from_fn(|_| AtomicU64::new(0))),
            words: CachePadded::new(std::array::from_fn(|_| {
                [const { AtomicU64::new(0) }; STR_WORDS]
            })),
        }
    }

    /// Stores an exemplar (last-writer-wins). `pairs` holds the primary
    /// pair first; only the first `n` are retained.
    pub(crate) fn store(&self, pairs: &[(&str, &str)], n: usize, value: f64) {
        // Odd sequence marks mid-write for concurrent readers.
        self.seq.fetch_add(1, Ordering::AcqRel);
        let n = n.min(MAX_PAIRS);
        self.pair_count.store(n as u64, Ordering::Relaxed);
        for (i, (key, val)) in pairs.iter().take(n).enumerate() {
            if let Some(len_cell) = self.lens.get(i * 2) {
                pack_str(len_cell, key);
            }
            if let Some(len_cell) = self.lens.get(i * 2 + 1) {
                pack_str(len_cell, val);
            }
            if let Some(word_cells) = self.words.get(i * 2) {
                pack_str_words(word_cells, key);
            }
            if let Some(word_cells) = self.words.get(i * 2 + 1) {
                pack_str_words(word_cells, val);
            }
        }
        self.value_bits.store(value.to_bits(), Ordering::Relaxed);
        // Even sequence: the write is complete.
        self.seq.fetch_add(1, Ordering::AcqRel);
    }

    /// Clears the slot so the next render omits the exemplar.
    pub(crate) fn clear(&self) {
        self.seq.fetch_add(1, Ordering::AcqRel);
        self.pair_count.store(0, Ordering::Relaxed);
        self.seq.fetch_add(1, Ordering::AcqRel);
    }

    /// Takes a consistent snapshot, retrying under writer contention.
    /// Returns `None` when no exemplar is stored or the slot stays
    /// mid-write for eight consecutive attempts (a panicking writer leaves
    /// the sequence odd forever; one scrape without the exemplar is the
    /// correct degradation).
    pub(crate) fn load(&self) -> Option<ExemplarSnapshot> {
        for _ in 0..8 {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 == 1 {
                continue;
            }
            let n = self
                .pair_count
                .load(Ordering::Relaxed)
                .min(MAX_PAIRS as u64) as usize;
            let mut labels = Vec::with_capacity(n);
            for i in 0..n {
                let key = self
                    .lens
                    .get(i * 2)
                    .map(|len| unpack_str(len, self.words.get(i * 2)))
                    .unwrap_or_default();
                let val = self
                    .lens
                    .get(i * 2 + 1)
                    .map(|len| unpack_str(len, self.words.get(i * 2 + 1)))
                    .unwrap_or_default();
                labels.push((key, val));
            }
            let value = f64::from_bits(self.value_bits.load(Ordering::Relaxed));
            let after = self.seq.load(Ordering::Acquire);
            if before == after {
                return if labels.is_empty() {
                    None
                } else {
                    Some(ExemplarSnapshot { labels, value })
                };
            }
        }
        None
    }
}

impl fmt::Debug for ExemplarSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExemplarSlot")
            .field("populated", &self.load().is_some())
            .finish_non_exhaustive()
    }
}

/// Stores the byte length of `s`, truncated at a UTF-8 char boundary.
fn pack_str(len_cell: &AtomicU64, s: &str) {
    let bytes = s.as_bytes();
    let mut take = bytes.len().min(MAX_EXEMPLAR_STR_BYTES);
    while take > 0 && !s.is_char_boundary(take) {
        take -= 1;
    }
    len_cell.store(take as u64, Ordering::Relaxed);
}

/// Packs `s` into fixed-width `u64` words, little-endian per word.
fn pack_str_words(word_cells: &[AtomicU64; STR_WORDS], s: &str) {
    let bytes = s.as_bytes();
    for (w, word_cell) in word_cells.iter().enumerate() {
        let mut word = 0u64;
        for i in 0..8 {
            if let Some(&b) = bytes.get(w * 8 + i) {
                word |= (b as u64) << (8 * i);
            }
        }
        word_cell.store(word, Ordering::Relaxed);
    }
}

/// Rebuilds a packed string; always valid UTF-8 because the writer
/// truncated at a char boundary.
fn unpack_str(len_cell: &AtomicU64, word_cells: Option<&[AtomicU64; STR_WORDS]>) -> String {
    let len = len_cell
        .load(Ordering::Relaxed)
        .min(MAX_EXEMPLAR_STR_BYTES as u64) as usize;
    let Some(word_cells) = word_cells else {
        return String::new();
    };
    let mut bytes = [0u8; MAX_EXEMPLAR_STR_BYTES];
    for (w, word_cell) in word_cells.iter().enumerate() {
        let word = word_cell.load(Ordering::Relaxed);
        for i in 0..8 {
            let idx = w * 8 + i;
            if idx < len {
                if let Some(slot) = bytes.get_mut(idx) {
                    *slot = ((word >> (8 * i)) & 0xFF) as u8;
                }
            }
        }
    }
    String::from_utf8_lossy(bytes.get(..len).unwrap_or(&[])).into_owned()
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;

    #[test]
    fn roundtrips_pairs_and_value() {
        let slot = ExemplarSlot::new();
        assert!(slot.load().is_none());
        slot.store(&[("trace_id", "abc123"), ("span", "root")], 2, 5.0);
        let snap = slot.load().expect("exemplar");
        assert_eq!(snap.labels.len(), 2);
        assert_eq!(
            snap.labels[0],
            ("trace_id".to_string(), "abc123".to_string())
        );
        assert_eq!(snap.labels[1], ("span".to_string(), "root".to_string()));
        assert_eq!(snap.value, 5.0);
    }

    #[test]
    fn last_writer_wins() {
        let slot = ExemplarSlot::new();
        slot.store(&[("trace_id", "first")], 1, 1.0);
        slot.store(&[("trace_id", "second")], 1, 2.0);
        let snap = slot.load().expect("exemplar");
        assert_eq!(snap.labels[0].1, "second");
        assert_eq!(snap.value, 2.0);
    }

    #[test]
    fn long_strings_truncate_at_char_boundary() {
        let slot = ExemplarSlot::new();
        let long_ascii = "x".repeat(200);
        // Each 'é' is two bytes; a 40-byte cut would split one.
        let multibyte = "é".repeat(40);
        slot.store(&[(&long_ascii, &multibyte)], 1, 1.0);
        let snap = slot.load().expect("exemplar");
        let (k, v) = &snap.labels[0];
        assert_eq!(k.len(), MAX_EXEMPLAR_STR_BYTES);
        assert!(v.len() <= MAX_EXEMPLAR_STR_BYTES);
        assert!(v.ends_with('é'), "cut must land on a char boundary: {v:?}");
    }

    #[test]
    fn clear_removes_the_exemplar() {
        let slot = ExemplarSlot::new();
        slot.store(&[("t", "v")], 1, 1.0);
        assert!(slot.load().is_some());
        slot.clear();
        assert!(slot.load().is_none());
    }

    #[test]
    fn extra_pairs_beyond_capacity_are_ignored() {
        let slot = ExemplarSlot::new();
        slot.store(
            &[
                ("p", "0"),
                ("a", "1"),
                ("b", "2"),
                ("c", "3"),
                ("d", "4"),
                ("e", "5"),
            ],
            MAX_PAIRS,
            9.0,
        );
        let snap = slot.load().expect("exemplar");
        assert_eq!(snap.labels.len(), MAX_PAIRS);
        assert_eq!(
            snap.labels[MAX_PAIRS - 1],
            ("c".to_string(), "3".to_string())
        );
    }
}
