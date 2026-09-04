//! tpt-video-livelink — Tier-1 real-time video codec.
//!
//! I/P-frame only (no B-frames). 5% intra-refresh every frame for
//! graceful packet-loss recovery. Full-pixel SAD motion estimation,
//! 4x4 and 8x8 DCT, simple 3x3 deblocking filter.

#![forbid(unsafe_code)]

pub const CODEC_ID: u8 = 0x20;

pub const INTRA_REFRESH_RATIO: f32 = 0.05;

pub struct LiveLinkEncoder;
pub struct LiveLinkDecoder;

impl LiveLinkEncoder { pub fn new() -> Self { Self } }
impl LiveLinkDecoder { pub fn new() -> Self { Self } }
impl Default for LiveLinkEncoder { fn default() -> Self { Self::new() } }
impl Default for LiveLinkDecoder { fn default() -> Self { Self::new() } }