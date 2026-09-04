//! Bit-oriented read/write over byte slices.
//!
//! * [`BitWriter`] — writes bits LSB-first into a `&mut [u8]` with a single
//!   `u64` accumulator. Zero heap allocation.
//! * [`BitReader`] — reads bits LSB-first from a `&[u8]` with a single
//!   `u64` accumulator.
//!
//! Both operate LSB-first within each byte to match the layout used by the
//! range coder and Huffman tables throughout `tpt-media`.

use crate::{Error, Result};

/// Writes bits LSB-first into a backing byte buffer.
///
/// Internally accumulates up to 64 bits in `accum`; whenever `count >= 8`
/// the low byte is flushed to the buffer. No heap allocation.
pub struct BitWriter<'a> {
    buf: &'a mut [u8],
    /// Number of bytes already flushed to `buf`.
    flushed: usize,
    /// Number of valid bits currently buffered in `accum`.
    count: u32,
    /// Bit accumulator. Buffered bits live in the low `count` bits.
    accum: u64,
}

impl<'a> BitWriter<'a> {
    /// Wrap a byte slice. Returns [`Error::BitstreamOverflow`] if the slice
    /// runs out of room before `finish()` is called.
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, flushed: 0, count: 0, accum: 0 }
    }

    /// Number of bytes already written to the backing buffer.
    #[inline]
    pub fn bytes_written(&self) -> usize {
        self.flushed
    }

    /// Number of bits still buffered but not yet flushed.
    #[inline]
    pub fn buffered_bits(&self) -> u32 {
        self.count
    }

    /// Write the `n` low-order bits of `value`, LSB-first. `n` must be ≤ 64.
    pub fn write_bits(&mut self, n: u32, value: u64) -> Result<()> {
        if n > 64 {
            return Err(Error::BitstreamOverflow { needed: n as usize, available: 64 });
        }
        let mask = if n == 64 { u64::MAX } else { (1u64 << n) - 1 };
        let v = value & mask;
        self.accum |= v << self.count;
        self.count += n;
        while self.count >= 8 {
            if self.flushed >= self.buf.len() {
                return Err(Error::BitstreamOverflow { needed: 1, available: 0 });
            }
            let byte = (self.accum & 0xFF) as u8;
            self.accum >>= 8;
            self.count -= 8;
            self.buf[self.flushed] = byte;
            self.flushed += 1;
        }
        Ok(())
    }

    /// Write a single bit.
    #[inline]
    pub fn write_bit(&mut self, bit: bool) -> Result<()> {
        self.write_bits(1, bit as u64)
    }

    /// Flush any remaining partial byte (zero-padding the high bits) and
    /// return the total number of bytes written.
    pub fn finish(mut self) -> Result<usize> {
        if self.count > 0 {
            if self.flushed >= self.buf.len() {
                return Err(Error::BitstreamOverflow { needed: 1, available: 0 });
            }
            self.buf[self.flushed] = self.accum as u8;
            self.flushed += 1;
        }
        Ok(self.flushed)
    }
}

/// Reads bits LSB-first from a byte slice.
pub struct BitReader<'a> {
    buf: &'a [u8],
    /// Index of the next byte to be consumed.
    next_byte: usize,
    /// Number of valid bits currently in `accum`.
    count: u32,
    /// Bit accumulator. Valid bits live in the low `count` bits.
    accum: u64,
}

impl<'a> BitReader<'a> {
    /// Wrap a byte slice.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, next_byte: 0, count: 0, accum: 0 }
    }

    /// Number of bits remaining to be read.
    #[inline]
    pub fn bits_remaining(&self) -> usize {
        (self.buf.len().saturating_sub(self.next_byte)) * 8 + self.count as usize
    }

    /// Read `n` low-order bits, LSB-first. `n` must be ≤ 64.
    pub fn read_bits(&mut self, n: u32) -> Result<u64> {
        if n > 64 {
            return Err(Error::BitstreamOverflow { needed: n as usize, available: 64 });
        }
        while self.count < n {
            if self.next_byte >= self.buf.len() {
                return Err(Error::BitstreamOverflow {
                    needed: (n - self.count) as usize,
                    available: 0,
                });
            }
            self.accum |= (self.buf[self.next_byte] as u64) << self.count;
            self.next_byte += 1;
            self.count += 8;
        }
        let mask = if n == 64 { u64::MAX } else { (1u64 << n) - 1 };
        let v = self.accum & mask;
        self.accum >>= n;
        self.count -= n;
        Ok(v)
    }

    /// Read a single bit.
    #[inline]
    pub fn read_bit(&mut self) -> Result<bool> {
        Ok(self.read_bits(1)? != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_arbitrary_bits() {
        let pattern: [(u32, u64); 6] = [
            (3, 0b101),
            (5, 0b11010),
            (1, 1),
            (16, 0xABCD),
            (8, 0xFF),
            (32, 0xDEADBEEF),
        ];
        let mut buf = [0u8; 64];
        let mut w = BitWriter::new(&mut buf);
        let mut total_bits = 0u32;
        for (n, v) in pattern {
            w.write_bits(n, v).unwrap();
            total_bits += n;
        }
        let nbytes = w.finish().unwrap();
        assert_eq!(nbytes, total_bits.div_ceil(8) as usize);

        let mut r = BitReader::new(&buf[..nbytes]);
        for (n, v) in pattern {
            assert_eq!(r.read_bits(n).unwrap(), v);
        }
    }

    #[test]
    fn overflow_writer_errors() {
        let mut buf = [0u8; 2];
        let mut w = BitWriter::new(&mut buf);
        w.write_bits(16, 0xFFFF).unwrap();
        // Buffer full; next bit must error.
        assert!(w.write_bit(true).is_err());
    }

    #[test]
    fn overflow_reader_errors() {
        let buf = [0xAAu8, 0x55];
        let mut r = BitReader::new(&buf);
        // 16 bits readable.
        r.read_bits(16).unwrap();
        // No more data.
        assert!(r.read_bit().is_err());
    }

    #[test]
    fn round_trip_single_bit_pattern() {
        // 0b1010 1010 pattern (16 bits).
        let mut buf = [0u8; 2];
        {
                let mut w = BitWriter::new(&mut buf);
                for i in 0..16 {
                        w.write_bit(i % 2 == 0).unwrap();
                }
                w.finish().unwrap();
            }
            assert_eq!(buf, [0xAA, 0xAA]);
            let mut r = BitReader::new(&buf);
            for i in 0..16 {
                assert_eq!(r.read_bit().unwrap(), i % 2 == 0);
            }
    }

    #[test]
    fn partial_final_byte() {
        let mut buf = [0u8; 4];
        let written = {
                let mut w = BitWriter::new(&mut buf);
                w.write_bits(11, 0b1_0101_0101_0).unwrap();
                w.finish().unwrap()
            };
            // 11 bits -> 2 bytes.
            assert_eq!(written, 2);
            // First byte: bits 0..=7 (LSB-first) = 0b01010101 = 0x55.
            // Second byte: bits 8..=10 of input = 0b101, then 5 zero-pad bits.
            assert_eq!(buf[0], 0x55);
            assert_eq!(buf[1], 0b0000_0101);
    }
}