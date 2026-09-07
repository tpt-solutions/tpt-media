//! Bit-level writer over a borrowed mutable byte slice.

use thiserror::Error;

/// Errors a [`BitWriter`] can produce.
#[derive(Copy, Clone, Debug, Error, PartialEq, Eq)]
pub enum WriteError {
    /// The output buffer is full and cannot accept more bytes.
    #[error("output buffer full")]
    BufferFull,
    /// A write wider than 56 bits was requested.
    #[error("bit width {0} too large (max 56)")]
    BitWidthTooLarge(u32),
}

/// Big-endian, MSB-first bit writer.
///
/// Writes go into a 64-bit accumulator; when the accumulator has 8 or
/// more bits, bytes are flushed into the output buffer.
#[derive(Debug)]
pub struct BitWriter<'a> {
    out: &'a mut [u8],
    pos: usize,
    acc: u64,
    bits_in_acc: u32,
    /// Number of bits that would have been flushed but were not because
    /// the buffer was full. Surfaced by [`BitWriter::finish`].
    pending_bits: u32,
    /// Pending bits (low `bits_in_acc` of `acc`).
    pending_value: u64,
}

impl<'a> BitWriter<'a> {
    /// Construct a writer over the given buffer.
    pub fn new(out: &'a mut [u8]) -> Self {
        BitWriter {
            out,
            pos: 0,
            acc: 0,
            bits_in_acc: 0,
            pending_bits: 0,
            pending_value: 0,
        }
    }

    /// Capacity of the underlying buffer in bytes.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.out.len()
    }

    /// Number of bytes already written.
    #[inline]
    pub fn position_bytes(&self) -> usize {
        self.pos
    }

    /// Number of bits currently buffered (not yet flushed).
    #[inline]
    pub fn bits_buffered(&self) -> u32 {
        self.bits_in_acc
    }

    /// Pad to the next byte boundary with zero bits.
    pub fn align_to_byte(&mut self) {
        while self.bits_in_acc >= 8 {
            let byte = (self.acc >> (self.bits_in_acc - 8)) as u8;
            if self.pos < self.out.len() {
                self.out[self.pos] = byte;
                self.pos += 1;
            } else {
                // Buffer full; surface later via finish().
                self.pending_bits = self.bits_in_acc;
                self.pending_value = self.acc;
                self.bits_in_acc = 0;
                self.acc = 0;
                return;
            }
            self.bits_in_acc -= 8;
        }
        if self.bits_in_acc > 0 {
            // Pad a partial byte with zeros.
            let byte = ((self.acc << (8 - self.bits_in_acc)) & 0xFF) as u8;
            if self.pos < self.out.len() {
                self.out[self.pos] = byte;
                self.pos += 1;
            } else {
                self.pending_bits = 8;
                self.pending_value = byte as u64;
            }
            self.bits_in_acc = 0;
            self.acc = 0;
        }
    }

    /// Write the low `n` bits of `value` MSB-first.
    pub fn write(&mut self, n: u32, value: u64) -> Result<(), WriteError> {
        if n == 0 {
            return Ok(());
        }
        if n > 56 {
            return Err(WriteError::BitWidthTooLarge(n));
        }
        let v = value & mask(n);
        self.acc = (self.acc << n) | v;
        self.bits_in_acc += n;
        while self.bits_in_acc >= 8 {
            self.bits_in_acc -= 8;
            let byte = (self.acc >> self.bits_in_acc) as u8;
            if self.pos < self.out.len() {
                self.out[self.pos] = byte;
                self.pos += 1;
            } else {
                self.pending_bits = self.bits_in_acc;
                self.pending_value = self.acc & mask(self.bits_in_acc);
                self.acc = 0;
                self.bits_in_acc = 0;
                return Err(WriteError::BufferFull);
            }
        }
        Ok(())
    }

    /// Flush any partial trailing bits into the buffer, returning the
    /// total number of bytes written and surfacing any data lost to
    /// buffer-full conditions.
    pub fn finish(mut self) -> Result<usize, WriteError> {
        if self.pending_bits > 0 {
            return Err(WriteError::BufferFull);
        }
        self.align_to_byte();
        if self.bits_in_acc > 0 {
            return Err(WriteError::BufferFull);
        }
        Ok(self.pos)
    }
}

#[inline]
fn mask(n: u32) -> u64 {
    if n == 0 {
        0
    } else if n >= 64 {
        u64::MAX
    } else {
        (1u64 << n) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_read_back() {
        let mut buf = [0u8; 4];
        let mut w = BitWriter::new(&mut buf);
        w.write(4, 0b1011).unwrap();
        w.write(4, 0b0010).unwrap();
        w.write(8, 0b0101_1110).unwrap();
        assert_eq!(w.finish().unwrap(), 2);
        assert_eq!(buf[0], 0b1011_0010);
        assert_eq!(buf[1], 0b0101_1110);
    }

    #[test]
    fn buffer_full_returns_error() {
        let mut buf = [0u8; 1];
        let mut w = BitWriter::new(&mut buf);
        w.write(8, 0xAA).unwrap();
        assert_eq!(w.write(8, 0xBB), Err(WriteError::BufferFull));
    }

    #[test]
    fn bit_width_too_large() {
        let mut buf = [0u8; 8];
        let mut w = BitWriter::new(&mut buf);
        assert_eq!(w.write(57, 0), Err(WriteError::BitWidthTooLarge(57)));
    }

    #[test]
    fn align_to_byte() {
        let mut buf = [0u8; 4];
        let mut w = BitWriter::new(&mut buf);
        w.write(3, 0b101).unwrap();
        w.align_to_byte();
        assert_eq!(w.finish().unwrap(), 1);
        assert_eq!(buf[0], 0b1010_0000);
    }
}