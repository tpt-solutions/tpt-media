//! Static (non-adaptive) range coder.
//!
//! Range coding is mathematically equivalent to arithmetic coding but
//! is simpler to implement and verify. This is a **static** coder: the
//! model (`cum`, `freq`, `total`) is supplied per symbol; there is no
//! context adaptation, no CABAC.
//!
//! This is the standard byte-oriented range coder design used by LZMA and
//! many other codecs. Encode-side carry propagation is handled by
//! buffering one "cache" byte plus a run-length of pending `0xFF` bytes
//! (see [`RangeEncoder::shift_low`]): a later carry can only ever need to
//! turn a run of trailing `0xFF`s into `0x00`s and increment the byte
//! before them, so nothing already fully-determined ever has to be
//! rewritten.

use crate::bit_reader::BitstreamError;
use crate::bit_writer::WriteError;

/// Renormalization threshold: while `range` is smaller than this, a byte
/// must be shifted out (encoder) or in (decoder) to keep enough
/// precision. Callers must keep `total` (the probability model's
/// normalizing constant, in `encode`/`decode`) well below this.
const TOP: u32 = 1 << 24;
/// Number of trailing [`RangeEncoder::shift_low`] flushes `finish` needs
/// to drain the cache and guarantee the decoder can read a full 32-bit
/// code word (1 cache byte + 4 code bytes).
const FLUSH_SHIFTS: usize = 5;

/// Encode symbols under a static probability model.
pub struct RangeEncoder<'a> {
    out: &'a mut [u8],
    pos: usize,
    /// Bits 0..32 are the coder's actual `low` value; bit 32 records a
    /// carry out of that range, consumed (and cleared) by `shift_low`.
    low: u64,
    range: u32,
    /// The most recently shifted-out byte, held back until we know
    /// whether a future carry needs to increment it.
    cache: u8,
    /// Number of bytes (including `cache`) awaiting output: `cache`
    /// followed by `cache_size - 1` bytes of `0xFF` (or `0x00` if a
    /// carry lands on them).
    cache_size: u64,
}

impl<'a> RangeEncoder<'a> {
    /// Create a new encoder over the given output buffer.
    pub fn new(out: &'a mut [u8]) -> Self {
        RangeEncoder {
            out,
            pos: 0,
            low: 0,
            range: 0xFFFF_FFFF,
            cache: 0,
            cache_size: 1,
        }
    }

    /// Number of bytes written so far.
    #[inline]
    pub fn bytes_written(&self) -> usize {
        self.pos
    }

    fn write_byte(&mut self, b: u8) -> Result<(), WriteError> {
        if self.pos >= self.out.len() {
            return Err(WriteError::BufferFull);
        }
        self.out[self.pos] = b;
        self.pos += 1;
        Ok(())
    }

    /// Encode one symbol whose cumulative-frequency range is
    /// `[cum, cum+freq)` in a model of size `total`.
    pub fn encode(&mut self, cum: u32, freq: u32, total: u32) -> Result<(), WriteError> {
        if total == 0 || freq == 0 || cum + freq > total {
            return Err(WriteError::BufferFull);
        }
        let r = self.range / total;
        self.low += u64::from(cum) * u64::from(r);
        self.range = r * freq;
        while self.range < TOP {
            self.range <<= 8;
            self.shift_low()?;
        }
        Ok(())
    }

    /// Shift the top byte of `low` out to `cache`/the output, propagating
    /// any pending carry (bit 32 of `low`) into the buffered run first.
    ///
    /// A byte can be finalized and written as soon as we know no later
    /// carry can reach it — which is true once the *current* top byte of
    /// `low` isn't `0xFF` (so even a future +1 carry stops there) or a
    /// carry has already happened (`low >> 32 != 0`, which must be
    /// applied now). Until then, bytes accumulate in the pending run as
    /// `0xFF` place-holders that a later carry would turn into `0x00`.
    fn shift_low(&mut self) -> Result<(), WriteError> {
        if (self.low as u32) < 0xFF00_0000 || (self.low >> 32) != 0 {
            let carry = (self.low >> 32) as u8;
            let mut out_byte = self.cache;
            loop {
                self.write_byte(out_byte.wrapping_add(carry))?;
                out_byte = 0xFF;
                self.cache_size -= 1;
                if self.cache_size == 0 {
                    break;
                }
            }
            self.cache = (self.low >> 24) as u8;
        }
        self.cache_size += 1;
        // Truncating shift: the top byte just consumed above must not
        // reappear in the new `low` — shift within `u32` width (which
        // drops it) before widening back to `u64`.
        self.low = ((self.low as u32) << 8) as u64;
        Ok(())
    }

