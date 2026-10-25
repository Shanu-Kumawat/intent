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
        assert_eq!(seen.len(), 10_000, "collision in 10k ids is a bug");
    }
}
