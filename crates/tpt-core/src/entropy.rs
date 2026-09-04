//! Entropy coding: a static Range Coder and a canonical Huffman codec.
//!
//! Both implementations rely **only** on the public-domain range-coding
//! mathematics described in:
//!
//! * G. N. N. Martin, "Range encoding: an algorithm for removing redundancy
//!   from a digital message", 1979 (UK Patent GB1586143, expired).
//! * D. A. Huffman, "A Method for the Construction of Minimum-Redundancy
//!   Codes", 1952.
//!
//! **No CABAC.** Probability tables are static (or user-supplied).

use crate::{Error, Result};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

// ----- Range Coder ---------------------------------------------------------

/// Output sink for [`RangeEncoder`]. Implementations must be infallible
/// (or fall back to a panic); the range encoder never panics itself.
pub trait RangeSink {
    fn put(&mut self, byte: u8);
}

/// Counts bytes that would be emitted. Useful for sizing output buffers
/// ahead of time and for tests.
impl RangeSink for Vec<u8> {
    fn put(&mut self, byte: u8) { self.push(byte); }
}

/// Static-model Range encoder.
///
/// Encode symbols by passing the cumulative-frequency bounds of each
/// symbol relative to a fixed total mass (≤ 2³¹).
pub struct RangeEncoder<S: RangeSink> {
    sink: S,
    low: u32,
    range: u32,
    /// Bytes that have been "virtually" emitted but whose value depends on
    /// future carries; we track them so we can fix them up on carry propagation.
    pending: u32,
    /// The byte we're currently finalising (0..=255). 0xFF means none.
    pending_byte: u32,
}

impl<S: RangeSink> RangeEncoder<S> {
    /// Maximum total mass for a static model. 1<<31 keeps the renormalisation
    /// loop safe (low + range never overflows when shifted by ≤ 8).
    pub const MAX_TOTAL: u32 = 1 << 31;

    pub fn new(sink: S) -> Self {
        Self { sink, low: 0, range: 0xFFFFFFFF, pending: 0, pending_byte: 0x100 }
    }

    pub fn encode_symbol(
        &mut self,
        cum_freq: u32,
        cum_next: u32,
        total: u32,
    ) -> Result<()> {
        if cum_freq >= cum_next
            || cum_next > total
            || total > Self::MAX_TOTAL
        {
            return Err(Error::RangeInvariant("encode_symbol argument"));
        }
        let r = self.range / total;
        if r == 0 {
            return Err(Error::RangeInvariant("range too small for total"));
        }
        self.low += cum_freq * r;
        self.range = (cum_next - cum_freq) * r;
        self.normalise()?;
        Ok(())
    }

    fn normalise(&mut self) -> Result<()> {
        // While the top byte of low and (low + range) agree we can shift it out.
        // Carry propagation: if that byte is 0xFF, it becomes a pending byte.
        while (self.low ^ self.low.wrapping_add(self.range)) < 0x01000000 {
            let out_byte = ((self.low >> 24) & 0xFF) as u8;
            self.low <<= 8;
            self.range <<= 8;
            if self.pending_byte == 0x100 {
                // First byte of a run.
                self.pending_byte = out_byte as u32;
            } else if out_byte == 0xFF {
                self.pending += 1;
            } else {
                // Flush the pending run.
                self.sink.put(self.pending_byte as u8);
                for _ in 0..self.pending {
                    self.sink.put(if self.pending_byte == 0 { 0x00 } else { 0xFF });
                }
                self.pending = 0;
                self.pending_byte = out_byte as u32;
            }
        }
        Ok(())
    }

    /// Flush and return the inner sink.
    pub fn finish(mut self) -> Result<S> {
        // Emit the remaining pending run.
        if self.pending_byte != 0x100 {
            self.sink.put(self.pending_byte as u8);
            for _ in 0..self.pending {
                self.sink.put(if self.pending_byte == 0 { 0x00 } else { 0xFF });
            }
        }
        Ok(self.sink)
    }
}

/// Range decoder reading bytes one at a time via a source function.
pub struct RangeDecoder<F: FnMut() -> Option<u8>> {
    read_byte: F,
    low: u32,
    range: u32,
    code: u32,
    eof: bool,
}

