//! tpt-audio-pulse — Tier-1 real-time audio codec.
//!
//! Pure transform coding with short MDCT windows (128 / 256 samples)
//! for ultra-low algorithmic latency (~2.5–5 ms @ 48 kHz).
//! No speech/music detection, no LPC.

#![forbid(unsafe_code)]

pub const CODEC_ID: u8 = 0x10;

pub struct PulseEncoder;

pub struct PulseDecoder;

impl PulseEncoder {
    pub fn new() -> Self { Self }
}

impl PulseDecoder {
    pub fn new() -> Self { Self }
}

impl Default for PulseEncoder { fn default() -> Self { Self::new() } }
impl Default for PulseDecoder { fn default() -> Self { Self::new() } }