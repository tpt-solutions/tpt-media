//! `tpt-audio-cinder` — Cinder: low-latency real-time audio codec.
//!
//! ## Algorithm
//!
//! - Frame sizes: 128 or 256 samples (default 256).
//! - Transform: MDCT with sine window.
//! - Quantisation: uniform scalar with per-band step size, with an
//!   optional perceptual reweighting (see [`CinderEncoderConfig::perceptual_weighting`]).
//! - Entropy: fixed-width bit fields (uniform quantiser index).
//! - PLC: phase-randomised sign perturbation of previous frame's MDCT.
//!
//! ## Optional perceptual weighting
//!
//! Unlike Aura's lossy mode (full simultaneous-masking model, §10 of
//! `todo.md`), Cinder is a low-latency, low-complexity real-time codec, so
//! it deliberately uses only the cheapest piece of that toolkit: the
//! absolute threshold of hearing
//! ([`tpt_dsp_core::threshold_in_quiet`]), evaluated once per band at
//! construction time (not per frame — it depends only on the fixed
//! band layout, so there is nothing to recompute per packet). Each band's
//! configured step is scaled by that band's threshold relative to a 1kHz
//! reference, so bands the ear is objectively less sensitive to (relative
//! to 1kHz) get proportionally coarser steps for the same nominal
//! `band_steps` input. The scaling only ever *coarsens* a band's step,
//! never sharpens it below its configured value, since the wire format's
//! step field is an integer with no sub-configured granularity to
//! sharpen into. Zero added real-time cost, no wire-format change, since
//! Cinder already transmits the actual per-band step used.
//!
//! See `docs/provenance.md` for algorithm sources.

#![warn(missing_docs)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use tpt_dsp_core::{db_to_power, mdct_sine_window, threshold_in_quiet, FastMdctPlan};
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

/// Codec id for Cinder.
pub const CODEC_ID: [u8; 4] = *b"CIND";

/// Bitstream version.
pub const VERSION: u16 = 1;

