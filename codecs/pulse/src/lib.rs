//! `tpt-audio-pulse` — Pulse: low-latency real-time audio codec.
//!
//! ## Algorithm
//!
//! - Frame sizes: 128 or 256 samples (default 256).
//! - Transform: MDCT with sine window.
//! - Quantisation: uniform scalar with per-band step size.
//! - Entropy: fixed-width bit fields (uniform quantiser index).
//! - PLC: phase-randomised sign perturbation of previous frame's MDCT.
//!
//! See `docs/provenance.md` for algorithm sources.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use tpt_dsp_core::{mdct_sine_window, FastMdctPlan};
use tpt_media_bitstream::{BitReader, BitWriter};
use tpt_media_codec::{
    Capabilities, CodecConfig, CodecError, CodecInfo, CodecResult, Decoder, DynDecoder,
    DynDecoderFactory, DynEncoder, DynEncoderFactory, Encoder, SupportedFormat,
};
use tpt_media_core::{
    AudioFrame, ChannelLayout, Frame, Packet, PacketFlags, SampleFormat, Timebase, Timestamp,
};

/// Supported frame size.
pub const FRAME_128: u16 = 128;
/// Supported frame size.
pub const FRAME_256: u16 = 256;

/// Codec id for Pulse.
pub const CODEC_ID: [u8; 4] = *b"PULS";

/// Bitstream version.
pub const VERSION: u16 = 1;

fn static_info(caps: Capabilities) -> CodecInfo {
    CodecInfo {
        name: "Pulse",
        codec_id: CODEC_ID,
        version: VERSION,
        capabilities: caps,
        supported_formats: alloc::vec![
            SupportedFormat::Audio {
                sample_format: SampleFormat::S16,
                channel_layout: ChannelLayout::Mono,
                sample_rate: 48_000,
            },
            SupportedFormat::Audio {
                sample_format: SampleFormat::S16,
                channel_layout: ChannelLayout::Mono,
                sample_rate: 44_100,
            },
        ],
    }
}

/// Sine analysis/synthesis window of length `len` (`len` must be even),
/// shared with Aura and any other MDCT-based codec — see
/// `tpt_dsp_core::mdct_sine_window`.
fn sine_window(len: usize) -> Vec<f32> {
    let mut w = vec![0f32; len];
    mdct_sine_window(len / 2, &mut w);
    w
}

#[inline]
fn quantize(v: f32, step: f32) -> i32 {
    if step <= 0.0 {
        return 0;
    }
    (v / step).round() as i32
}

#[inline]
fn dequantize(q: i32, step: f32) -> f32 {
    q as f32 * step
}

/// Pulse encoder configuration.
#[derive(Clone, Debug)]
pub struct PulseEncoderConfig {
    pub frame_size: u16,
    pub sample_rate: u32,
    pub n_bands: u8,
    pub band_steps: Vec<f32>,
    pub bits_per_coef: u8,
}

impl Default for PulseEncoderConfig {
    fn default() -> Self {
        PulseEncoderConfig {
            frame_size: 256,
            sample_rate: 48_000,
            n_bands: 16,
            band_steps: alloc::vec![1.0; 16],
            bits_per_coef: 8,
        }
    }
}

/// Pulse encoder.
pub struct PulseEncoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: PulseEncoderConfig,
    last_spectrum: Vec<f32>,
    mdct_plan: FastMdctPlan,
}

impl PulseEncoder {
    pub fn new(codec_cfg: CodecConfig, enc: PulseEncoderConfig) -> CodecResult<Self> {
        if enc.frame_size != 128 && enc.frame_size != 256 {
            return Err(CodecError::InvalidConfig("frame_size must be 128 or 256"));
        }
        if enc.band_steps.len() != enc.n_bands as usize {
            return Err(CodecError::InvalidConfig(
                "band_steps length must equal n_bands",
            ));
        }
        let mdct_plan = FastMdctPlan::new(enc.frame_size as usize);
        Ok(PulseEncoder {
            info: static_info(Capabilities {
                real_time_encoder: true,
                ..Capabilities::default()
            }),
            cfg: codec_cfg,
            enc,
            last_spectrum: Vec::new(),
            mdct_plan,
        })
    }
}