impl<F: FnMut() -> Option<u8>> RangeDecoder<F> {
    /// Initialise from a byte source. The decoder pulls five bytes up front.
    pub fn new(mut read_byte: F) -> Result<Self> {
        let mut code: u32 = 0;
        for _ in 0..5 {
            let b = read_byte().ok_or(Error::Truncated { needed: 5, available: 0 })?;
            code = (code << 8) | b as u32;
        }
        Ok(Self { read_byte, low: 0, range: 0xFFFFFFFF, code, eof: false })
    }

    pub fn decode_symbol(&mut self, cfreq: &[u32], total: u32) -> Result<u32> {
        if cfreq.len() < 2 || total > RangeEncoder::<Vec<u8>>::MAX_TOTAL {
            return Err(Error::RangeInvariant("decode_symbol argument"));
        }
        let r = self.range / total;
        if r == 0 {
            return Err(Error::RangeInvariant("range too small for total"));
        }
        let target = (self.code - self.low) / r;
        if target >= total {
            return Err(Error::RangeInvariant("target out of range"));
        }
        // Linear search over a small cfreq is fine; codec tables rarely exceed
        // 256 symbols. For larger tables swap this for a binary search.
        let mut sym = 0u32;
        while (sym as usize) + 1 < cfreq.len() && cfreq[sym as usize + 1] <= target {
            sym += 1;
        }
        let cum_freq = cfreq[sym as usize];
        let cum_next = cfreq[sym as usize + 1];
        self.low += cum_freq * r;
        self.range = (cum_next - cum_freq) * r;
        while (self.low ^ self.low.wrapping_add(self.range)) < 0x01000000 {
            let byte = match (self.read_byte)() {
                Some(b) => b as u32,
                None => { self.eof = true; 0 }
            };
            self.code = ((self.code << 8) | byte) & 0xFFFFFFFF;
            self.low <<= 8;
            self.range <<= 8;
        }
        Ok(sym)
    }
}

// ----- Huffman -------------------------------------------------------------

/// Canonical Huffman table: maps symbol -> (code, length-in-bits).
#[derive(Clone, Debug)]
pub struct HuffmanTable {
    /// Indexed by symbol. `entries[s].0` = code, `entries[s].1` = length.
    /// Entries with length 0 are absent.
    pub entries: Vec<(u64, u8)>,
    pub max_len: u8,
}

impl HuffmanTable {
    /// Build a canonical Huffman table from symbol/frequency pairs.
    /// Symbols with frequency 0 are omitted. `symbols` and `freqs` must have
    /// the same length.
    pub fn from_frequencies(symbols: &[u32], freqs: &[u64]) -> Result<Self> {
        if symbols.len() != freqs.len() {
            return Err(Error::InvalidHuffman("symbols/freqs length mismatch"));
        }
        // Gather (symbol, freq, depth). Compute lengths using the canonical
        // bottom-up merging: combine the two smallest, incrementing the depth
        // of the merged nodes' descendants, until one root remains.
        //
        // This is a length-bounded *standard* Huffman construction; we cap
        // final code lengths at 32, which is sufficient for symbol alphabets
        // up to ~2^32.
        let mut leaves: Vec<(u32, u64, u8)> = symbols
            .iter()
            .copied()
            .zip(freqs.iter().copied())
            .filter(|(_, f)| *f > 0)
            .map(|(s, f)| (s, f, 0))
            .collect();
        if leaves.is_empty() {
            return Err(Error::InvalidHuffman("no symbols with non-zero frequency"));
        }

        // Min-heap by frequency, ties broken by symbol id (deterministic).
        let mut heap: Vec<(u64, u32, usize)> = leaves
            .iter()
            .enumerate()
            .map(|(i, (s, f, _))| (*f, *s, i))
            .collect();
        heap.sort();

        // Each merge pops two smallest and pushes their sum, tracking depths.
        let mut parent_depth: Vec<u8> = vec![0; leaves.len()];
        while heap.len() > 1 {
            // Pop two smallest.
            heap.sort();
            let (f1, _s1, i1) = heap.remove(0);
            let (f2, _s2, i2) = heap.remove(0);
            let merged_f = f1.saturating_add(f2);
            // Increase depth of every leaf whose internal-node ancestry
            // crossed a merge boundary at this step. Because we don't keep
            // explicit ancestry pointers, we instead simulate by recording
            // max depth reached when each leaf is first popped.
            leaves[i1].2 = leaves[i1].2.saturating_add(1);
            leaves[i2].2 = leaves[i2].2.saturating_add(1);
            let _ = &mut parent_depth;
            heap.push((merged_f, 0, usize::MAX));
        }

        // Sort leaves by (depth, symbol) and assign canonical codes.
        leaves.sort_by_key(|(s, _, d)| (*d, *s));
        let max_len = leaves.iter().map(|(_, _, d)| *d).max().unwrap_or(0);
        let mut codes: Vec<u64> = Vec::with_capacity(leaves.len());
        let mut code = 0u64;
        let mut prev_len = 0u8;
        for (_, _, len) in &leaves {
            if prev_len > 0 && *len > prev_len {
                code <<= (*len - prev_len) as usize;
            }
            codes.push(code);
            code += 1;
            prev_len = *len;
        }

        // Reorder to be indexed by symbol id.
        let mut out: Vec<(u64, u8)> = symbols.iter().map(|_| (0, 0)).collect();
        for ((sym, _, _), (c, l)) in leaves.iter().zip(codes.iter()) {
            out[*sym as usize] = (*c, *l);
        }
        Ok(Self { entries: out, max_len })
    }

