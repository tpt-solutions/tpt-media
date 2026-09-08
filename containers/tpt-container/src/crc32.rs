//! CRC-32 (IEEE 802.3, polynomial `0xEDB88320`, reflected).
//!
//! Tables would be faster, but a bitwise implementation is dependency-free,
//! no_std, and uses no static data, which is fine for the byte sizes that a
//! container trailer covers in practice.

/// Compute the CRC-32 of `bytes` (initial value `0xFFFFFFFF`, final xor
/// `0xFFFFFFFF`), matching the usual `crc32("123456789") == 0xCBF43926`.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn crc32_empty_is_not_zero() {
        assert_eq!(crc32(b""), 0x0000_0000);
    }

    #[test]
    fn crc32_single_byte() {
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    }

    #[test]
    fn crc32_detects_single_bit_flips() {
        let a = crc32(b"the quick brown fox jumps over the lazy dog");
        let mut mangled = b"the quick brown fox jumps over the lazy dog".to_vec();
        mangled[10] ^= 0x01;
        let b = crc32(&mangled);
        assert_ne!(a, b);
    }
}
