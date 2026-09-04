//! tpt-audio-vox — Tier-3 edge / IoT speech codec.
//!
//! CELP / pure-LPC, hardcoded to 8 kHz or 16 kHz, mono only.
//! `#![no_std]`-friendly with fixed-point i16/i32 math (no FPU).
//! Residual is matched to a 1024-entry public-domain codebook; only
//! a 10-bit index is transmitted.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

#[cfg(not(feature = "std"))]
extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

pub const CODEC_ID: u8 = 0x12;

pub const CODEBOOK_SIZE: usize = 1024;
pub const CODEBOOK_INDEX_BITS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleRate {
    Hz8000,
    Hz16000,
}

impl SampleRate {
    pub fn hz(self) -> u32 {
        match self {
            SampleRate::Hz8000 => 8_000,
            SampleRate::Hz16000 => 16_000,
        }
    }
}

pub struct VoxEncoder;
pub struct VoxDecoder;

impl VoxEncoder {
    pub fn new() -> Self { Self }
}

impl VoxDecoder {
    pub fn new() -> Self { Self }
}

impl Default for VoxEncoder { fn default() -> Self { Self::new() } }
impl Default for VoxDecoder { fn default() -> Self { Self::new() } }