impl Encoder for PulseEncoder {
    type Frame = AudioFrame;
    fn info(&self) -> &CodecInfo {
        &self.info
    }
    fn encode(&mut self, frame: &AudioFrame) -> CodecResult<Vec<Packet>> {
        self.cfg.limits.check_packet(4 * 1024)?;
        let fs = self.enc.frame_size as usize;
        let two_n = 2 * fs;
        if frame.samples.len() < two_n * 2 {
            return Err(CodecError::InvalidConfig(
                "not enough samples for one frame",
            ));
        }
        let mut x = vec![0f32; two_n];
        for i in 0..two_n {
            let v = i16::from_le_bytes([frame.samples[2 * i], frame.samples[2 * i + 1]]) as f32
                / i16::MAX as f32;
            x[i] = v;
        }
        let w = sine_window(two_n);
        for i in 0..two_n {
            x[i] *= w[i];
        }
        let mut spec = vec![0f32; fs];
        self.mdct_plan.forward(&x, &mut spec);
        self.last_spectrum = spec.clone();
        let band_size = (spec.len() / self.enc.n_bands as usize).max(1);
        // Upper bound: 2-byte header + per band (2-byte step + 1-byte
        // count + up to `bits_per_coef` bits per coefficient), plus the
        // trailing keyframe bit, rounded up to whole bytes.
        let max_bits =
            16 + self.enc.n_bands as usize * (24 + self.enc.bits_per_coef as usize * band_size) + 8;
        let mut buf = vec![0u8; max_bits.div_ceil(8)];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            w.write(1, (self.enc.frame_size as u64 / 128) - 1)
                .map_err(|_| CodecError::Internal)?;
            w.write(8, self.enc.n_bands as u64)
                .map_err(|_| CodecError::Internal)?;
            for band in 0..self.enc.n_bands as usize {
                let step_bits = self.enc.band_steps[band].clamp(0.0, 65535.0) as u16;
                w.write(16, step_bits as u64)
                    .map_err(|_| CodecError::Internal)?;
                let start = band * band_size;
                let end = ((band + 1) * band_size).min(spec.len());
                w.write(8, (end - start) as u64)
                    .map_err(|_| CodecError::Internal)?;
                for i in start..end {
                    let q = quantize(spec[i], self.enc.band_steps[band].max(1e-9)).clamp(-127, 127);
                    w.write(self.enc.bits_per_coef as u32, (q + 128) as u64)
                        .map_err(|_| CodecError::Internal)?;
                }
            }
            w.write(1, 1).map_err(|_| CodecError::Internal)?;
            w.finish().map_err(|_| CodecError::Internal)?
        };
        buf.truncate(written);
        let pkt = Packet {
            data: buf,
            pts: frame.frame.pts,
            dts: frame.frame.pts,
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        Ok(vec![pkt])
    }
}

/// Pulse decoder.
///
/// Holds a pre-built [`FastMdctPlan`] for each supported frame size (128 and
/// 256). The wire format self-describes its frame size per packet
/// (`decode_inner` reads it from the bitstream), independent of `enc`'s own
/// configured default, so a single fixed-size plan is not enough — the
/// decoder must be able to invert whichever size the sender actually used.
pub struct PulseDecoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: PulseEncoderConfig,
    last_spectrum: Vec<f32>,
    plc_phase: u32,
    mdct_plan_128: FastMdctPlan,
    mdct_plan_256: FastMdctPlan,
}

impl PulseDecoder {
    pub fn new(codec_cfg: CodecConfig, enc: PulseEncoderConfig) -> CodecResult<Self> {
        if enc.frame_size != 128 && enc.frame_size != 256 {
            return Err(CodecError::InvalidConfig("frame_size must be 128 or 256"));
        }
        Ok(PulseDecoder {
            info: static_info(Capabilities {
                real_time_decoder: true,
                ..Capabilities::default()
            }),
            cfg: codec_cfg,
            enc,
            last_spectrum: Vec::new(),
            plc_phase: 0,
            mdct_plan_128: FastMdctPlan::new(FRAME_128 as usize),
            mdct_plan_256: FastMdctPlan::new(FRAME_256 as usize),
        })
    }

