use rand::Rng;

/// Alphabet excludes ambiguous characters (0, 1, i, l, o) — SPEC §1.
const ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";

pub const ID_LEN: usize = 6;

/// Generate a fresh 6-character task id.
pub fn new_id() -> String {
    let mut rng = rand::thread_rng();
    (0..ID_LEN)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Resolve user-supplied id text: exact match is required after trimming.
pub fn normalize(input: &str) -> &str {
    input.trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn shape() {
        for _ in 0..200 {
            let id = new_id();
            assert_eq!(id.len(), ID_LEN);
            assert!(id.bytes().all(|b| ALPHABET.contains(&b)));
        }
    }

    #[test]
    fn unique_enough() {
        let seen: HashSet<_> = (0..10_000).map(|_| new_id()).collect();
        // Birthday bound: 10k draws from 23^6 ≈ 1.5e8 expect ~0.34 random
        // collisions, so requiring perfect uniqueness makes this test fail
        // ~1 run in 3. A systematic generator bug produces far more than a
        // handful.
        assert!(
            seen.len() >= 9_995,
            "excessive id collisions: {}",
            10_000 - seen.len()
        );
    }
}
