//! Bit sinks and the small Rice helpers the entropy layer shares.
//!
//! Every syntax writer in this crate is generic over
//! [`tpt_media_bitstream::rice::WriteLike`], so the *same* code path that
//! emits a packet also measures a candidate: the RDO's `R` in
//! `J = D + lambda * R` is the exact number of bits the real writer
//! produces, obtained by running it against a [`BitCounter`] instead of a
//! [`PacketBits`].

use alloc::vec::Vec;

use tpt_media_bitstream::rice::{ReadLike, WriteLike};
use tpt_media_bitstream::{BitstreamError, WriteError};

/// A sink that only counts bits. Used to price a candidate exactly.
#[derive(Default, Debug, Clone, Copy)]
pub(crate) struct BitCounter {
    pub bits: u64,
}

impl WriteLike for BitCounter {
    fn write_bits(&mut self, n: u32, _value: u64) -> Result<(), WriteError> {
        self.bits += u64::from(n);
        Ok(())
    }
}

/// Growable MSB-first bit writer for the packet body.
///
/// The packet is written into a `Vec` rather than a worst-case sized
/// buffer: [`crate::VaultConfig::max_packet_bits`] is a *provable* upper
/// bound, but at 1080p it is tens of megabytes, and a codec that allocates
/// its worst case per stream is not a good citizen. The bound is instead
/// asserted against the finished packet.
#[derive(Debug, Default)]
pub(crate) struct PacketBits {
    bytes: Vec<u8>,
    acc: u64,
    nacc: u32,
    total: u64,
}

impl PacketBits {
    pub fn new() -> Self {
        PacketBits::default()
    }

    /// Total bits written so far.
    pub fn bits(&self) -> u64 {
        self.total
    }

    /// Flush the partial byte (zero padded) and return the bytes.
    pub fn finish(mut self) -> Vec<u8> {
        if self.nacc > 0 {
            let byte = (self.acc << (8 - self.nacc)) as u8;
            self.bytes.push(byte);
        }
        self.bytes
    }
}

impl WriteLike for PacketBits {
    fn write_bits(&mut self, n: u32, value: u64) -> Result<(), WriteError> {
        if n == 0 {
            return Ok(());
        }
        if n > 32 {
            return Err(WriteError::BitWidthTooLarge(n));
        }
        let v = value & ((1u64 << n) - 1);
        self.acc = (self.acc << n) | v;
        self.nacc += n;
        self.total += u64::from(n);
        while self.nacc >= 8 {
            self.nacc -= 8;
            self.bytes.push((self.acc >> self.nacc) as u8);
            self.acc &= (1u64 << self.nacc) - 1;
        }
        Ok(())
    }
}

/// Largest Rice parameter a coefficient block may select (3 bits on the wire).
pub(crate) const RICE_K_MAX: u32 = 7;

/// Bits an unsigned Rice code of `u` at parameter `k` costs.
#[inline]
pub(crate) fn rice_len_unsigned(u: u32, k: u32) -> u32 {
    (u >> k) + 1 + k
}

/// Zig-zag fold a signed value onto an unsigned one (sign in the LSB), the
/// same fold `tpt_media_bitstream::rice` applies internally.
#[inline]
pub(crate) fn fold(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

/// Bits a *signed* Rice code of `v` at parameter `k` costs.
#[inline]
pub(crate) fn rice_len_signed(v: i32, k: u32) -> u32 {
    rice_len_unsigned(fold(v), k)
}

/// Write an unsigned Rice code (unary quotient, then `k` remainder bits).
///
/// `tpt_media_bitstream::rice` exposes only the signed (folded) form, which
/// would spend one wasted bit per zero-run length; run lengths are
/// non-negative, so they use this unsigned form of the same code.
pub(crate) fn write_rice_unsigned<W: WriteLike>(
    w: &mut W,
    k: u32,
    u: u32,
) -> Result<(), WriteError> {
    for _ in 0..(u >> k) {
        w.write_bits(1, 0)?;
    }
    w.write_bits(1, 1)?;
    if k > 0 {
        w.write_bits(k, u64::from(u) & ((1u64 << k) - 1))?;
    }
    Ok(())
}

/// Longest unary quotient the decoder will follow. Far above anything the
/// encoder can emit (`MAX_RICE_BITS`), and it bounds the loop on hostile
/// all-zero input independently of the packet-size limit.
const MAX_UNARY: u32 = 1 << 16;

/// Read an unsigned Rice code written by [`write_rice_unsigned`].
pub(crate) fn read_rice_unsigned<R: ReadLike>(r: &mut R, k: u32) -> Result<u32, BitstreamError> {
    let mut q: u32 = 0;
    while r.read(1)? == 0 {
        q += 1;
        if q > MAX_UNARY {
            return Err(BitstreamError::EndOfInput);
        }
    }
    let rem = if k > 0 { r.read(k)? as u32 } else { 0 };
    Ok((q << k) | rem)
}
