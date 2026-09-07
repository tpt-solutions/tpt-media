//! Audio sample format enumeration.

/// Audio sample format.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SampleFormat {
    /// Unsigned 8-bit.
    U8 = 0,
    /// Signed 16-bit little-endian.
    S16 = 1,
    /// Signed 24-bit little-endian (3 bytes per sample).
    S24 = 2,
    /// Signed 32-bit little-endian.
    S32 = 3,
    /// 32-bit IEEE float little-endian.
    F32 = 4,
    /// 64-bit IEEE float little-endian.
    F64 = 5,
}

impl SampleFormat {
    /// Bytes per sample.
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            SampleFormat::U8 => 1,
            SampleFormat::S16 => 2,
            SampleFormat::S24 => 3,
            SampleFormat::S32 => 4,
            SampleFormat::F32 => 4,
            SampleFormat::F64 => 8,
        }
    }

    /// `true` if this is a floating-point sample format.
    pub const fn is_float(self) -> bool {
        matches!(self, SampleFormat::F32 | SampleFormat::F64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_per_sample_table() {
        assert_eq!(SampleFormat::U8.bytes_per_sample(), 1);
        assert_eq!(SampleFormat::S16.bytes_per_sample(), 2);
        assert_eq!(SampleFormat::S24.bytes_per_sample(), 3);
        assert_eq!(SampleFormat::S32.bytes_per_sample(), 4);
        assert_eq!(SampleFormat::F32.bytes_per_sample(), 4);
        assert_eq!(SampleFormat::F64.bytes_per_sample(), 8);
    }

    #[test]
    fn is_float() {
        assert!(!SampleFormat::U8.is_float());
        assert!(!SampleFormat::S16.is_float());
        assert!(SampleFormat::F32.is_float());
        assert!(SampleFormat::F64.is_float());
    }
}