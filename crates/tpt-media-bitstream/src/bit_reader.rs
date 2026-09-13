//! Bit-level reader over a borrowed byte slice.

use thiserror::Error;

/// Errors a [`BitReader`] can produce.
#[derive(Copy, Clone, Debug, Error, PartialEq, Eq)]
pub enum BitstreamError {
    /// The reader ran out of input bytes.
    #[error("end of input")]
    EndOfInput,
    /// A read or peek wider than 56 bits was requested.
    #[error("bit width {0} too large (max 56)")]
    BitWidthTooLarge(u32),
}

/// Big-endian, MSB-first bit reader.
///
/// Internally holds a 64-bit accumulator with `bits_in_acc` bits ready
/// to read. Each `read` consumes from the high end; when the
/// accumulator drops below 8 bits it is refilled from the next input
/// byte.
#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    bytes: &'a [u8],
    pos: usize,
    acc: u64,
    bits_in_acc: u32,
}

impl<'a> BitReader<'a> {
    /// Construct a reader over the given bytes.
    pub fn new(bytes: &'a [u8]) -> Self {
        BitReader {
            bytes,
            pos: 0,
            acc: 0,
            bits_in_acc: 0,
        }
    }

    /// Number of whole bytes consumed.
    #[inline]
    pub fn position_bytes(&self) -> usize {
        self.pos
    }

    /// Number of bits currently buffered in the accumulator (0..=56).
    #[inline]
    pub fn bits_buffered(&self) -> u32 {
        self.bits_in_acc
    }

    /// Number of bytes left in the input.
    #[inline]
    pub fn remaining_bytes(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    /// `true` if no more bytes can be read.
    #[inline]
    pub fn at_end(&self) -> bool {
        self.pos >= self.bytes.len() && self.bits_in_acc == 0
    }

    /// Skip to the next byte boundary.
    ///
    /// Only drops the partial-byte remainder of the *logical* stream
    /// position — not the whole accumulator, which (via `refill`'s
    /// read-ahead) can already hold several bytes past the current
    /// position that must stay available to later reads.
    pub fn align_to_byte(&mut self) {
        let drop = self.bits_in_acc % 8;
        self.bits_in_acc -= drop;
        self.acc &= mask64(self.bits_in_acc);
    }

    fn refill(&mut self) -> Result<(), BitstreamError> {
        let want_bits = 56u32.saturating_sub(self.bits_in_acc);
        let want_bytes = want_bits.div_ceil(8) as usize;
        let available = self.remaining_bytes();
        if available == 0 {
            return Err(BitstreamError::EndOfInput);
        }
        let take = want_bytes.min(available);
        for _ in 0..take {
            let b = self.bytes[self.pos];
            self.pos += 1;
            self.acc = (self.acc << 8) | (b as u64);
            self.bits_in_acc += 8;
        }
        Ok(())
    }

    fn refill_one(&mut self) -> Result<(), BitstreamError> {
        if self.pos >= self.bytes.len() {
            return Err(BitstreamError::EndOfInput);
        }
        let b = self.bytes[self.pos];
        self.pos += 1;
        self.acc = (self.acc << 8) | (b as u64);
        self.bits_in_acc += 8;
        Ok(())
    }
}

#[inline]
fn mask64(n: u32) -> u64 {
    if n == 0 {
        0
    } else if n >= 64 {
        u64::MAX
    } else {
        (1u64 << n) - 1
    }
}

impl<'a> BitReader<'a> {
    /// Skip `n` bits, ignoring their values.
    pub fn skip(&mut self, n: u32) -> Result<(), BitstreamError> {
        if n == 0 {
            return Ok(());
        }
        if n <= self.bits_in_acc {
            self.bits_in_acc -= n;
            self.acc &= mask64(self.bits_in_acc);
            return Ok(());
        }
        // Everything currently buffered is skipped outright; `remaining`
        // is how many more bits beyond that still need to go.
        let mut remaining = n - self.bits_in_acc;
        self.acc = 0;
        self.bits_in_acc = 0;
        // Whole bytes can be skipped by advancing `pos` directly, with no
        // need to load them into the accumulator at all.
        let whole_bytes = (remaining / 8) as usize;
        if whole_bytes > self.remaining_bytes() {
            return Err(BitstreamError::EndOfInput);
        }
        self.pos += whole_bytes;
        remaining -= whole_bytes as u32 * 8;
        if remaining > 0 {
            // A partial byte remains: its leading `remaining` bits are
            // skipped, but its trailing bits belong to the *next* read,
            // so the byte must be loaded (not skipped over) and only its
            // leading bits dropped.
            self.refill_one()?;
            self.bits_in_acc -= remaining;
            self.acc &= mask64(self.bits_in_acc);
        }
        Ok(())
    }

