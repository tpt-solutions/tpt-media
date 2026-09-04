//! tpt-audio-aura — Tier-2 high-fidelity / archival audio codec.
//!
//! Mode A (Lossless): high-order LPC (up to order 32) + Rice-coded residual.
//! Mode B (Lossy): large MDCT (2048 / 4096) + Bark-scale psychoacoustic
//! masking and noise shaping. Internal precision: f64.

#![forbid(unsafe_code)]

pub const CODEC_ID: u8 = 0x11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Lossless,
    Lossy,
}

pub struct AuraEncoder { mode: Mode }
pub struct AuraDecoder { mode: Mode }

impl AuraEncoder {
    pub fn new(mode: Mode) -> Self { Self { mode } }
    pub fn mode(&self) -> Mode { self.mode }
}

impl AuraDecoder {
    pub fn new(mode: Mode) -> Self { Self { mode } }
    pub fn mode(&self) -> Mode { self.mode }
}