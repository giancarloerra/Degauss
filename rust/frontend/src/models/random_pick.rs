// Zaparoo Frontend
// Copyright (c) 2026 Wizzo Pty Ltd and the Zaparoo Project contributors.
// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0
//
// Shared uniform pick for the "random" actions. Both the favorites list and
// the games listing choose an entry by POSITION rather than asking Core's
// `**launch.random`, whose selection is weighted by the gaps between database
// row ids and in practice returns the same game nearly every time on a system
// whose rows were added over a long period.

/// Index in `0..len`, uniform enough for choosing a game.
///
/// Uses the standard library's randomly seeded hasher so the crate needs no
/// RNG dependency. The modulo skew against a `u64` seed is on the order of
/// `len / 2^64`, i.e. far below anything observable, and nothing here is
/// security-sensitive.
pub fn random_index(len: usize) -> usize {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    if len == 0 {
        return 0;
    }
    let seed = RandomState::new().build_hasher().finish();
    usize::try_from(seed % len as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, reason = "tests should fail-fast")]

    use super::random_index;
    use std::collections::HashSet;

    #[test]
    fn random_index_stays_in_range() {
        for len in 1..12usize {
            for _ in 0..64 {
                assert!(random_index(len) < len, "index must address a real row");
            }
        }
    }

    #[test]
    fn random_index_of_empty_pool_is_zero() {
        // Callers guard on a non-empty pool; this keeps the helper total
        // rather than dividing by zero if that guard ever moves.
        assert_eq!(random_index(0), 0);
    }

    #[test]
    fn random_index_eventually_varies() {
        // A fixed return would silently make every "random" action launch the
        // same entry, which is exactly the Core behavior this replaces.
        let picks: HashSet<usize> = (0..256).map(|_| random_index(8)).collect();
        assert!(picks.len() > 1, "picks must not be constant");
    }
}
