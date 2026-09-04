//! tpt-core — shared foundation for the tpt-media codec suite.
//!
//! Provides:
//! * Bitstream I/O ([`bitstream`])
//! * Entropy coding: Range Coder and simple Huffman ([`entropy`])
//! * DSP primitives: DCT, MDCT, LPC ([`math`])
//! * `.tpt` container format reader/writer ([`container`])
//!
//! All hot paths are zero-allocation. The default build uses `std`; a
//! `no_std` build is supported via `--no-default-features`.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
extern crate alloc;

pub mod bitstream;
pub mod container;
pub mod entropy;
pub mod math;

pub use bitstream::{BitReader, BitWriter};
pub use container::{Cluster, ClusterFlags, Container, TrackHeader, FILE_HEADER_SIZE};
pub use entropy::{HuffmanCodec, HuffmanTable, RangeDecoder, RangeEncoder, RangeSink};

/// Errors produced by tpt-core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("bitstream buffer exhausted (needed {needed} bits, had {available})")]
    BitstreamOverflow { needed: usize, available: usize },
    #[error("invalid container magic; not a .tpt file")]
    InvalidMagic,
    #[error("unsupported container version: {0}")]
    UnsupportedVersion(u16),
    #[error("malformed {chunk} chunk: {reason}")]
    MalformedChunk { chunk: &'static str, reason: &'static str },
    #[error("invalid Huffman tree: {0}")]
    InvalidHuffman(&'static str),
    #[error("range coder invariant violated: {0}")]
    RangeInvariant(&'static str),
    #[error("data is truncated: needed {needed} bytes, had {available}")]
    Truncated { needed: usize, available: usize },
}

pub type Result<T> = core::result::Result<T, Error>;