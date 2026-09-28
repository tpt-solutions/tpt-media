//! Codec identifier and configuration blob.

use alloc::vec::Vec;

/// Stable codec identifier.
///
/// The 4 bytes are ASCII (e.g. `b"CIND"`, `b"AURA"`, `b"REED"`,
/// `b"THRM"`, `b"VAUL"`, `b"MOTE"`). Reserved codec IDs begin with
/// `0x00`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct CodecId([u8; 4]);

impl CodecId {
    /// Build from 4 ASCII bytes.
    pub const fn new(bytes: [u8; 4]) -> Self {
        CodecId(bytes)
    }

    /// The 4-byte ASCII representation.
    pub const fn as_bytes(self) -> [u8; 4] {
        self.0
    }

    /// `true` if the ID is ASCII-printable.
    pub fn is_ascii(self) -> bool {
        self.0.iter().all(|b| (0x20..=0x7E).contains(b))
    }

    /// `true` if this is a TPT-native codec ID.
    pub fn is_native(self) -> bool {
        self.is_ascii() && !matches!(self.0, [0, 0, 0, 0])
    }
}

impl core::fmt::Display for CodecId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // SAFETY: is_ascii ensures the bytes are printable.
        let s = core::str::from_utf8(&self.0).unwrap_or("????");
        f.write_str(s)
    }
}

impl core::str::FromStr for CodecId {
    type Err = CodecIdParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() != 4 {
            return Err(CodecIdParseError::WrongLength);
        }
        let mut out = [0u8; 4];
        out.copy_from_slice(bytes);
        if !out.iter().all(|b| (0x20..=0x7E).contains(b)) {
            return Err(CodecIdParseError::NotAscii);
        }
        Ok(CodecId(out))
    }
}

/// Error parsing a [`CodecId`] from a string.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CodecIdParseError {
    /// String was not exactly 4 bytes.
    WrongLength,
    /// String contained non-printable-ASCII bytes.
    NotAscii,
}

impl core::fmt::Display for CodecIdParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CodecIdParseError::WrongLength => f.write_str("codec id must be exactly 4 bytes"),
            CodecIdParseError::NotAscii => f.write_str("codec id must be printable ASCII"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CodecIdParseError {}

/// Codec-specific configuration blob.
///
/// Codecs serialize their setup parameters (sample rate, frame size,
/// channel layout, private tables, codebook, etc.) into `data`. The
/// blob is opaque to the container and to the codec framework; only
/// the codec itself knows how to parse it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CodecConfig {
    /// Codec identifier.
    pub codec_id: CodecId,
    /// Bitstream version (`major << 8 | minor`).
    pub version: u16,
    /// Codec-specific configuration bytes.
    pub data: Vec<u8>,
}

impl CodecConfig {
    /// Empty configuration for the given codec ID.
    pub fn empty(codec_id: CodecId) -> Self {
        CodecConfig {
            codec_id,
            version: 1,
            data: Vec::new(),
        }
    }

    /// Length of the configuration in bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// `true` if the configuration carries no bytes.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::str::FromStr;

    #[test]
    fn codec_id_round_trip() {
        let id = CodecId::new(*b"CIND");
        assert_eq!(format!("{}", id), "CIND");
        assert_eq!(CodecId::from_str("CIND").unwrap(), id);
    }

    #[test]
    fn codec_id_rejects_bad_input() {
        assert!(CodecId::from_str("PUL").is_err());
        assert!(CodecId::from_str("PUL\x00S").is_err());
    }

    #[test]
    fn codec_config_default() {
        let c = CodecConfig::empty(CodecId::new(*b"REED"));
        assert!(c.is_empty());
        assert_eq!(c.version, 1);
    }
}
