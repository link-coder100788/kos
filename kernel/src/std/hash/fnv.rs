const FNV_OFFSET_BASIS: u64 = 14695981039346656037;
const FNV_PRIME: u64 = 1099511628211;

pub fn fnv1a_64(data: &[u8]) -> u64 {
    fnv1a_64_continue(FNV_OFFSET_BASIS, data)
}

/// Continues a hash from a previous `state`, so hashing `a` then `b` gives
/// the same result as hashing `a ++ b`.
pub fn fnv1a_64_continue(state: u64, data: &[u8]) -> u64 {
    let mut hash = state;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}