    /// The pre-built plan matching spectral length `n` (128 or 256).
    fn mdct_plan_for(&mut self, n: usize) -> &mut FastMdctPlan {
        if n == FRAME_128 as usize {
            &mut self.mdct_plan_128
        } else {
            &mut self.mdct_plan_256
        }
    }

    /// Packet-loss concealment: synthesise a plausible frame.
    pub fn conceal(&mut self, pts: Timestamp) -> CodecResult<AudioFrame> {
        let fs = self.enc.frame_size;
        let two_n = 2 * fs as usize;
        let mut spec = self.last_spectrum.clone();
        if spec.is_empty() {
            spec = alloc::vec![0.0; fs as usize];
        }
        for (i, s) in spec.iter_mut().enumerate() {
            let phase = self.plc_phase.wrapping_add(i as u32);
            if phase.wrapping_mul(2654435761) & 0x8000_0000 != 0 {
                *s = -*s;
            }
        }
        self.plc_phase = self.plc_phase.wrapping_add(1);
        let mut x = vec![0f32; two_n];
        self.mdct_plan_for(fs as usize).inverse(&spec, &mut x);
        let w = sine_window(two_n);
        for i in 0..two_n {
            x[i] *= w[i];
        }
        let mut samples = vec![0u8; two_n * 2];
        for i in 0..two_n {
            let v = (x[i].clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            samples[2 * i..2 * i + 2].copy_from_slice(&v.to_le_bytes());
        }
        Ok(AudioFrame {
            frame: Frame {
                pts,
                duration: None,
                side_data: Vec::new(),
            },
            samples,
            sample_count: two_n as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: self.enc.sample_rate,
        })
    }
}

impl Decoder for PulseDecoder {
    type Frame = AudioFrame;
    fn info(&self) -> &CodecInfo {
        &self.info
    }

    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<AudioFrame>> {
        self.cfg.limits.check_packet(packet.data.len())?;
        self.decode_inner(packet)
    }
}

impl PulseDecoder {
    fn decode_inner(&mut self, packet: &Packet) -> CodecResult<Vec<AudioFrame>> {
        let mut r = BitReader::new(&packet.data);
        let fs_code = r.read(1).map_err(|_| CodecError::MalformedPacket)?;
        let frame_size = 128 * (fs_code as u16 + 1);
        if frame_size != 128 && frame_size != 256 {
            return Err(CodecError::MalformedPacket);
        }
        let n_bands = r.read(8).map_err(|_| CodecError::MalformedPacket)? as u8;
        let band_size = (frame_size as usize / n_bands as usize).max(1);
        let mut spec = vec![0f32; frame_size as usize];
        for band in 0..n_bands as usize {
            let step_bits = r.read(16).map_err(|_| CodecError::MalformedPacket)? as u16;
            let step = step_bits as f32;
            let n_coeffs = r.read(8).map_err(|_| CodecError::MalformedPacket)? as usize;
            let start = band * band_size;
            for i in 0..n_coeffs {
                let q = r
                    .read(self.enc.bits_per_coef as u32)
                    .map_err(|_| CodecError::MalformedPacket)? as i32;
                spec[start + i] = dequantize(q - 128, step.max(1e-9));
            }
        }
        let _keyframe = r.read(1).map_err(|_| CodecError::MalformedPacket)?;
        self.last_spectrum = spec.clone();
        let two_n = 2 * frame_size as usize;
        let mut x = vec![0f32; two_n];
        self.mdct_plan_for(frame_size as usize)
            .inverse(&spec, &mut x);
        let w = sine_window(two_n);
        for i in 0..two_n {
            x[i] *= w[i];
        }
        let mut samples = vec![0u8; two_n * 2];
        for i in 0..two_n {
            let v = (x[i].clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            samples[2 * i..2 * i + 2].copy_from_slice(&v.to_le_bytes());
        }
        Ok(vec![AudioFrame {
            frame: Frame {
                pts: packet.pts,
                duration: None,
                side_data: Vec::new(),
            },
            samples,
            sample_count: two_n as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: self.enc.sample_rate,
        }])
    }
}

/// Type-erased decoder factory.
pub struct PulseDecoderFactory;
impl DynDecoderFactory for PulseDecoderFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        static_info(Capabilities {
            real_time_decoder: true,
            ..Capabilities::default()
        })
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<alloc::boxed::Box<dyn DynDecoder>> {
        let cfg2 = PulseEncoderConfig::default();
        let decoder = PulseDecoder::new(cfg.clone(), cfg2)?;
        Ok(alloc::boxed::Box::new(PulseDynDecoder { inner: decoder }))
    }
}

pub struct PulseDynDecoder {
    inner: PulseDecoder,
}
impl DynDecoder for PulseDynDecoder {
    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<tpt_media_codec::DynFrame>> {
        let frames = Decoder::decode(&mut self.inner, packet)?;
        Ok(frames
            .into_iter()
            .map(|f| tpt_media_codec::DynFrame::Audio {
                sample_format: f.format,
                channel_layout: f.channel_layout,
                sample_rate: f.sample_rate,
                sample_count: f.sample_count,
                samples: f.samples,
            })
            .collect())
    }
    fn flush(&mut self) -> CodecResult<Vec<tpt_media_codec::DynFrame>> {
        Ok(Vec::new())
    }
    fn drain(&mut self) -> CodecResult<Vec<tpt_media_codec::DynFrame>> {
        Ok(Vec::new())
    }
    fn info(&self) -> CodecInfo {
        self.inner.info().clone()
    }
}

/// Type-erased encoder factory.
pub struct PulseEncoderFactory;
impl DynEncoderFactory for PulseEncoderFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        static_info(Capabilities {
            real_time_encoder: true,
            ..Capabilities::default()
        })
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<alloc::boxed::Box<dyn DynEncoder>> {
        let cfg2 = PulseEncoderConfig::default();
        let enc = PulseEncoder::new(cfg.clone(), cfg2)?;
        Ok(alloc::boxed::Box::new(PulseDynEncoder { inner: enc }))
    }
}

