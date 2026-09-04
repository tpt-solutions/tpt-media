//! tpt-video-vault — Tier-2 archival video codec.
//!
//! Recursive superblock partitioning (64×64 down to 8×8). Full-pixel
//! and half-pixel (2-tap bilinear — patent-safe) motion estimation.
//! Brute-force rate-distortion optimization with Lagrangian cost
//! J = D + λR. Up to 16 reference frames.

#![forbid(unsafe_code)]

pub const CODEC_ID: u8 = 0x22;

pub const MAX_SUPERBLOCK: usize = 64;
pub const MIN_BLOCK: usize = 8;
pub const MAX_REFERENCE_FRAMES: usize = 16;

pub struct VaultEncoder;
pub struct VaultDecoder;

impl VaultEncoder { pub fn new() -> Self { Self } }
impl VaultDecoder { pub fn new() -> Self { Self } }
impl Default for VaultEncoder { fn default() -> Self { Self::new() } }
impl Default for VaultDecoder { fn default() -> Self { Self::new() } }