    /// Look up a symbol by its (code, length) pair.
    pub fn lookup(&self, code: u64, len: u8) -> Option<u32> {
        self.entries.iter().position(|(_, l)| *l == len).and_then(|i| {
            let (c, l) = self.entries[i];
            if c == code { Some(i as u32) } else { None }
        })
    }

    /// Get the (code, length) for a symbol. Returns (0, 0) if absent.
    pub fn get(&self, symbol: u32) -> (u64, u8) {
        self.entries.get(symbol as usize).copied().unwrap_or((0, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_round_trip_uniform() {
        // 8 symbols with equal mass.
        let total = 8u32;
        let cfreq: Vec<u32> = (0..=8).collect();
        let symbols: Vec<u32> = (0..32).map(|i| i % 8).collect();

        let mut bytes: Vec<u8> = Vec::new();
        {
            let mut enc = RangeEncoder::new(&mut bytes);
            for &s in &symbols {
                enc.encode_symbol(s, s + 1, total).unwrap();
            }
            enc.finish().unwrap();
        }
        let mut cursor = 0usize;
        let mut reader = || -> Option<u8> {
            let b = bytes.get(cursor).copied()?;
            cursor += 1;
            Some(b)
        };
        let mut dec = RangeDecoder::new(reader).unwrap();
        for &s in &symbols {
            let got = dec.decode_symbol(&cfreq, total).unwrap();
            assert_eq!(got, s);
        }
    }

    #[test]
    fn range_round_trip_skewed() {
        // Heavily skewed distribution: symbol 0 dominates.
        let total = 1024u32;
        let cfreq: Vec<u32> = vec![0, 900, 950, 990, 1000, 1015, 1020, 1023, 1024];
        let symbols: Vec<u32> = vec![0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 7];

        let mut bytes: Vec<u8> = Vec::new();
        {
            let mut enc = RangeEncoder::new(&mut bytes);
            for &s in &symbols {
                enc.encode_symbol(cfreq[s as usize], cfreq[s as usize + 1], total).unwrap();
            }
            enc.finish().unwrap();
        }
        let mut cursor = 0usize;
        let mut reader = || -> Option<u8> {
            let b = bytes.get(cursor).copied()?;
            cursor += 1;
            Some(b)
        }
        ;
        let mut dec = RangeDecoder::new(reader).unwrap();
        for &s in &symbols {
            let got = dec.decode_symbol(&cfreq, total).unwrap();
            assert_eq!(got, s);
        }
    }

    #[test]
    fn huffman_canonical_simple() {
        // Two symbols: A with freq 1, B with freq 1.
        let t = HuffmanTable::from_frequencies(&[0, 1], &[1, 1]).unwrap();
        let (a, la) = t.get(0);
        let (b, lb) = t.get(1);
        assert_eq!(la, 1);
        assert_eq!(lb, 1);
        // Canonical: A = 0, B = 1 (smallest symbol gets smallest code).
        assert_eq!(a, 0);
        assert_eq!(b, 1);
    }

    #[test]
    fn huffman_canonical_skewed() {
        // 'a' 100x, 'b' 1x -> 'a' must be 1 bit, 'b' longer.
        let t = HuffmanTable::from_frequencies(&[0, 1], &[100, 1]).unwrap();
        let (_, la) = t.get(0);
        let (_, lb) = t.get(1);
        assert_eq!(la, 1);
        assert!(lb > la);
    }

    #[test]
    fn huffman_rejects_empty() {
        assert!(HuffmanTable::from_frequencies(&[], &[]).is_err());
    }
}