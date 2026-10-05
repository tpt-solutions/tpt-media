//! `tpt-media-io` — media I/O for the `tpt-media` stack.
//!
//! Layers (each covered in detail in `docs/media-io-design.md`):
//!
//! - [`pcm`] — PCM integration: interleaved raw-sample bytes ↔
//!   [`tpt_media_core::AudioFrame`], chunked decoding with monotonic PTS, and the
//!   `f32` `[-1, 1]` normalization/denormalization pair.
//! - [`wav`] — WAV: byte-exact round trips for the codec pipeline plus a
//!   normalized-`f32` bridge that reuses [`tpt_dsp_io`] ('reuse, don't
//!   duplicate', §27).
//! - [`raster`] — image/video frame I/O: dependency-free PPM P6 / PGM P5
//!   readers and writers over [`tpt_media_core::VideoFrame`], plus fixture generators.
//! - [`source`] — streaming sources: the [`FrameSource`] pull trait and
//!   concrete PCM chunk readers over slices and (with `std`) files/stdin.
//! - [`sink`] — streaming sinks: the [`FrameSink`] push trait and
//!   concrete writers over vectors and (with `std`) files/stdout.
//! - [`async_io`] (`std`) — async integration: [`AsyncFrameSource`] /
//!   [`AsyncFrameSink`] traits and a synchronous inline adapter.
//!
//! Dependency picture: `tpt-media-io` depends on `tpt-media-core`
//! (always) and `tpt-dsp-io` (behind the `std` feature, which is the
//! default). It deliberately does **not** depend on `tpt-media-pipeline`:
//! the pipeline consumes these sources/sinks, never the other way round.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::string::String;
use core::fmt;

/// I/O error type.
#[derive(Debug)]
pub enum IoError {
    /// The input ended before a header/chunk was complete.
    UnexpectedEof,
    /// A container/file header was syntactically invalid.
    InvalidHeader,
    /// A well-formed structure uses a feature we deliberately reject.
    Unsupported(&'static str),
    /// An integer overflow occurred while computing sizes/offsets.
    Overflow,
    /// Any other I/O problem, with a message.
    Other(String),
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoError::UnexpectedEof => f.write_str("unexpected end of input"),
            IoError::InvalidHeader => f.write_str("invalid header"),
            IoError::Unsupported(s) => write!(f, "unsupported: {}", s),
            IoError::Overflow => f.write_str("integer overflow"),
            IoError::Other(s) => f.write_str(s),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for IoError {}

/// Result alias.
pub type IoResult<T> = Result<T, IoError>;

pub mod pcm;
pub mod raster;
pub mod sink;
pub mod source;
pub mod wav;

#[cfg(feature = "std")]
pub mod async_io;

pub use pcm::{decode_pcm, denormalize_from_f32, encode_pcm, normalize_to_f32, PcmSpec};
pub use raster::{checkerboard, read_raster, write_raster, RasterFormat};
pub use sink::{FrameSink, VecMediaSink};
pub use source::{FrameSource, MediaItem, SlicePcmReader};
pub use wav::{decode_wav, parse_wav, sine_wave, write_wav, WavFormat};

#[cfg(feature = "std")]
pub use sink::{FilePcmSink, StdoutPcmSink};
#[cfg(feature = "std")]
pub use source::{FilePcmReader, StdinPcmReader};

#[cfg(feature = "std")]
pub use async_io::{AsyncFrameSink, AsyncFrameSource, BlockingAdapter};

#[cfg(feature = "std")]
pub use raster::{read_raster_path, write_raster_path};
#[cfg(feature = "std")]
pub use wav::{decode_wav_f32, write_wav_f32};
