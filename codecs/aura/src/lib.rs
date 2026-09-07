//! `tpt-audio-aura` — Aura: high-fidelity / archival audio codec.
//!
//! ## Modes
//!
//! - **Lossless** — linear-predictive coding (LPC) up to order 32
//!   followed by Exp-Golomb-coded residuals (zigzag-mapped). Bit-exact
//!   round-trip.
//! - **Lossy** — large-window MDCT (2048/4096) with Bark-band
//!   quantisation. Uses `f64` internally.
//!
//! This v0.1 implementation includes:
//!
//! - Autocorrelation-based LPC analysis (Levinson-Durbin recursion).
//! - Exp-Golomb-coded residuals (a tiny local `write_exp_golomb` /
//!   `read_exp_golomb`, NOT the `Rice` coder in `tpt-media-bitstream` —
//!   see `Known Issues` in `todo.md`; the v0.1 wire format uses
//!   Exp-Golomb and changing it would break every existing packet, so
//!   the doc is being brought into line with the code rather than vice
//!   versa).
//! - MDCT analysis and synthesis.
//!
//! See `docs/provenance.md` for algorithm sources.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use tpt_dsp_core::{autocorrelation, imdct, levinson_durbin, mdct};
use tpt_media_bitstream::{BitReader, BitWriter};
use tpt_media_codec::{
    Capabilities, CodecConfig, CodecError, CodecInfo, CodecResult, Decoder, DynDecoder,
    DynDecoderFactory, DynEncoder, DynEncoderFactory, Encoder,
};
use tpt_media_core::{
    AudioFrame, ChannelLayout, Frame, Packet, PacketFlags, SampleFormat, Timestamp, Timebase,
};

/// Codec id for Aura.
pub const CODEC_ID: [u8; 4] = *b"AURA";

/// Bitstream version.
pub const VERSION: u16 = 1;

fn static_info(caps: Capabilities) -> CodecInfo {
    CodecInfo {
        name: "Aura",
        codec_id: CODEC_ID,
        version: VERSION,
        capabilities: caps,
    }
}

/// Mode selector.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AuraMode {
    /// Linear-predictive lossless.
    Lossless,
    /// Large-window MDCT lossy.
    Lossy,
}

/// Aura configuration.
#[derive(Clone, Debug)]
pub struct AuraConfig {
    pub mode: AuraMode,
    pub sample_rate: u32,
    pub lpc_order: u8,
    pub block_size: u16,
    pub mdct_size: u16,
}

impl Default for AuraConfig {
    fn default() -> Self {
        AuraConfig {
            mode: AuraMode::Lossless,
            sample_rate: 48_000,
            lpc_order: 16,
            block_size: 1024,
            mdct_size: 2048,
        }
    }
}

/// Aura encoder.
pub struct AuraEncoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: AuraConfig,
}

impl AuraEncoder {
    pub fn new(codec_cfg: CodecConfig, enc: AuraConfig) -> CodecResult<Self> {
        if (enc.lpc_order as usize) > 32 {
            return Err(CodecError::InvalidConfig("aura lpc_order max 32"));
        }
        Ok(AuraEncoder {
            info: static_info(Capabilities {
                real_time_encoder: false,
                lossless: enc.mode == AuraMode::Lossless,
                lossy: enc.mode == AuraMode::Lossy,
                ..Capabilities::default()
            }),
            cfg: codec_cfg,
            enc,
        })
    }
}

