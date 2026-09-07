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
//!
//! All types in this crate are allocation-free at runtime: they
//! operate on caller-owned buffers.
//!
//! See [`docs/architecture.md`] §2 for the design rationale.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod bit_reader;
mod bit_writer;
pub mod huffman;
pub mod rice;
pub mod range;

pub use bit_reader::{BitReader, BitstreamError};
pub use bit_writer::{BitWriter, WriteError};