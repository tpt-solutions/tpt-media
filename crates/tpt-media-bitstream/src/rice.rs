//! Rice / Golomb coder for Laplacian-distributed integers.
//!
//! Rice coding is the canonical choice for residuals in audio and
//! video codecs because it is allocation-free, deterministic, and
//! asymptotically optimal for geometric distributions. We support
//! parameter `k` from 0 to 16 and signed values via fold (zig-zag).
//!
//! ```text
//! n -> 2n     (n >= 0)
//! n -> 2|n|-1  (n < 0)
//! ```
//!
//! See: Rice, "Some Practical Universal Noiseless Coding Techniques",
//! 1979; and Golomb, "Run-Length Encodings", 1966.

use crate::bit_reader::{BitReader, BitstreamError};
use crate::bit_writer::{BitWriter, WriteError};

/// Encode a signed integer using a Rice code with parameter `k`.
///
/// `k` must be in `0..=16`. The output is written to `w`.
pub fn encode_riced<W: WriteLike>(w: &mut W, k: u32, value: i64) -> Result<(), WriteError> {
    let u = zig_zag_encode(value);
    let q = u >> k;
    let r = u & mask(k);
    // unary: q zeros followed by a one
    for _ in 0..q {
        w.write_bits(1, 0)?;
    }
    w.write_bits(1, 1)?;
    if k > 0 {
        w.write_bits(k, r)?;
    }
    Ok(())
}

/// Decode a signed integer using a Rice code with parameter `k`.
pub fn decode_riced<R: ReadLike>(r: &mut R, k: u32) -> Result<i64, BitstreamError> {
    let mut q: u32 = 0;
    while r.read(1)? == 0 {
        q = q.checked_add(1).ok_or(BitstreamError::EndOfInput)?;
        if q > (1 << 30) {
            return Err(BitstreamError::EndOfInput);
        }
    }
    let rbits = if k == 0 { 0 } else { r.read(k)? };
    let u = (q << k) | rbits as u32;
    Ok(zig_zag_decode(u as u64))
}

#[inline]
fn mask(k: u32) -> u64 {
    if k == 0 {
        0
    } else if k >= 64 {
        u64::MAX
    } else {
        (1u64 << k) - 1
    }
}

#[inline]
fn zig_zag_encode(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

#[inline]
fn zig_zag_decode(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

/// Abstract interface over a bit writer.
pub trait WriteLike {
    /// Write `n` bits of `value`.
    fn write_bits(&mut self, n: u32, value: u64) -> Result<(), WriteError>;
}

impl<'a> WriteLike for BitWriter<'a> {
    fn write_bits(&mut self, n: u32, value: u64) -> Result<(), WriteError> {
        BitWriter::write(self, n, value)
    }
}

/// Abstract interface over a bit reader.
pub trait ReadLike {
    /// Read `n` bits.
    fn read(&mut self, n: u32) -> Result<u64, BitstreamError>;
}

impl<'a> ReadLike for BitReader<'a> {
    fn read(&mut self, n: u32) -> Result<u64, BitstreamError> {
        BitReader::read(self, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zig_zag_round_trip() {
        for v in [-1_000_000i64, -7, -1, 0, 1, 7, 1_000_000] {
            let u = zig_zag_encode(v);
            assert_eq!(zig_zag_decode(u), v);
        }
    }

    #[test]
    fn rice_round_trip() {
        let mut buf = [0u8; 32];
        let values: [i64; 5] = [0, 1, -1, 5, -13];
        let mut w = BitWriter::new(&mut buf);
        for &v in &values {
            encode_riced(&mut w, 3, v).unwrap();
        }
        let n = w.finish().unwrap();
        let mut r = BitReader::new(&buf[..n]);
        for &v in &values {
            let got = decode_riced(&mut r, 3).unwrap();
            assert_eq!(got, v);
        }
    }
}