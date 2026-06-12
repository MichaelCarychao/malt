// FNV-1a 64-bit — a STABLE hash. std's DefaultHasher explicitly reserves
// the right to change its algorithm between Rust releases, so it must never
// feed anything persisted to disk or used as an on-disk identity (a
// toolchain bump would silently invalidate every stored value). Use this
// for persisted content hashes and derived file names; keep DefaultHasher
// for purely in-memory, single-run work.

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn fnv1a64_str(s: &str) -> u64 {
    fnv1a64(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // Published FNV-1a 64 test vectors.
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }
}