impl Encoder for AuraEncoder {
    type Frame = AudioFrame;
    fn info(&self) -> &CodecInfo { &self.info }
    fn encode(&mut self, frame: &AudioFrame) -> CodecResult<Vec<Packet>> {
        self.cfg.limits.check_packet(64 * 1024)?;
        let fs = self.enc.block_size as usize;
        if self.enc.mode == AuraMode::Lossy && fs % 2 != 0 {
            return Err(CodecError::InvalidConfig("aura lossy block_size must be even"));
        }
        let n_samples = frame.samples.len() / 2;
        if n_samples < fs {
            return Err(CodecError::InvalidConfig("not enough samples for one block"));
        }
        let mut x = vec![0f64; fs];
        for i in 0..fs {
            x[i] = i16::from_le_bytes([frame.samples[2 * i], frame.samples[2 * i + 1]]) as f64 / i16::MAX as f64;
        }
        let order = self.enc.lpc_order as usize;
        // Upper bound: ~64 bits of header slack, `order` 16-bit LPC
        // coefficients, and up to 96 bits/sample (a generous bound on the
        // exp-Golomb-coded lossless residual — see `integer_predict`'s doc
        // for why the residual can occasionally be much larger than the
        // sample range; lossy only needs 16 bits/coefficient).
        let max_bits = 64 + order * 16 + fs * 96;
        let mut buf = vec![0u8; max_bits.div_ceil(8)];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            let mode_bit = if self.enc.mode == AuraMode::Lossless { 1u64 } else { 0 };
            w.write(1, mode_bit).map_err(|_| CodecError::Internal)?;
            w.write(5, self.enc.lpc_order as u64).map_err(|_| CodecError::Internal)?;
            w.write(8, (self.enc.mdct_size / 256 - 1) as u64).map_err(|_| CodecError::Internal)?;
            match self.enc.mode {
                AuraMode::Lossless => {
                    let mut r = vec![0f64; order + 1];
                    autocorrelation(&x, order, &mut r);
                    let mut a = vec![0f64; order];
                    let mut scratch = vec![0f64; order];
                    levinson_durbin(&r, &mut a, &mut scratch);
                    let coeffs_q = quantize_coeffs(&a);
                    let samples_i32: Vec<i32> = (0..fs)
                        .map(|i| i16::from_le_bytes([frame.samples[2 * i], frame.samples[2 * i + 1]]) as i32)
                        .collect();
                    for c in &coeffs_q {
                        w.write(16, (*c as i16 as u16) as u64).map_err(|_| CodecError::Internal)?;
                    }
                    for i in 0..fs {
                        let take = order.min(i);
                        let predicted = if take == 0 {
                            0
                        } else {
                            integer_predict(&samples_i32[..i], &coeffs_q[..take])
                        };
                        let residual = samples_i32[i] as i64 - predicted;
                        write_exp_golomb(&mut w, zigzag_encode(residual)).map_err(|_| CodecError::Internal)?;
                    }
                }
                AuraMode::Lossy => {
                    let n_half = fs / 2;
                    let mut spec = vec![0f64; n_half];
                    mdct(&x, &mut spec);
                    for s in &spec {
                        let q = (*s * 32767.0).clamp(-32768.0, 32767.0) as i16;
                        w.write(16, q as u64).map_err(|_| CodecError::Internal)?;
                    }
                }
            }
            w.finish().map_err(|_| CodecError::Internal)?
        };
        buf.truncate(written);
        Ok(vec![Packet {
            data: buf,
            pts: frame.frame.pts,
            dts: frame.frame.pts,
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        }])
    }
}

/// Fixed-point scale for quantized LPC coefficients (Q15): a coefficient
/// magnitude of 1.0 maps to ±32767/32768.
const COEFF_SHIFT: u32 = 15;
const COEFF_SCALE: f64 = 32768.0;

/// Quantize float LPC coefficients to Q15 fixed point, clamped to `i16`.
fn quantize_coeffs(a: &[f64]) -> Vec<i32> {
    a.iter()
        .map(|ai| (*ai * COEFF_SCALE).round().clamp(-32768.0, 32767.0) as i32)
        .collect()
}

/// Integer fixed-point LPC prediction from Q15 `coeffs_q` and integer
/// sample `history` (oldest first, most recent last).
///
/// Using the *quantized* coefficients and exact integer arithmetic on both
/// the encode and decode side is what makes lossless mode bit-exact:
/// coefficient/prediction error only ever affects how small the residual
/// is (compression efficiency), never whether decode reproduces the
/// original samples — encode and decode both compute this same function
/// over identical inputs (decode's "history" is itself reconstructed
/// bit-exactly by induction from sample 0 onward), so they always agree.
///
/// Because each of `coeffs_q.len()` (up to 32) terms can be as large as
/// `32767 * i16::MAX` in the worst case, the residual this feeds can
/// briefly exceed the input sample range by roughly two orders of
/// magnitude for pathological coefficients — callers sizing output
/// buffers should budget for that (see `encode`'s `max_bits`).
fn integer_predict(history: &[i32], coeffs_q: &[i32]) -> i64 {
    let h = history.len();
    let mut acc: i64 = 1i64 << (COEFF_SHIFT - 1); // round-to-nearest
    for (k, c) in coeffs_q.iter().enumerate() {
        acc += *c as i64 * history[h - 1 - k] as i64;
    }
    acc >> COEFF_SHIFT
}