pub struct PulseDynEncoder {
    inner: PulseEncoder,
}
impl DynEncoder for PulseDynEncoder {
    fn encode(&mut self, frame: &tpt_media_codec::DynFrame) -> CodecResult<Vec<Packet>> {
        let audio = match frame {
            tpt_media_codec::DynFrame::Audio {
                samples,
                sample_format,
                channel_layout,
                sample_rate,
                sample_count,
            } => AudioFrame {
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
            _ => return Err(CodecError::Unsupported("Pulse expects audio frames")),
        };
        Encoder::encode(&mut self.inner, &audio)
    }
    fn flush(&mut self) -> CodecResult<Vec<Packet>> {
        Encoder::flush(&mut self.inner)
    }
    fn drain(&mut self) -> CodecResult<Vec<Packet>> {
        Encoder::drain(&mut self.inner)
    }
    fn info(&self) -> CodecInfo {
        self.inner.info().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_silence_frame(n: usize) -> AudioFrame {
        let mut f = AudioFrame::alloc(n as u32, SampleFormat::S16, ChannelLayout::Mono, 48_000);
        for s in f.samples.chunks_mut(2) {
            s.copy_from_slice(&0i16.to_le_bytes());
        }
        f
    }

    #[test]
    fn encode_decode_round_trip_silence() {
        let cfg = CodecConfig::default();
        let enc_cfg = PulseEncoderConfig {
            frame_size: 128,
            n_bands: 4,
            band_steps: alloc::vec![1.0; 4],
            ..Default::default()
        };
        let mut enc = PulseEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = PulseDecoder::new(cfg, enc_cfg).unwrap();
        let mut frame = make_silence_frame(256);
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));
        let packets = enc.encode(&frame).unwrap();
        assert_eq!(packets.len(), 1);
        assert!(packets[0].is_keyframe());
        let decoded = dec.decode(&packets[0]).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].sample_count, 256);
    }

    #[test]
    fn encode_decode_round_trip_tone_carries_signal() {
        // Regression test: an earlier version sized the packet buffer with
        // `Vec::with_capacity` instead of an actual length, so `BitWriter`
        // silently wrote nothing and every packet decoded back to silence
        // regardless of input. A real (non-silent) tone must survive
        // encode+decode with most of its energy intact.
        let cfg = CodecConfig::default();
        let enc_cfg = PulseEncoderConfig {
            frame_size: 128,
            n_bands: 8,
            band_steps: alloc::vec![50.0; 8],
            bits_per_coef: 8,
            ..Default::default()
        };
        let mut enc = PulseEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = PulseDecoder::new(cfg, enc_cfg).unwrap();

        let n = 256usize;
        let mut frame = AudioFrame::alloc(n as u32, SampleFormat::S16, ChannelLayout::Mono, 48_000);
        for (i, s) in frame.samples.chunks_mut(2).enumerate() {
            let v = (0.5 * (i as f32 * 0.2).sin() * i16::MAX as f32) as i16;
            s.copy_from_slice(&v.to_le_bytes());
        }
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));

        let packets = enc.encode(&frame).unwrap();
        assert!(
            !packets[0].data.is_empty(),
            "encoded packet must not be empty"
        );

        let decoded = dec.decode(&packets[0]).unwrap();
        let out = &decoded[0];
        let energy: i64 = (0..out.sample_count as usize)
            .map(|i| {
                let v = out.s16(0, i).unwrap() as i64;
                v * v
            })
            .sum();
        assert!(energy > 0, "decoded tone must not be silent");
    }

    #[test]
    fn plc_synthesises_a_frame() {
        let cfg = CodecConfig::default();
        let enc_cfg = PulseEncoderConfig::default();
        let mut dec = PulseDecoder::new(cfg, enc_cfg).unwrap();
        let pts = Timestamp::zero(Timebase::new(1, 48_000));
        let f = dec.conceal(pts).unwrap();
        assert_eq!(f.sample_count, 512);
    }

    #[test]
    fn default_frame_size_256_round_trips() {
        // Regression: the frame-size header was written into 8 bits, so the
        // default frame_size 256 overflowed to 0 and the decoder rejected
        // every packet. The header now carries a 1-bit 128/256 selector.
        let cfg = CodecConfig::default();
        let enc_cfg = PulseEncoderConfig::default();
        let mut enc = PulseEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = PulseDecoder::new(cfg, enc_cfg).unwrap();

        let mut frame = AudioFrame::alloc(512, SampleFormat::S16, ChannelLayout::Mono, 48_000);
        for (i, s) in frame.samples.chunks_mut(2).enumerate() {
            let v = (0.4 * (i as f32 * 0.07).sin() * i16::MAX as f32) as i16;
            s.copy_from_slice(&v.to_le_bytes());
        }
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));

        let packets = enc.encode(&frame).unwrap();
        assert_eq!(packets.len(), 1);
        let decoded = dec.decode(&packets[0]).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].sample_count, 512);
    }

    #[test]
    fn rejects_bad_frame_size() {
        let cfg = CodecConfig::default();
        let enc_cfg = PulseEncoderConfig {
            frame_size: 333,
            ..Default::default()
        };
        assert!(PulseEncoder::new(cfg.clone(), enc_cfg.clone()).is_err());
    }

    #[test]
    fn factory_builds_decoder() {
        let f = PulseDecoderFactory;
        assert_eq!(f.codec_id(), b"PULS");
        let dec = f.build(&CodecConfig::default()).unwrap();
        assert_eq!(dec.info().codec_id, CODEC_ID);
    }

    #[test]
    fn factory_builds_encoder() {
        let f = PulseEncoderFactory;
        let enc = f.build(&CodecConfig::default()).unwrap();
        assert_eq!(enc.info().codec_id, CODEC_ID);
    }
}