    /// Flush the final bytes and return the number written.
    pub fn finish(mut self) -> Result<usize, WriteError> {
        for _ in 0..FLUSH_SHIFTS {
            self.shift_low()?;
        }
        Ok(self.pos)
    }
}

/// Decode symbols under a static probability model.
pub struct RangeDecoder<'a> {
    input: &'a [u8],
    pos: usize,
    range: u32,
    code: u32,
}

impl<'a> RangeDecoder<'a> {
    /// Create a new decoder over the given input buffer.
    pub fn new(input: &'a [u8]) -> Result<Self, BitstreamError> {
        // The encoder's first flushed byte is always its initial dummy
        // `cache` (0, discarded here); the next 4 form the initial code.
        if input.len() < 5 {
            return Err(BitstreamError::EndOfInput);
        }
        let code = u32::from_be_bytes([input[1], input[2], input[3], input[4]]);
        Ok(RangeDecoder {
            input,
            pos: 5,
            range: 0xFFFF_FFFF,
            code,
        })
    }

    /// Decode one symbol, returning its cumulative frequency (the caller
    /// maps that back to a symbol using the same model given to `encode`).
    pub fn decode(&mut self, total: u32) -> Result<u32, BitstreamError> {
        if total == 0 {
            return Err(BitstreamError::EndOfInput);
        }
        let r = self.range / total;
        let mut cum = self.code / r;
        if cum >= total {
            cum = total - 1;
        }
        self.code -= cum * r;
        self.range = r;
        while self.range < TOP {
            if self.pos >= self.input.len() {
                return Err(BitstreamError::EndOfInput);
            }
            self.code = (self.code << 8) | self.input[self.pos] as u32;
            self.pos += 1;
            self.range <<= 8;
        }
        Ok(cum)
    }

    /// Number of bytes consumed.
    pub fn bytes_read(&self) -> usize {
        self.pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_uniform() {
        let mut buf = [0u8; 64];
        let mut enc = RangeEncoder::new(&mut buf);
        let symbols = [0u32, 1, 2, 3, 0, 1, 2, 3];
        for &s in &symbols {
            enc.encode(s, 1, 4).unwrap();
        }
        let n = enc.finish().unwrap();
        let mut dec = RangeDecoder::new(&buf[..n]).unwrap();
        for &s in &symbols {
            let cum = dec.decode(4).unwrap();
            assert_eq!(cum, s);
        }
    }

    #[test]
    fn encode_decode_skewed() {
        let mut buf = [0u8; 64];
        let mut enc = RangeEncoder::new(&mut buf);
        let symbols = [0u32, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        for &s in &symbols {
            enc.encode(if s == 0 { 0 } else { 100 }, 1, 101).unwrap();
        }
        let n = enc.finish().unwrap();
        let mut dec = RangeDecoder::new(&buf[..n]).unwrap();
        for &s in &symbols {
            let cum = dec.decode(101).unwrap();
            let got = if cum < 100 { 0u32 } else { 1u32 };
            assert_eq!(got, s);
        }
    }

    #[test]
    fn encode_decode_forces_carry_propagation() {
        // A long run of near-maximum cumulative values pushes `low`
        // arbitrarily close to its ceiling, so finishing the stream all
        // but guarantees at least one carry out of the pending `0xFF`
        // run — exactly the case a `low: u32` (no carry tracking) drops.
        let mut buf = [0u8; 256];
        let mut enc = RangeEncoder::new(&mut buf);
        let symbols = [999u32; 64];
        for &s in &symbols {
            enc.encode(s, 1, 1000).unwrap();
        }
        let n = enc.finish().unwrap();
        let mut dec = RangeDecoder::new(&buf[..n]).unwrap();
        for &s in &symbols {
            assert_eq!(dec.decode(1000).unwrap(), s);
        }
    }

    #[test]
    fn decode_rejects_short_input() {
        assert!(matches!(RangeDecoder::new(&[0u8; 4]), Err(BitstreamError::EndOfInput)));
    }
}
