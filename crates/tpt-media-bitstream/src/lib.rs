//! `tpt-media-bitstream` — bit-level I/O and entropy coding.
//!
//! Provides:
//!
//! - [`BitReader`]: wraps a `&[u8]`, internal `u64` accumulator,
//!   supports arbitrary-width reads, alignment, peek, skip, EOF.
//! - [`BitWriter`]: wraps a `&mut [u8]`, internal `u64` accumulator,
//!   supports arbitrary-width writes, alignment, flush, bounded.
//! - [`huffman`]: canonical Huffman encoder/decoder.
//! - [`rice`]: Rice/Golomb coder.
//! - [`range`]: static range coder.
//! - [`entropy`]: Shannon entropy, histogram, and distribution helpers.
//!
//! All types in this crate are allocation-free at runtime: they
//! operate on caller-owned buffers.
//!
//! See `docs/architecture.md` §2 for the design rationale.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod bit_reader;
mod bit_writer;
pub mod entropy;
pub mod huffman;
pub mod range;
pub mod rice;

pub use bit_reader::{BitReader, BitstreamError};
pub use bit_writer::{BitWriter, WriteError};

/// §38 "bitstream fuzzing": deterministic, dependency-free corruption
/// sweeps over every decode entry point this crate exposes, in the same
/// spirit as `tpt-container`'s `corruption_never_panics_and_truncation_never_panics`
/// (§24) — a fixed-seed LCG generates arbitrary/corrupted byte buffers
/// and bit widths, and every call is required to return a `Result`
/// (`Ok` or `Err`), never panic. No external fuzz-harness dependency
/// (`cargo-fuzz`/libFuzzer) is used, matching this codebase's established
/// preference for a self-contained, reproducible sweep over a build-time
/// dependency that may not always be available.
#[cfg(test)]
mod fuzz_like {
    use crate::huffman::build_table;
    use crate::range::RangeDecoder;
    use crate::rice::decode_riced;
    use crate::BitReader;
    use alloc::vec::Vec;

    fn lcg_next(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state
    }

    fn random_bytes(state: &mut u64, len: usize) -> Vec<u8> {
        (0..len).map(|_| lcg_next(state) as u8).collect()
    }

    #[test]
    fn bit_reader_never_panics_on_arbitrary_bytes_and_widths() {
        let mut state = 0x243F_6A88_85A3_08D3u64;
        for _ in 0..1000 {
            let len = (lcg_next(&mut state) % 17) as usize;
            let bytes = random_bytes(&mut state, len);
            let mut r = BitReader::new(&bytes);
            for _ in 0..20 {
                let width = 1 + (lcg_next(&mut state) % 56) as u32;
                match lcg_next(&mut state) % 3 {
                    0 => {
                        let _ = r.read(width);
                    }
                    1 => {
                        let _ = r.peek(width);
                    }
                    _ => {
                        let _ = r.skip(width);
                    }
                }
            }
        }
    }

    #[test]
    fn huffman_decode_never_panics_on_arbitrary_bits() {
        // A real (non-degenerate) table, decoded against bits it was
        // never actually used to encode.
        let table = build_table(&[10, 5, 3, 1, 1]);
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..500 {
            let len = 1 + (lcg_next(&mut state) % 16) as usize;
            let bytes = random_bytes(&mut state, len);
            let mut r = BitReader::new(&bytes);
            for _ in 0..8 {
                let _ = table.decode(&mut r);
            }
        }
    }

    #[test]
    fn rice_decode_never_panics_on_arbitrary_bits() {
        let mut state = 0xB529_7A4D_A7A4_3B22u64;
        for _ in 0..500 {
            let len = 1 + (lcg_next(&mut state) % 16) as usize;
            let bytes = random_bytes(&mut state, len);
            let k = (lcg_next(&mut state) % 16) as u32;
            let mut r = BitReader::new(&bytes);
            for _ in 0..8 {
                let _ = decode_riced(&mut r, k);
            }
        }
    }

    #[test]
    fn range_decoder_never_panics_on_arbitrary_bytes() {
        let mut state = 0x1656_67B1_9E37_79F9u64;
        for _ in 0..500 {
            let len = (lcg_next(&mut state) % 32) as usize;
            let bytes = random_bytes(&mut state, len);
            if let Ok(mut dec) = RangeDecoder::new(&bytes) {
                for _ in 0..8 {
                    let _ = dec.decode(256);
                }
            }
        }
    }
}
