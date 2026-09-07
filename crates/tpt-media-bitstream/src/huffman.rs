//! Canonical Huffman coder.
//!
//! Given a set of symbol frequencies, [`build_table`] produces a
//! canonical Huffman code suitable for fast encode/decode.

use crate::bit_reader::{BitReader, BitstreamError};
use crate::bit_writer::{BitWriter, WriteError};
use alloc::vec;
use alloc::vec::Vec;

/// One entry in a Huffman table.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct HuffmanEntry {
    /// Symbol index.
    pub symbol: u16,
    /// Code value (MSB-first).
    pub code: u32,
    /// Code length in bits (1..=32).
    pub length: u8,
}

/// A canonical Huffman code table.
#[derive(Clone, Debug, Default)]
pub struct HuffmanTable {
    entries: Vec<HuffmanEntry>,
}

impl HuffmanTable {
    /// Empty table.
    pub fn empty() -> Self {
        HuffmanTable { entries: Vec::new() }
    }

    /// Number of symbols in the table.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` if the table has no symbols.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up an entry by symbol index.
    pub fn get(&self, symbol: u16) -> Option<&HuffmanEntry> {
        self.entries.iter().find(|e| e.symbol == symbol)
    }

    /// Encode a symbol into the bit writer.
    pub fn encode<W: BitWriterLike>(
        &self,
        writer: &mut W,
        symbol: u16,
    ) -> Result<(), WriteError> {
        let entry = self.get(symbol).ok_or(WriteError::BufferFull)?;
        writer.write_bits(entry.length as u32, entry.code as u64)
    }

    /// Decode one symbol from the bit reader.
    pub fn decode<R: BitReaderLike>(
        &self,
        reader: &mut R,
    ) -> Result<u16, BitstreamError> {
        let mut code: u32 = 0;
        for len in 1..=32 {
            let bit = reader.read(1)?;
            code = (code << 1) | bit as u32;
            if let Some(entry) = self.entries.iter().find(|e| e.code == code && e.length as u32 == len) {
                return Ok(entry.symbol);
            }
        }
        Err(BitstreamError::EndOfInput)
    }
}

/// Build a canonical Huffman table from symbol frequencies.
pub fn build_table(freqs: &[u64]) -> HuffmanTable {
    let mut entries: Vec<(u16, u64)> = freqs
        .iter()
        .enumerate()
        .filter_map(|(i, &f)| {
            if f == 0 {
                None
            } else {
                Some((i as u16, f.min(u64::MAX / 2)))
            }
        })
        .collect();

    if entries.is_empty() {
        return HuffmanTable::empty();
    }
    if entries.len() == 1 {
        let (sym, _) = entries[0];
        return HuffmanTable {
            entries: vec![HuffmanEntry {
                symbol: sym,
                code: 0,
                length: 1,
            }],
        };
    }
    if entries.len() == 2 {
        let (s0, _) = entries[0];
        let (s1, _) = entries[1];
        return HuffmanTable {
            entries: vec![
                HuffmanEntry { symbol: s0, code: 0, length: 1 },
                HuffmanEntry { symbol: s1, code: 1, length: 1 },
            ],
        };
    }

    // General case: derive approximate code lengths from frequencies.
    let n = entries.len();
    let max_len = (n as u8).min(32);
    let total: u64 = entries.iter().map(|e| e.1).sum::<u64>().max(1);

    // Compute lengths proportional to 1/freq, clamped.
    let mut symbols: Vec<(u16, u8)> = entries
        .iter()
        .map(|&(s, f)| {
            let l = if f == 0 {
                max_len
            } else {
                let l = (total / f).max(1).ilog2() as u8;
                l.clamp(1, max_len)
            };
            (s, l)
        })
        .collect();

    // Ensure the Kraft inequality (Σ 2^-l <= 1, scaled here to Σ
    // 2^(max_len-l) <= 2^max_len) holds — the frequency-derived estimate
    // above has no such guarantee. Lengthening the *shortest* remaining
    // code roughly halves its contribution to the sum each time, so this
    // always makes progress and is guaranteed to terminate once every
    // code reaches max_len (where the sum is at most n <= 2^max_len).
    while kraft_sum(&symbols, max_len) > (1u64 << max_len) {
        let shortest = symbols
            .iter()
            .enumerate()
            .filter(|(_, (_, l))| *l < max_len)
            .min_by_key(|(_, (_, l))| *l)
            .map(|(i, _)| i);
        match shortest {
            Some(i) => symbols[i].1 += 1,
            None => break,
        }
    }

    // Canonical assignment: sort by length ascending, then by symbol.
    symbols.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));

    let mut codes: Vec<(u16, u32, u8)> = Vec::with_capacity(n);
    let mut code: u32 = 0;
    let mut prev_len: u8 = 0;
    for (i, &(sym, len)) in symbols.iter().enumerate() {
        if i == 0 {
            code = 0;
        } else {
            code = (code + 1) << (len as u32 - prev_len as u32);
        }
        codes.push((sym, code, len));
        prev_len = len;
    }

    HuffmanTable {
        entries: codes
            .into_iter()
            .map(|(sym, code, length)| HuffmanEntry { symbol: sym, code, length })
            .collect(),
    }
}

fn kraft_sum(symbols: &[(u16, u8)], max_len: u8) -> u64 {
    symbols.iter().map(|&(_, l)| 1u64 << (max_len - l)).sum()
}

/// Abstract interface for bit writers, to avoid the orphan-rule dance.
pub trait BitWriterLike {
    /// Write `n` bits of `value`.
    fn write_bits(&mut self, n: u32, value: u64) -> Result<(), WriteError>;
}

impl<'a> BitWriterLike for BitWriter<'a> {
    fn write_bits(&mut self, n: u32, value: u64) -> Result<(), WriteError> {
        BitWriter::write(self, n, value)
    }
}

/// Abstract interface for bit readers.
pub trait BitReaderLike {
    /// Read `n` bits MSB-first.
    fn read(&mut self, n: u32) -> Result<u64, BitstreamError>;
}

impl<'a> BitReaderLike for BitReader<'a> {
    fn read(&mut self, n: u32) -> Result<u64, BitstreamError> {
        BitReader::read(self, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_two_symbol_table() {
        let t = build_table(&[1, 1]);
        assert_eq!(t.len(), 2);
        assert_eq!(t.get(0).unwrap().length, 1);
        assert_eq!(t.get(1).unwrap().length, 1);
    }

    #[test]
    fn build_skewed_table() {
        let t = build_table(&[99, 1]);
        let e0 = t.get(0).unwrap();
        let e1 = t.get(1).unwrap();
        assert!(e0.length <= e1.length);
    }

    #[test]
    fn encode_decode_round_trip() {
        let freqs = [50u64, 25, 12, 6, 3, 2, 1, 1];
        let table = build_table(&freqs);
        let mut buf = [0u8; 64];
        let mut w = BitWriter::new(&mut buf);
        let symbols = [0u16, 1, 2, 3, 0, 1, 0, 4, 5, 6, 7];
        for &s in &symbols {
            table.encode(&mut w, s).unwrap();
        }
        let n = w.finish().unwrap();
        let mut r = BitReader::new(&buf[..n]);
        for &s in &symbols {
            let got = table.decode(&mut r).unwrap();
            assert_eq!(got, s);
        }
    }
}