    /// Read up to 56 bits, MSB first.
    pub fn read(&mut self, n: u32) -> Result<u64, BitstreamError> {
        if n == 0 {
            return Ok(0);
        }
        if n > 56 {
            return Err(BitstreamError::BitWidthTooLarge(n));
        }
        if self.bits_in_acc < n {
            self.refill()?;
        }
        if self.bits_in_acc < n {
            return Err(BitstreamError::EndOfInput);
        }
        let shift = self.bits_in_acc - n;
        let v = self.acc >> shift;
        self.bits_in_acc -= n;
        self.acc &= mask64(self.bits_in_acc);
        Ok(v & mask64(n))
    }

    /// Peek without consuming.
    pub fn peek(&mut self, n: u32) -> Result<u64, BitstreamError> {
        if n == 0 {
            return Ok(0);
        }
        if n > 56 {
            return Err(BitstreamError::BitWidthTooLarge(n));
        }
        if self.bits_in_acc < n {
            self.refill()?;
        }
        if self.bits_in_acc < n {
            return Err(BitstreamError::EndOfInput);
        }
        let shift = self.bits_in_acc - n;
        Ok((self.acc >> shift) & mask64(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_bits_msb_first() {
        let bytes = [0b1011_0010u8, 0b0101_1110];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read(4).unwrap(), 0b1011);
        assert_eq!(r.read(4).unwrap(), 0b0010);
        assert_eq!(r.read(8).unwrap(), 0b0101_1110);
        assert!(r.at_end());
    }

    #[test]
    fn peek_does_not_consume() {
        let bytes = [0xFF];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.peek(4).unwrap(), 0b1111);
        assert_eq!(r.peek(4).unwrap(), 0b1111);
        assert_eq!(r.read(4).unwrap(), 0b1111);
        assert_eq!(r.read(4).unwrap(), 0b1111);
    }

    #[test]
    fn skip_to_byte_boundary() {
        let bytes = [0xAB, 0xCD];
        let mut r = BitReader::new(&bytes);
        r.read(3).unwrap();
        r.align_to_byte();
        assert_eq!(r.read(8).unwrap(), 0xCD);
    }

    #[test]
    fn skip_across_bytes() {
        let bytes = [0xFF, 0xFF, 0xFF];
        let mut r = BitReader::new(&bytes);
        r.read(4).unwrap();
        r.skip(20).unwrap();
        assert!(r.at_end());
    }

    #[test]
    fn skip_past_a_fresh_reader_leaves_trailing_bits_of_the_last_byte_readable() {
        // Regression: a fresh reader (nothing yet buffered) skipping a
        // non-byte-multiple of bits must still leave the *trailing* bits
        // of the last partially-skipped byte available to the next
        // read — the old implementation instead read one byte too far
        // (the byte *after* the skip region) for that leftover, silently
        // corrupting every read that followed a multi-byte `skip`. Here:
        // skip the top 4 bits of byte 0 plus all of... no, skip 12 bits
        // total (byte 0 entirely is 8 bits, plus the top 4 of byte 1),
        // so the remaining low nibble of byte 1 (`0xC` here) must be what
        // `read(4)` returns next, not anything from byte 2.
        let bytes = [0b1111_1111u8, 0b1010_1100, 0b0000_0000];
        let mut r = BitReader::new(&bytes);
        r.skip(12).unwrap();
        assert_eq!(r.read(4).unwrap(), 0b1100, "must read byte 1's low nibble");
        assert_eq!(
            r.read(8).unwrap(),
            0,
            "must continue into byte 2, not skip it"
        );
        assert!(r.at_end());
    }

    #[test]
    fn skip_an_exact_multiple_of_8_bits_consumes_no_extra_byte() {
        // Regression: when the skip lands exactly on a byte boundary
        // (no partial trailing byte), the old implementation still
        // speculatively loaded and partially retained one extra byte it
        // had no business touching. Skipping exactly 8 bits (one whole
        // byte) from a fresh reader must leave the *next* byte fully
        // intact for the following read.
        let bytes = [0xFFu8, 0b1010_0101];
        let mut r = BitReader::new(&bytes);
        r.skip(8).unwrap();
        assert_eq!(r.read(8).unwrap(), 0b1010_0101);
        assert!(r.at_end());
    }

    #[test]
    fn skip_beyond_available_bits_is_end_of_input() {
        let bytes = [0xFFu8, 0xFF];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.skip(17), Err(BitstreamError::EndOfInput));
    }

    #[test]
    fn end_of_input() {
        let bytes = [0xFF];
        let mut r = BitReader::new(&bytes);
        r.read(8).unwrap();
        assert_eq!(r.read(1), Err(BitstreamError::EndOfInput));
    }

    #[test]
    fn bit_width_too_large() {
        let bytes = [0xFF; 8];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read(57), Err(BitstreamError::BitWidthTooLarge(57)));
    }
}