/// Map a signed integer to an unsigned one so small-magnitude values
/// (positive or negative) get small codes: `0,-1,1,-2,2,... -> 0,1,2,3,4,...`.
fn zigzag_encode(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// Inverse of [`zigzag_encode`].
fn zigzag_decode(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

/// Exp-Golomb (Elias-gamma) code for `v >= 0`: encodes `val = v + 1` as
/// `n` zero bits followed by the `n+1`-bit binary representation of `val`
/// (`n = bit_length(val) - 1`), so the value field's own leading `1` bit
/// doubles as the prefix's terminator.
fn write_exp_golomb(w: &mut BitWriter, v: u64) -> Result<(), tpt_media_bitstream::WriteError> {
    let val = v + 1;
    let n = 63 - val.leading_zeros();
    for _ in 0..n {
        w.write(1, 0)?;
    }
    w.write(n + 1, val)?;
    Ok(())
}

/// Inverse of [`write_exp_golomb`].
fn read_exp_golomb(r: &mut BitReader) -> CodecResult<u64> {
    let mut n = 0u32;
    while r.peek(1).map_err(|_| CodecError::MalformedPacket)? == 0 {
        r.skip(1).map_err(|_| CodecError::MalformedPacket)?;
        n += 1;
        if n >= 56 {
            return Err(CodecError::MalformedPacket);
        }
    }
    let val = r.read(n + 1).map_err(|_| CodecError::MalformedPacket)?;
    Ok(val - 1)
}

/// Aura decoder.
pub struct AuraDecoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: AuraConfig,
}

impl AuraDecoder {
    pub fn new(codec_cfg: CodecConfig, enc: AuraConfig) -> CodecResult<Self> {
        Ok(AuraDecoder {
            info: static_info(Capabilities {
                real_time_decoder: false,
                lossless: enc.mode == AuraMode::Lossless,
                lossy: enc.mode == AuraMode::Lossy,
                ..Capabilities::default()
            }),
            cfg: codec_cfg,
            enc,
        })
    }
}

impl Decoder for AuraDecoder {
    type Frame = AudioFrame;
    fn info(&self) -> &CodecInfo { &self.info }
    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<AudioFrame>> {
        self.cfg.limits.check_packet(packet.data.len())?;
        self.decode_inner(packet)
    }
}

impl AuraDecoder {
    fn decode_inner(&mut self, packet: &Packet) -> CodecResult<Vec<AudioFrame>> {
        let mut r = BitReader::new(&packet.data);
        let mode_bit = r.read(1).map_err(|_| CodecError::MalformedPacket)?;
        let mode = if mode_bit == 1 { AuraMode::Lossless } else { AuraMode::Lossy };
        let lpc_order = r.read(5).map_err(|_| CodecError::MalformedPacket)? as u8;
        let _mdct_size_bits = r.read(8).map_err(|_| CodecError::MalformedPacket)?;
        let fs = self.enc.block_size as usize;
        let samples = match mode {
            AuraMode::Lossless => {
                let mut coeffs_q = vec![0i32; lpc_order as usize];
                for c in coeffs_q.iter_mut() {
                    let bits = r.read(16).map_err(|_| CodecError::MalformedPacket)? as u16;
                    *c = bits as i16 as i32;
                }
                // Reconstructed sample-by-sample: each `samples_i32[i]`
                // depends only on already-reconstructed history, which by
                // induction exactly equals the encoder's original samples
                // (see `integer_predict`'s doc) — this is what makes
                // lossless mode bit-exact.
                let mut samples_i32 = vec![0i32; fs];
                for i in 0..fs {
                    let s = read_exp_golomb(&mut r)?;
                    let residual = zigzag_decode(s);
                    let take = (lpc_order as usize).min(i);
                    let predicted = if take == 0 {
                        0
                    } else {
                        integer_predict(&samples_i32[..i], &coeffs_q[..take])
                    };
                    let sample = residual + predicted;
                    // Defensive clamp against malformed streams; a stream
                    // produced by our own encoder always fits exactly.
                    samples_i32[i] = sample.clamp(i16::MIN as i64, i16::MAX as i64) as i32;
                }
                let mut bytes = vec![0u8; fs * 2];
                for (i, v) in samples_i32.iter().enumerate() {
                    bytes[2 * i..2 * i + 2].copy_from_slice(&(*v as i16).to_le_bytes());
                }
                bytes
            }
            AuraMode::Lossy => {
                if fs % 2 != 0 {
                    return Err(CodecError::MalformedPacket);
                }
                let n_half = fs / 2;
                let mut spec = vec![0f64; n_half];
                for s in spec.iter_mut() {
                    let q = r.read(16).map_err(|_| CodecError::MalformedPacket)? as i16;
                    *s = q as f64 / 32767.0;
                }
                let mut x = vec![0f64; fs];
                imdct(&spec, &mut x);
                let mut bytes = vec![0u8; fs * 2];
                for (i, xi) in x.iter().enumerate() {
                    let v = (xi.clamp(-1.0, 1.0) * i16::MAX as f64) as i16;
                    bytes[2 * i..2 * i + 2].copy_from_slice(&v.to_le_bytes());
                }
                bytes
            }
        };
        Ok(vec![AudioFrame {
            frame: Frame {
                pts: packet.pts,
                duration: None,
                side_data: Vec::new(),
            },
            samples,
            sample_count: fs as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: self.enc.sample_rate,
        }])
    }
}

/// Type-erased decoder factory.
pub struct AuraDecoderFactory;
impl DynDecoderFactory for AuraDecoderFactory {
    fn codec_id(&self) -> &[u8; 4] { &CODEC_ID }
    fn info(&self) -> CodecInfo {
        static_info(Capabilities { lossless: true, lossy: true, ..Capabilities::default() })
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<alloc::boxed::Box<dyn DynDecoder>> {
        let enc = AuraConfig::default();
        let d = AuraDecoder::new(cfg.clone(), enc)?;
        Ok(alloc::boxed::Box::new(AuraDynDecoder { inner: d }))
    }
}
pub struct AuraDynDecoder { inner: AuraDecoder }
impl DynDecoder for AuraDynDecoder {
    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<tpt_media_codec::DynFrame>> {
        let frames = Decoder::decode(&mut self.inner, packet)?;
        Ok(frames.into_iter().map(|f| tpt_media_codec::DynFrame::Audio {
            sample_format: f.format,
            channel_layout: f.channel_layout,
            sample_rate: f.sample_rate,
            sample_count: f.sample_count,
            samples: f.samples,
        }).collect())
    }
    fn flush(&mut self) -> CodecResult<Vec<tpt_media_codec::DynFrame>> { Ok(Vec::new()) }
    fn info(&self) -> CodecInfo { self.inner.info().clone() }
}

pub struct AuraEncoderFactory;
impl DynEncoderFactory for AuraEncoderFactory {
    fn codec_id(&self) -> &[u8; 4] { &CODEC_ID }
    fn info(&self) -> CodecInfo {
        static_info(Capabilities { lossless: true, lossy: true, ..Capabilities::default() })
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<alloc::boxed::Box<dyn DynEncoder>> {
        let enc = AuraConfig::default();
        let e = AuraEncoder::new(cfg.clone(), enc)?;
        Ok(alloc::boxed::Box::new(AuraDynEncoder { inner: e }))
    }
}
pub struct AuraDynEncoder { inner: AuraEncoder }
impl DynEncoder for AuraDynEncoder {
    fn encode(&mut self, frame: &tpt_media_codec::DynFrame) -> CodecResult<Vec<Packet>> {
        let audio = match frame {
            tpt_media_codec::DynFrame::Audio { samples, sample_format, channel_layout, sample_rate, sample_count } => AudioFrame {
                frame: Frame {
                    pts: Timestamp::zero(Timebase::new(1, *sample_rate)),
                    duration: None,
                    side_data: Vec::new(),
                },
                samples: samples.clone(),
                sample_count: *sample_count,
                format: *sample_format,
                channel_layout: *channel_layout,
                sample_rate: *sample_rate,
            },
            _ => return Err(CodecError::Unsupported("Aura expects audio frames")),
        };
        Encoder::encode(&mut self.inner, &audio)
    }
    fn flush(&mut self) -> CodecResult<Vec<Packet>> { Encoder::flush(&mut self.inner) }
    fn info(&self) -> CodecInfo { self.inner.info().clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silence_frame(n: usize, rate: u32) -> AudioFrame {
        let mut f = AudioFrame::alloc(n as u32, SampleFormat::S16, ChannelLayout::Mono, rate);
        for s in f.samples.chunks_mut(2) {
            s.copy_from_slice(&0i16.to_le_bytes());
        }
        f
    }

    fn tone_frame(n: usize, rate: u32) -> AudioFrame {
        let mut f = AudioFrame::alloc(n as u32, SampleFormat::S16, ChannelLayout::Mono, rate);
        for (i, s) in f.samples.chunks_mut(2).enumerate() {
            let v = (0.5 * (i as f64 * 0.2).sin() * i16::MAX as f64) as i16;
            s.copy_from_slice(&v.to_le_bytes());
        }
        f
    }

    fn energy(frame: &AudioFrame) -> i64 {
        (0..frame.sample_count as usize)
            .map(|i| {
                let v = frame.s16(0, i).unwrap() as i64;
                v * v
            })
            .sum()
    }

    #[test]
    fn encode_decode_lossless_silence() {
        let cfg = CodecConfig::default();
        let enc_cfg = AuraConfig { mode: AuraMode::Lossless, block_size: 256, ..Default::default() };
        let mut enc = AuraEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = AuraDecoder::new(cfg, enc_cfg).unwrap();
        let mut frame = silence_frame(256, 48_000);
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));
        let packets = enc.encode(&frame).unwrap();
        let decoded = dec.decode(&packets[0]).unwrap();
        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn encode_decode_lossy() {
        let cfg = CodecConfig::default();
        let enc_cfg = AuraConfig { mode: AuraMode::Lossy, block_size: 256, ..Default::default() };
        let mut enc = AuraEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = AuraDecoder::new(cfg, enc_cfg).unwrap();
        let mut frame = silence_frame(256, 48_000);
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));
        let packets = enc.encode(&frame).unwrap();
        let decoded = dec.decode(&packets[0]).unwrap();
        assert_eq!(decoded.len(), 1);
    }

    #[test]
    fn encode_decode_lossless_tone_is_bit_exact() {
        // Regression test: an earlier version sized the packet buffer with
        // `Vec::new()` (zero capacity, zero length) instead of an actual
        // length, so `BitWriter` silently wrote nothing and every packet
        // decoded back to silence regardless of input. Lossless mode is
        // documented as bit-exact, so a real tone must round-trip exactly.
        let cfg = CodecConfig::default();
        let enc_cfg = AuraConfig { mode: AuraMode::Lossless, block_size: 256, lpc_order: 8, ..Default::default() };
        let mut enc = AuraEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = AuraDecoder::new(cfg, enc_cfg).unwrap();
        let mut frame = tone_frame(256, 48_000);
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));
        let packets = enc.encode(&frame).unwrap();
        assert!(!packets[0].data.is_empty(), "encoded packet must not be empty");
        let decoded = dec.decode(&packets[0]).unwrap();
        for i in 0..256 {
            assert_eq!(
                frame.s16(0, i),
                decoded[0].s16(0, i),
                "sample {i} not bit-exact"
            );
        }
    }

    #[test]
    fn encode_decode_lossy_tone_carries_signal() {
        let cfg = CodecConfig::default();
        let enc_cfg = AuraConfig { mode: AuraMode::Lossy, block_size: 256, ..Default::default() };
        let mut enc = AuraEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = AuraDecoder::new(cfg, enc_cfg).unwrap();
        let mut frame = tone_frame(256, 48_000);
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));
        let packets = enc.encode(&frame).unwrap();
        assert!(!packets[0].data.is_empty(), "encoded packet must not be empty");
        let decoded = dec.decode(&packets[0]).unwrap();
        assert!(energy(&decoded[0]) > 0, "decoded tone must not be silent");
    }

    #[test]
    fn rejects_oversized_lpc_order() {
        let cfg = CodecConfig::default();
        let enc_cfg = AuraConfig { lpc_order: 64, ..Default::default() };
        assert!(AuraEncoder::new(cfg, enc_cfg).is_err());
    }
}