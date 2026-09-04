//! tpt-video-whisper — Tier-3 edge / IoT video codec.
//!
//! Dictionary / vector quantization: encoder and decoder share a
//! pre-trained 4096-entry dictionary of common 8×8 pixel patches.
//! 12-bit patch indices are transmitted. Hardcoded to ≤480p with
//! client-side Lanczos upscaling. Target bitrate: <50 kbps.

#![forbid(unsafe_code)]

pub const CODEC_ID: u8 = 0x21;

pub const DICT_SIZE: usize = 4096;
pub const PATCH_SIZE: usize = 8;
pub const DICT_INDEX_BITS: usize = 12;

pub const MAX_WIDTH: u32 = 854;   // 480p widescreen
pub const MAX_HEIGHT: u32 = 480;

pub struct WhisperEncoder;
pub struct WhisperDecoder;

impl WhisperEncoder { pub fn new() -> Self { Self } }
impl WhisperDecoder { pub fn new() -> Self { Self } }
impl Default for WhisperEncoder { fn default() -> Self { Self::new() } }
impl Default for WhisperDecoder { fn default() -> Self { Self::new() } }