fn static_info(caps: Capabilities) -> CodecInfo {
    CodecInfo {
        name: "Cinder",
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

/// Cinder encoder configuration.
#[derive(Clone, Debug)]
pub struct CinderEncoderConfig {
    /// MDCT window size in samples: 128 or 256. Encode consumes
    /// `2·frame_size` samples per block (the TDAC window pair).
    pub frame_size: u16,
    /// Sample rate in Hz; 44.1/48 kHz per the supported-format table.
    pub sample_rate: u32,
    /// Number of spectral bands the spectrum is quantised in; the
    /// `frame_size` MDCT bins are split into `n_bands` equal bands.
    pub n_bands: u8,
    /// Per-band quantisation steps (length must equal `n_bands`).
    /// Larger = coarser = fewer bits.
    pub band_steps: Vec<f32>,
    /// Fixed bit width per quantised coefficient, 4..=12 (the
    /// `--quality` mapping target).
    pub bits_per_coef: u8,
    /// When `true`, each band's step from `band_steps` is scaled by that
    /// band's absolute threshold of hearing relative to a 1kHz reference
    /// (see the module doc's "Optional perceptual weighting" section).
    /// Defaults to `false` (every existing caller keeps its exact
    /// previous behavior).
    pub perceptual_weighting: bool,
}

impl Default for CinderEncoderConfig {
    fn default() -> Self {
        CinderEncoderConfig {
            frame_size: 256,
            sample_rate: 48_000,
            n_bands: 16,
            band_steps: alloc::vec![1.0; 16],
            bits_per_coef: 8,
            perceptual_weighting: false,
        }
    }
}

/// Reference frequency (Hz) the per-band perceptual weight is normalized
/// against: a band's weight is 1.0 at 1kHz, `>1` where the ear is less
/// sensitive than at 1kHz (coarser step), `<1` where it's more sensitive
/// (finer step). Purely relative, so it needs no dBFS/SPL calibration
/// assumption — see the module doc.
const PERCEPTUAL_REFERENCE_HZ: f32 = 1000.0;

/// Precompute a per-band perceptual weight multiplier for `n_bands` bands
/// spanning `frame_size` MDCT bins at `sample_rate`, from the absolute
/// threshold of hearing alone (see the module doc's "Optional perceptual
/// weighting" section). Returns all-`1.0` (a no-op) when `enabled` is
/// `false`.
fn perceptual_band_weights(
    enabled: bool,
    frame_size: usize,
    n_bands: usize,
    sample_rate: u32,
) -> Vec<f32> {
    let mut weights = vec![1.0f32; n_bands];
    if !enabled || n_bands == 0 {
        return weights;
    }
    let band_size = (frame_size / n_bands).max(1);
    let nyquist = sample_rate as f32 / 2.0;
    let reference_db = threshold_in_quiet(PERCEPTUAL_REFERENCE_HZ);
    for (band, w) in weights.iter_mut().enumerate() {
        let start = band * band_size;
        let end = ((band + 1) * band_size).min(frame_size).max(start + 1);
        let mid_bin = (start + end - 1) / 2;
        let bin_hz = (mid_bin as f32 + 0.5) * nyquist / frame_size as f32;
        let quiet_db = threshold_in_quiet(bin_hz.max(1.0));
        *w = db_to_power(quiet_db - reference_db);
    }
    weights
}

/// Cinder encoder.
pub struct CinderEncoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: CinderEncoderConfig,
    last_spectrum: Vec<f32>,
    mdct_plan: FastMdctPlan,
    /// Per-band perceptual weight multiplier (all `1.0` when
    /// `enc.perceptual_weighting` is `false`); precomputed once here since
    /// it depends only on the fixed band layout, not per-frame signal
    /// content — see [`perceptual_band_weights`].
    perceptual_weights: Vec<f32>,
}

impl CinderEncoder {
    /// Build an encoder for `enc` under the framework configuration
    /// (`version` + resource limits); validates frame size, band
    /// count, and step-table length before anything is allocated.
    pub fn new(codec_cfg: CodecConfig, enc: CinderEncoderConfig) -> CodecResult<Self> {
        if enc.frame_size != 128 && enc.frame_size != 256 {
            return Err(CodecError::InvalidConfig("frame_size must be 128 or 256"));
        }
        if enc.band_steps.len() != enc.n_bands as usize {
            return Err(CodecError::InvalidConfig(
                "band_steps length must equal n_bands",
            ));
        }
        let mdct_plan = FastMdctPlan::new(enc.frame_size as usize);
        let perceptual_weights = perceptual_band_weights(
            enc.perceptual_weighting,
            enc.frame_size as usize,
            enc.n_bands as usize,
            enc.sample_rate,
        );
        Ok(CinderEncoder {
            info: static_info(Capabilities {
                real_time_encoder: true,
                ..Capabilities::default()
            }),
            cfg: codec_cfg,
            enc,
            last_spectrum: Vec::new(),
            mdct_plan,
            perceptual_weights,
        })
    }
}

impl Encoder for CinderEncoder {
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
        let mut x: Vec<f32> = frame.samples[..two_n * 2]
            .chunks_exact(2)
            .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / i16::MAX as f32)
            .collect();
        let w = sine_window(two_n);
        for (xi, wi) in x.iter_mut().zip(w.iter()) {
            *xi *= *wi;
        }
        let mut spec = vec![0f32; fs];
        self.mdct_plan.forward(&x, &mut spec);
        self.last_spectrum = spec.clone();
        let band_size = (spec.len() / self.enc.n_bands as usize).max(1);
        // Upper bound: 2-byte header + per band (2-byte step + 1-byte
        // count + up to `bits_per_coef` bits per coefficient), plus the
        // trailing keyframe bit and the leading wire-version field,
        // rounded up to whole bytes.
        let max_bits = 16
            + self.enc.n_bands as usize * (24 + self.enc.bits_per_coef as usize * band_size)
            + 8
            + 16;
        let mut buf = vec![0u8; max_bits.div_ceil(8)];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            // §35: a versioned bitstream, checked first on decode so an
            // unrecognized version fails cleanly instead of being
            // misparsed as this decoder's own layout.
            w.write(16, VERSION as u64)
                .map_err(|_| CodecError::Internal)?;
            w.write(1, (self.enc.frame_size as u64 / 128) - 1)
                .map_err(|_| CodecError::Internal)?;
            w.write(8, self.enc.n_bands as u64)
                .map_err(|_| CodecError::Internal)?;
            for band in 0..self.enc.n_bands as usize {
                // The wire format's step field is an integer `u16` (no
                // fractional granularity), so perceptual weighting can
                // only ever *coarsen* a band's step (weight > 1, for
                // less-sensitive bands), never sharpen an already
                // integer-quantized one below its own configured value —
                // floor at the configured step itself.
                let effective_step = (self.enc.band_steps[band] * self.perceptual_weights[band])
                    .max(self.enc.band_steps[band]);
                let step_bits = effective_step.clamp(0.0, 65535.0) as u16;
                w.write(16, step_bits as u64)
                    .map_err(|_| CodecError::Internal)?;
                let start = band * band_size;
                let end = ((band + 1) * band_size).min(spec.len());
                w.write(8, (end - start) as u64)
                    .map_err(|_| CodecError::Internal)?;
                for &s in &spec[start..end] {
                    let q = quantize(s, effective_step.max(1e-9)).clamp(-127, 127);
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

/// Cinder decoder.
///
/// Holds a pre-built [`FastMdctPlan`] for each supported frame size (128 and
/// 256). The wire format self-describes its frame size per packet
/// (`decode_inner` reads it from the bitstream), independent of `enc`'s own
/// configured default, so a single fixed-size plan is not enough — the
/// decoder must be able to invert whichever size the sender actually used.
pub struct CinderDecoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: CinderEncoderConfig,
    last_spectrum: Vec<f32>,
    plc_phase: u32,
    mdct_plan_128: FastMdctPlan,
    mdct_plan_256: FastMdctPlan,
}

impl CinderDecoder {
    /// Build a decoder matching the *encoder* configuration `enc`
    /// (band layout, bit depth, rates); frame size is self-described
    /// per packet, so a decoder configured for one size decodes the
    /// other correctly.
    pub fn new(codec_cfg: CodecConfig, enc: CinderEncoderConfig) -> CodecResult<Self> {
        if enc.frame_size != 128 && enc.frame_size != 256 {
            return Err(CodecError::InvalidConfig("frame_size must be 128 or 256"));
        }
        Ok(CinderDecoder {
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

impl Decoder for CinderDecoder {
    type Frame = AudioFrame;
    fn info(&self) -> &CodecInfo {
        &self.info
    }

    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<AudioFrame>> {
        self.cfg.limits.check_packet(packet.data.len())?;
        self.decode_inner(packet)
    }
}

impl CinderDecoder {
    fn decode_inner(&mut self, packet: &Packet) -> CodecResult<Vec<AudioFrame>> {
        let mut r = BitReader::new(&packet.data);
        // §35: reject an unrecognized wire version before interpreting
        // any further bits as if they were this decoder's own layout.
        let wire_version = r.read(16).map_err(|_| CodecError::MalformedPacket)? as u16;
        if wire_version != VERSION {
            return Err(CodecError::Unsupported("unsupported Cinder packet version"));
        }
        let fs_code = r.read(1).map_err(|_| CodecError::MalformedPacket)?;
        let frame_size = 128 * (fs_code as u16 + 1);
        if frame_size != 128 && frame_size != 256 {
            return Err(CodecError::MalformedPacket);
        }
        self.cfg
            .limits
            .check_allocation(frame_size as usize * core::mem::size_of::<f32>())?;
        self.cfg.limits.check_frame_bytes(4 * frame_size as usize)?;
        let n_bands = r.read(8).map_err(|_| CodecError::MalformedPacket)? as u8;
        if n_bands == 0 {
            return Err(CodecError::MalformedPacket);
        }
        let band_size = (frame_size as usize / n_bands as usize).max(1);
        let mut spec = vec![0f32; frame_size as usize];
        for band in 0..n_bands as usize {
            let step_bits = r.read(16).map_err(|_| CodecError::MalformedPacket)? as u16;
            let step = step_bits as f32;
            let n_coeffs = r.read(8).map_err(|_| CodecError::MalformedPacket)? as usize;
            let start = band * band_size;
            // `n_bands`/`n_coeffs` come straight from the packet, so a
            // malformed or adversarial stream can claim more coefficients
            // than actually fit in `spec` (e.g. `n_bands` far exceeding
            // `frame_size`, or a `band`/`n_coeffs` combination that runs
            // past the end) — bounds-check before writing instead of
            // indexing out of range.
            let end = start
                .checked_add(n_coeffs)
                .ok_or(CodecError::MalformedPacket)?;
            if end > spec.len() {
                return Err(CodecError::MalformedPacket);
            }
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
pub struct CinderDecoderFactory;
impl DynDecoderFactory for CinderDecoderFactory {
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
        let cfg2 = CinderEncoderConfig::default();
        let decoder = CinderDecoder::new(cfg.clone(), cfg2)?;
        Ok(alloc::boxed::Box::new(CinderDynDecoder { inner: decoder }))
    }
}

/// Type-erased wrapper around [`CinderDecoder`].
pub struct CinderDynDecoder {
    inner: CinderDecoder,
}
impl CinderDynDecoder {
    /// Wrap a concrete decoder for direct type-erased use (the FFI
    /// bridge, custom registries). The factory route stays
    /// available for config-from-codec-private construction.
    pub fn new(inner: CinderDecoder) -> Self {
        Self { inner }
    }
}

impl DynDecoder for CinderDynDecoder {
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
pub struct CinderEncoderFactory;
impl DynEncoderFactory for CinderEncoderFactory {
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
        let cfg2 = CinderEncoderConfig::default();
        let enc = CinderEncoder::new(cfg.clone(), cfg2)?;
        Ok(alloc::boxed::Box::new(CinderDynEncoder { inner: enc }))
    }
}

/// Type-erased wrapper around [`CinderEncoder`].
pub struct CinderDynEncoder {
    inner: CinderEncoder,
}
impl CinderDynEncoder {
    /// Wrap a concrete encoder for direct type-erased use (the FFI
    /// bridge, custom registries). The factory route stays
    /// available for config-from-codec-private construction.
    pub fn new(inner: CinderEncoder) -> Self {
        Self { inner }
    }
}

impl DynEncoder for CinderDynEncoder {
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
            _ => return Err(CodecError::Unsupported("Cinder expects audio frames")),
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
        let enc_cfg = CinderEncoderConfig {
            frame_size: 128,
            n_bands: 4,
            band_steps: alloc::vec![1.0; 4],
            ..Default::default()
        };
        let mut enc = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();
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
        let enc_cfg = CinderEncoderConfig {
            frame_size: 128,
            n_bands: 8,
            band_steps: alloc::vec![50.0; 8],
            bits_per_coef: 8,
            ..Default::default()
        };
        let mut enc = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();

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
        let enc_cfg = CinderEncoderConfig::default();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();
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
        let enc_cfg = CinderEncoderConfig::default();
        let mut enc = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();

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
        let enc_cfg = CinderEncoderConfig {
            frame_size: 333,
            ..Default::default()
        };
        assert!(CinderEncoder::new(cfg.clone(), enc_cfg.clone()).is_err());
    }

    #[test]
    fn factory_builds_decoder() {
        let f = CinderDecoderFactory;
        assert_eq!(f.codec_id(), b"CIND");
        let dec = f.build(&CodecConfig::default()).unwrap();
        assert_eq!(dec.info().codec_id, CODEC_ID);
    }

    #[test]
    fn decode_respects_logical_allocation_limits() {
        // §19: a decoder must refuse work whose internal buffers exceed the
        // caller's configured `ResourceLimits`, not just packet size. Same
        // packet as the passing round-trip, but a tight allocation budget.
        let enc_cfg = CinderEncoderConfig {
            frame_size: 128,
            n_bands: 4,
            band_steps: alloc::vec![1.0; 4],
            ..Default::default()
        };
        let mut encoder = CinderEncoder::new(CodecConfig::default(), enc_cfg.clone()).unwrap();
        let mut frame = make_silence_frame(256);
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));
        let packets = encoder.encode(&frame).unwrap();
        assert_eq!(packets.len(), 1);

        let tight = CodecConfig {
            limits: tpt_media_codec::ResourceLimits {
                max_allocation_bytes: 128,
                ..tpt_media_codec::ResourceLimits::default()
            },
            ..CodecConfig::default()
        };
        let mut dec = CinderDecoder::new(tight, enc_cfg).unwrap();
        assert_eq!(
            dec.decode(&packets[0]).unwrap_err(),
            CodecError::ResourceLimit(tpt_media_codec::ResourceLimitKind::Allocation)
        );
    }

    #[test]
    fn factory_builds_encoder() {
        let f = CinderEncoderFactory;
        let enc = f.build(&CodecConfig::default()).unwrap();
        assert_eq!(enc.info().codec_id, CODEC_ID);
    }

    fn make_tone_audio_frame(n: usize, pts_ticks: u64) -> AudioFrame {
        let mut f = AudioFrame::alloc(n as u32, SampleFormat::S16, ChannelLayout::Mono, 48_000);
        for (i, s) in f.samples.chunks_mut(2).enumerate() {
            let v = (0.9 * (i as f32 * 0.11).sin() * i16::MAX as f32) as i16;
            s.copy_from_slice(&v.to_le_bytes());
        }
        f.frame.pts = Timestamp {
            ticks: pts_ticks as i64,
            timebase: timebase_48k(),
        };
        f
    }

    fn timebase_48k() -> Timebase {
        Timebase::new(1, 48_000)
    }

    fn frame_s16_energy(frame: &AudioFrame) -> i64 {
        (0..frame.sample_count as usize)
            .map(|i| {
                let v = frame.s16(0, i).unwrap() as i64;
                v * v
            })
            .sum()
    }

    #[test]
    fn plc_conceals_mid_stream_packet_loss() {
        // End-to-end drop-and-conceal: encode a steady tone, lose the
        // middle packet, substitute `conceal()` at its slot, and confirm
        // the stream survives — concealed frame has the right size/PTS and
        // carries energy (it reuses the last decoded spectrum), and both
        // the packet before and after the gap still decode cleanly.
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 128,
            n_bands: 8,
            band_steps: alloc::vec![12.0; 8],
            bits_per_coef: 8,
            ..Default::default()
        };
        let fs = 128usize;
        let frame_len = 2 * fs;
        let mut dec = CinderDecoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut encoder = CinderEncoder::new(cfg, enc_cfg).unwrap();

        let mut packets = alloc::vec![];
        for frame_idx in 0..4 {
            let pts = frame_idx as u64 * frame_len as u64;
            let frame = make_tone_audio_frame(frame_len, pts);
            packets.extend(encoder.encode(&frame).unwrap());
        }
        assert_eq!(packets.len(), 4);

        for (idx, pkt) in packets.iter().enumerate() {
            if idx == 1 {
                continue;
            }
            let decoded = dec.decode(pkt).unwrap();
            assert_eq!(decoded.len(), 1);
            assert_eq!(decoded[0].sample_count as usize, frame_len);
            assert_eq!(decoded[0].frame.pts, pkt.pts);
        }

        let lost_pts = packets[1].pts;
        let concealed = dec.conceal(lost_pts).unwrap();
        assert_eq!(concealed.sample_count as usize, frame_len);
        assert_eq!(concealed.frame.pts, lost_pts);
        assert!(
            frame_s16_energy(&concealed) > 0,
            "concealed frame must carry energy (realised from last decoded spectrum)"
        );

        let tail = dec.decode(&packets[2]).unwrap();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].sample_count as usize, frame_len);
        assert_eq!(tail[0].frame.pts, packets[2].pts);
    }

    #[test]
    fn jittered_pts_are_preserved_not_reordered() {
        // Deliver valid packets out of PTS order (arrival: 0, 2, 3, 1) as a
        // jitter-bound real-time transport might. The decoder must not
        // panic, must not reorder or drop on its own, and must echo each
        // packet's own PTS back on its output frame.
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 128,
            n_bands: 8,
            band_steps: alloc::vec![12.0; 8],
            bits_per_coef: 8,
            ..Default::default()
        };
        let fs = 128usize;
        let frame_len = 2 * fs;
        let mut dec = CinderDecoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut encoder = CinderEncoder::new(cfg, enc_cfg).unwrap();

        let mut packets = alloc::vec![];
        for frame_idx in 0..4 {
            let pts = frame_idx as u64 * frame_len as u64;
            let frame = make_tone_audio_frame(frame_len, pts);
            packets.extend(encoder.encode(&frame).unwrap());
        }
        let arrival_order = [0usize, 2, 3, 1];
        for &idx in &arrival_order {
            let decoded = dec.decode(&packets[idx]).unwrap();
            assert_eq!(decoded.len(), 1);
            assert_eq!(decoded[0].frame.pts, packets[idx].pts);
            assert_eq!(decoded[0].sample_count as usize, frame_len);
            assert!(
                frame_s16_energy(&decoded[0]) > 0,
                "frame {idx} must decode with energy despite out-of-order arrival"
            );
        }
    }

    /// Parse a Cinder packet's header far enough to recover the transmitted
    /// per-band step values (skipping over each band's coefficients),
    /// without going through a full decode.
    fn extract_band_steps(pkt: &Packet, bits_per_coef: u32) -> alloc::vec::Vec<u16> {
        let mut r = BitReader::new(&pkt.data);
        let _wire_version = r.read(16).unwrap();
        let _fs_code = r.read(1).unwrap();
        let n_bands = r.read(8).unwrap() as usize;
        let mut steps = alloc::vec::Vec::with_capacity(n_bands);
        for _ in 0..n_bands {
            let step_bits = r.read(16).unwrap() as u16;
            steps.push(step_bits);
            let n_coeffs = r.read(8).unwrap() as u32;
            r.skip(n_coeffs * bits_per_coef).unwrap();
        }
        steps
    }

    #[test]
    fn perceptual_weighting_off_by_default_leaves_step_unchanged() {
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 128,
            n_bands: 8,
            band_steps: alloc::vec![12.0; 8],
            bits_per_coef: 8,
            ..Default::default()
        };
        assert!(!enc_cfg.perceptual_weighting);
        let mut enc = CinderEncoder::new(cfg, enc_cfg.clone()).unwrap();
        let frame = make_tone_audio_frame(256, 0);
        let packet = enc.encode(&frame).unwrap().into_iter().next().unwrap();
        let steps = extract_band_steps(&packet, enc_cfg.bits_per_coef as u32);
        assert_eq!(
            steps,
            alloc::vec![12u16; 8],
            "with perceptual_weighting off, every band must keep its configured step exactly"
        );
    }

    #[test]
    fn perceptual_weighting_coarsens_less_sensitive_bands_never_below_configured() {
        // §29: "optional perceptual weighting". A band covering ~22kHz
        // (far less sensitive than the 1kHz reference) must end up with a
        // coarser step than the configured baseline, while no band is
        // ever sharpened below what was configured (the wire format's
        // step field is an integer, so there's no sub-configured
        // granularity to sharpen into) — see `perceptual_band_weights`.
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 128,
            n_bands: 8,
            band_steps: alloc::vec![12.0; 8],
            bits_per_coef: 8,
            perceptual_weighting: true,
            ..Default::default()
        };
        let mut enc = CinderEncoder::new(cfg, enc_cfg.clone()).unwrap();
        let frame = make_tone_audio_frame(256, 0);
        let packet = enc.encode(&frame).unwrap().into_iter().next().unwrap();
        let steps = extract_band_steps(&packet, enc_cfg.bits_per_coef as u32);

        assert!(
            steps.iter().all(|&s| s >= 12),
            "perceptual weighting must never coarsen below the configured step: {steps:?}"
        );
        assert!(
            steps.iter().any(|&s| s != steps[0]),
            "perceptual weighting must actually vary the step across a wide frequency span: {steps:?}"
        );
    }

    #[test]
    fn perceptual_weighting_round_trip_carries_signal() {
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 128,
            n_bands: 8,
            band_steps: alloc::vec![12.0; 8],
            bits_per_coef: 8,
            perceptual_weighting: true,
            ..Default::default()
        };
        let mut enc = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();
        let frame = make_tone_audio_frame(256, 0);
        let packet = enc.encode(&frame).unwrap().into_iter().next().unwrap();
        let decoded = dec.decode(&packet).unwrap();
        assert_eq!(decoded.len(), 1);
        assert!(
            frame_s16_energy(&decoded[0]) > 0,
            "decoded tone must not be silent with perceptual weighting enabled"
        );
    }

    #[test]
    fn decode_rejects_unsupported_wire_version() {
        // §35: a decoder must fail cleanly on a wire version it doesn't
        // understand, rather than silently misinterpreting the rest of
        // the packet as if it were this decoder's own layout.
        let mut buf = alloc::vec![0u8; 4];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            w.write(16, 99).unwrap(); // bogus/future version
            w.write(1, 0).unwrap(); // frame-size selector; never reached
            w.write(8, 0).unwrap();
            w.finish().unwrap()
        };
        buf.truncate(written);
        let pkt = Packet {
            data: buf,
            pts: Timestamp::zero(Timebase::new(1, 48_000)),
            dts: Timestamp::zero(Timebase::new(1, 48_000)),
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        let mut dec =
            CinderDecoder::new(CodecConfig::default(), CinderEncoderConfig::default()).unwrap();
        assert_eq!(
            dec.decode(&pkt).unwrap_err(),
            CodecError::Unsupported("unsupported Cinder packet version")
        );
    }

    #[test]
    fn decode_rejects_n_coeffs_that_would_overrun_the_spectrum() {
        // Regression: `n_coeffs` (and `n_bands`) come straight from the
        // packet with no bound relative to `frame_size`. A single band
        // claiming more coefficients than fit in the whole spectrum used
        // to index `spec` out of bounds and panic instead of returning
        // `MalformedPacket`. Buffer is sized generously so the failure
        // path being tested is the bounds check itself, not an
        // incidental end-of-input a few bits earlier.
        let mut buf = alloc::vec![0u8; 8];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            w.write(16, VERSION as u64).unwrap();
            w.write(1, 0).unwrap(); // frame_size = 128
            w.write(8, 1).unwrap(); // n_bands = 1 (band_size = 128)
            w.write(16, 12).unwrap(); // step
            w.write(8, 200).unwrap(); // n_coeffs: far more than band_size
            w.finish().unwrap()
        };
        buf.truncate(written);
        let pkt = Packet {
            data: buf,
            pts: Timestamp::zero(timebase_48k()),
            dts: Timestamp::zero(timebase_48k()),
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        let mut dec =
            CinderDecoder::new(CodecConfig::default(), CinderEncoderConfig::default()).unwrap();
        assert_eq!(dec.decode(&pkt).unwrap_err(), CodecError::MalformedPacket);
    }

    #[test]
    fn decode_rejects_zero_bands() {
        let mut buf = alloc::vec![0u8; 4];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            w.write(16, VERSION as u64).unwrap();
            w.write(1, 0).unwrap();
            w.write(8, 0).unwrap(); // n_bands = 0
            w.finish().unwrap()
        };
        buf.truncate(written);
        let pkt = Packet {
            data: buf,
            pts: Timestamp::zero(timebase_48k()),
            dts: Timestamp::zero(timebase_48k()),
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        let mut dec =
            CinderDecoder::new(CodecConfig::default(), CinderEncoderConfig::default()).unwrap();
        assert_eq!(dec.decode(&pkt).unwrap_err(), CodecError::MalformedPacket);
    }

    #[test]
    fn decode_never_panics_on_corrupted_or_arbitrary_packets() {
        // §38 "codec fuzzing" / "malformed input": a deterministic
        // corruption sweep, in the same spirit as `tpt-container`'s
        // `corruption_never_panics_and_truncation_never_panics` (§24) —
        // no external fuzz-harness dependency. Every call must return a
        // `Result`, never panic.
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 128,
            n_bands: 8,
            band_steps: alloc::vec![12.0; 8],
            bits_per_coef: 8,
            ..Default::default()
        };
        let mut encoder = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();
        let mut state = 0xC0FF_EE12_3456_789Au64;
        let mut lcg = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            state
        };
        let pts = Timestamp::zero(timebase_48k());

        // Arbitrary bytes of varying length, unrelated to any real packet.
        for _ in 0..500 {
            let len = (lcg() % 300) as usize;
            let data: Vec<u8> = (0..len).map(|_| lcg() as u8).collect();
            let pkt = Packet {
                data,
                pts,
                dts: pts,
                flags: PacketFlags::KEYFRAME,
                track_id: 0,
                duration: None,
            };
            let _ = dec.decode(&pkt);
        }

        // A real valid packet, truncated at every length.
        let frame = make_tone_audio_frame(256, 0);
        let valid = encoder.encode(&frame).unwrap().into_iter().next().unwrap();
        for cut in 0..=valid.data.len() {
            let pkt = Packet {
                data: valid.data[..cut].to_vec(),
                ..valid.clone()
            };
            let _ = dec.decode(&pkt);
        }
        // Bit-flip sweep over the valid packet.
        for _ in 0..256 {
            if valid.data.is_empty() {
                break;
            }
            let pos = (lcg() as usize) % valid.data.len();
            let bit = 1u8 << (lcg() % 8);
            let mut corrupt = valid.clone();
            corrupt.data[pos] ^= bit;
            let _ = dec.decode(&corrupt);
        }
        // The pristine packet still decodes after the sweep.
        assert!(dec.decode(&valid).is_ok());
    }
}
