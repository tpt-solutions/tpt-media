//! `tpt-audio-cinder` — Cinder: low-latency real-time audio codec.
//!
//! ## Algorithm
//!
//! - Frame sizes: 128 or 256 samples (default 256).
//! - Transform: MDCT with sine window, hop `frame_size` (50% overlap). The
//!   decoder overlap-adds consecutive inverse blocks, so time-domain
//!   aliasing cancels (TDAC) and the reconstruction is not limited by the
//!   window edges (see `docs/codec-design.md`, "Cinder v2").
//! - Quantisation: uniform scalar with per-band step size, with an
//!   optional perceptual reweighting (see [`CinderEncoderConfig::perceptual_weighting`]).
//! - Entropy: fixed-width bit fields (uniform quantiser index).
//! - PLC: phase-randomised sign perturbation of previous frame's MDCT.
//!
//! ## Framing and latency
//!
//! One packet carries one `frame_size`-coefficient spectrum and is emitted
//! for every `frame_size` input samples. The decoder completes a block only
//! when the packet after it arrives, so the first packet of a stream emits
//! no samples and the stream has one block of latency. Each packet is
//! stamped with the start of the output it yields, so a decoded frame's
//! timestamp is its packet's timestamp. The encoder's [`Encoder::drain`]
//! zero-pads the last block and appends the silent blocks needed to
//! complete every received sample, so decoded output has exactly the input
//! length, up to the zero padding of the final block.
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
//! never sharpens it below its configured value. The wire carries each
//! band's step in `1 / STEP_UNIT` fixed point, so the effective step is
//! rounded to that grid before quantising. Zero added real-time cost, and
//! no extra side information, since Cinder already transmits the actual
//! per-band step used.
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

/// Bitstream version. Version 2 changed the meaning of a packet: hop-`frame_size`
/// MDCT blocks that the decoder overlap-adds. Version 3 changes the meaning of
/// the per-band step field: it is now a fixed-point value in units of
/// `1 / STEP_UNIT` (it was a whole number, so every step below 1.0 was
/// unrepresentable). A packet from either earlier version would be
/// misreconstructed, so both are rejected rather than guessed at.
pub const VERSION: u16 = 4;

/// Rice parameter width in the packet header: `k` is in `0..=7`.
const RICE_K_BITS: u32 = 3;

/// A Rice unary prefix of this many ones is the escape: the level's code
/// follows as a raw `bits_per_coef`-bit value instead. Keeps the worst case
/// bounded, so no packet can run past its buffer or the decoder's limits.
const UNARY_LIMIT: u32 = 16;

/// Zigzag map a signed level onto an unsigned code: 0, -1, 1, -2, ... become
/// 0, 1, 2, 3, ...
fn zigzag(q: i32) -> u32 {
    if q >= 0 {
        (q as u32) << 1
    } else {
        ((-q) as u32) * 2 - 1
    }
}

fn unzigzag(u: u32) -> i32 {
    if u & 1 == 0 {
        (u >> 1) as i32
    } else {
        -(((u + 1) >> 1) as i32)
    }
}

/// Bits a Rice code of `u` with parameter `k` takes (escape included).
fn rice_cost(u: u32, k: u32, escape_bits: u32) -> u64 {
    let prefix = u >> k;
    if prefix < UNARY_LIMIT {
        prefix as u64 + 1 + k as u64
    } else {
        UNARY_LIMIT as u64 + escape_bits as u64
    }
}

/// Units per 1.0 of a band's quantisation step on the wire. Coefficients
/// of normalised input have a median magnitude near 0.003 and a 99th
/// percentile near 1, so a 1/256 unit (which cannot go below 0.0039) cannot
/// reach the fine steps a rate-quality curve needs. A `u16` at 1/1024 spans
/// steps from 0.001 to 64.
pub const STEP_UNIT: f32 = 1024.0;

/// Smallest and largest per-band step the wire can carry.
const STEP_RANGE: core::ops::RangeInclusive<f32> = (1.0 / STEP_UNIT)..=(u16::MAX as f32 / STEP_UNIT);

/// Inclusive range of supported `bits_per_coef` values.
const BITS_PER_COEF: core::ops::RangeInclusive<u8> = 4..=12;

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

/// Timestamp `samples` after (`forward`) or before `ts`, on the sample
/// timeline at `rate`. Exact for the usual `1/rate` timebase; other
/// timebases are converted with truncating division.
fn shift_by_samples(ts: Timestamp, samples: u64, rate: u32, forward: bool) -> Timestamp {
    let tb = ts.timebase;
    let delta = (u128::from(samples) * u128::from(tb.den))
        / (u128::from(tb.num.max(1)) * u128::from(rate.max(1)));
    let delta = delta as i64;
    let ticks = if forward {
        ts.ticks.saturating_add(delta)
    } else {
        ts.ticks.saturating_sub(delta)
    };
    Timestamp {
        ticks,
        timebase: tb,
    }
}

/// Build an S16 mono frame from float samples in `[-1, 1]`, clamped.
fn audio_frame(samples: &[f32], pts: Timestamp, sample_rate: u32) -> AudioFrame {
    let mut bytes = vec![0u8; samples.len() * 2];
    for (i, &v) in samples.iter().enumerate() {
        let s = (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        bytes[2 * i..2 * i + 2].copy_from_slice(&s.to_le_bytes());
    }
    AudioFrame {
        frame: Frame {
            pts,
            duration: None,
            side_data: Vec::new(),
        },
        samples: bytes,
        sample_count: samples.len() as u32,
        format: SampleFormat::S16,
        channel_layout: ChannelLayout::Mono,
        sample_rate,
    }
}

/// Cinder encoder configuration.
#[derive(Clone, Debug)]
pub struct CinderEncoderConfig {
    /// MDCT block size in samples: 128 or 256. The encoder emits one packet
    /// per `frame_size` input samples; each packet's analysis window spans
    /// `2·frame_size` samples (the current block plus the previous one).
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
    /// `--quality` mapping target). Levels are clamped to the signed range
    /// this width can carry.
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
///
/// Hop-`N` analysis: each block windows the previous block's `N` samples
/// followed by its own `N` new samples, so consecutive windows overlap by
/// half and the decoder's overlap-add cancels the time-domain aliasing.
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
    /// The `2N`-point sine analysis window, built once at construction.
    window: Vec<f32>,
    /// The previous block's `N` samples: the first half of the next
    /// window. Zeros before the first block.
    history: Vec<f32>,
    /// Received samples not yet forming a full block (fewer than `N`
    /// between calls).
    pending: Vec<f32>,
    /// Timestamp of the first input sample of the stream, set from the
    /// first frame received. Packet timestamps are derived from it (see
    /// [`CinderEncoder::encode_block`]).
    origin: Option<Timestamp>,
    /// Blocks emitted since the stream started or was last flushed.
    blocks: u64,
    /// Samples received since the stream started or was last flushed,
    /// excluding flush padding.
    received: u64,
}

impl CinderEncoder {
    /// Build an encoder for `enc` under the framework configuration
    /// (`version` + resource limits); validates frame size, band
    /// count, bit depth, and step-table length before anything is allocated.
    pub fn new(codec_cfg: CodecConfig, enc: CinderEncoderConfig) -> CodecResult<Self> {
        if enc.frame_size != 128 && enc.frame_size != 256 {
            return Err(CodecError::InvalidConfig("frame_size must be 128 or 256"));
        }
        if enc.band_steps.len() != enc.n_bands as usize {
            return Err(CodecError::InvalidConfig(
                "band_steps length must equal n_bands",
            ));
        }
        if !BITS_PER_COEF.contains(&enc.bits_per_coef) {
            return Err(CodecError::InvalidConfig("bits_per_coef must be 4..=12"));
        }
        if !enc.band_steps.iter().all(|s| STEP_RANGE.contains(s)) {
            return Err(CodecError::InvalidConfig(
                "band_steps must be within 1/1024..=64",
            ));
        }
        let n = enc.frame_size as usize;
        let mdct_plan = FastMdctPlan::new(n);
        let perceptual_weights = perceptual_band_weights(
            enc.perceptual_weighting,
            n,
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
            window: sine_window(2 * n),
            history: vec![0.0; n],
            pending: Vec::with_capacity(2 * n),
            origin: None,
            blocks: 0,
            received: 0,
        })
    }

    /// Return to the state of a freshly built encoder, ready for a new
    /// stream. The configuration and precomputed tables are kept.
    fn reset_stream(&mut self) {
        self.history.iter_mut().for_each(|s| *s = 0.0);
        self.pending.clear();
        self.origin = None;
        self.blocks = 0;
        self.received = 0;
    }

    /// Encode the first `N` pending samples as one block. The caller
    /// guarantees at least `N` are pending.
    ///
    /// The packet is stamped with the start of the output it yields, the
    /// same convention every codec here uses for output frames. Packet `k`
    /// completes the samples from `(k - 1)·N`, so it carries
    /// `origin + (k - 1)·N`. The first packet yields nothing and takes the
    /// origin itself, which keeps every timestamp non-negative.
    fn encode_block(&mut self) -> CodecResult<Packet> {
        let n = self.enc.frame_size as usize;
        let origin = self.origin.ok_or(CodecError::Internal)?;
        let completed_from = self.blocks.saturating_sub(1) * n as u64;
        let pts = shift_by_samples(origin, completed_from, self.enc.sample_rate, true);
        let mut x = vec![0f32; 2 * n];
        for i in 0..n {
            x[i] = self.history[i] * self.window[i];
            x[n + i] = self.pending[i] * self.window[n + i];
        }
        self.history.copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        let mut spec = vec![0f32; n];
        self.mdct_plan.forward(&x, &mut spec);
        self.last_spectrum = spec.clone();
        let n_bands = self.enc.n_bands as usize;
        let band_size = (spec.len() / n_bands).max(1);
        let coded = n_bands * band_size;
        let bits = self.enc.bits_per_coef as u32;
        let max_level = (1i32 << (bits - 1)) - 1;
        // Perceptual weighting can only ever *coarsen* a band's step (weight
        // > 1, for less-sensitive bands), never sharpen it below its
        // configured value. Each step is rounded to the wire's fixed-point
        // grid, and the coefficients are quantised with that rounded value,
        // so the decoder reconstructs at exactly the step the encoder used.
        let step_bits: Vec<u16> = (0..n_bands)
            .map(|band| {
                let effective_step = (self.enc.band_steps[band] * self.perceptual_weights[band])
                    .max(self.enc.band_steps[band]);
                (effective_step * STEP_UNIT).round().clamp(1.0, 65535.0) as u16
            })
            .collect();
        let levels: Vec<i32> = (0..coded)
            .map(|i| {
                let step = step_bits[i / band_size] as f32 / STEP_UNIT;
                quantize(spec[i], step).clamp(-max_level, max_level)
            })
            .collect();
        // One Rice parameter per packet, chosen to minimise its exact cost.
        let k = (0..(1u32 << RICE_K_BITS))
            .min_by_key(|&k| levels.iter().map(|&q| rice_cost(zigzag(q), k, bits)).sum::<u64>())
            .unwrap_or(0);
        // Upper bound: header (version, frame size, band count, step mode,
        // up to one 16-bit step per band, Rice parameter), then the worst
        // case per level (the escape), then the trailing keyframe bit.
        let max_bits = 16
            + 1
            + 8
            + 1
            + 16 * n_bands
            + RICE_K_BITS as usize
            + coded * (UNARY_LIMIT as usize + bits as usize)
            + 1
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
            w.write(8, n_bands as u64).map_err(|_| CodecError::Internal)?;
            // Step mode: 0 = one step for every band (the common case, and the
            // MSE-optimal one), 1 = a 16-bit step per band.
            let uniform = step_bits.iter().all(|&s| s == step_bits[0]);
            w.write(1, if uniform { 0 } else { 1 })
                .map_err(|_| CodecError::Internal)?;
            let steps: &[u16] = if uniform { &step_bits[..1] } else { &step_bits };
            for &s in steps {
                w.write(16, s as u64).map_err(|_| CodecError::Internal)?;
            }
            w.write(RICE_K_BITS, k as u64)
                .map_err(|_| CodecError::Internal)?;
            for &q in &levels {
                let u = zigzag(q);
                let prefix = u >> k;
                if prefix < UNARY_LIMIT {
                    for _ in 0..prefix {
                        w.write(1, 1).map_err(|_| CodecError::Internal)?;
                    }
                    w.write(1, 0).map_err(|_| CodecError::Internal)?;
                    if k > 0 {
                        w.write(k, (u & ((1 << k) - 1)) as u64)
                            .map_err(|_| CodecError::Internal)?;
                    }
                } else {
                    for _ in 0..UNARY_LIMIT {
                        w.write(1, 1).map_err(|_| CodecError::Internal)?;
                    }
                    w.write(bits, u as u64).map_err(|_| CodecError::Internal)?;
                }
            }
            w.write(1, 1).map_err(|_| CodecError::Internal)?;
            w.finish().map_err(|_| CodecError::Internal)?
        };
        buf.truncate(written);

        self.blocks += 1;
        Ok(Packet {
            data: buf,
            pts,
            dts: pts,
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
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
        let count = frame.sample_count as usize;
        if frame.samples.len() < 2 * count {
            return Err(CodecError::InvalidConfig(
                "frame has fewer bytes than its sample_count",
            ));
        }
        if self.origin.is_none() {
            self.origin = Some(frame.frame.pts);
        }
        self.pending.extend(
            frame.samples[..2 * count]
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / i16::MAX as f32),
        );
        self.received += count as u64;
        let n = self.enc.frame_size as usize;
        let mut packets = Vec::new();
        while self.pending.len() >= n {
            packets.push(self.encode_block()?);
        }
        Ok(packets)
    }

    /// End the stream gracefully: the partial last block is zero-padded,
    /// then silent blocks are appended until the decoder has completed
    /// every received sample (it completes block `j - 1` only when packet
    /// `j` arrives). Encoder state is kept, so a repeated call emits
    /// nothing new.
    fn drain(&mut self) -> CodecResult<Vec<Packet>> {
        let mut packets = Vec::new();
        if self.received == 0 {
            return Ok(packets);
        }
        let n = self.enc.frame_size as usize;
        if !self.pending.is_empty() {
            self.pending.resize(n, 0.0);
            packets.push(self.encode_block()?);
        }
        while self.blocks.saturating_sub(1) * (n as u64) < self.received {
            self.pending.resize(n, 0.0);
            packets.push(self.encode_block()?);
        }
        Ok(packets)
    }

    /// Hard reset: emit the same tail as [`Encoder::drain`], then discard
    /// the stream state so the encoder can start an unrelated stream.
    fn flush(&mut self) -> CodecResult<Vec<Packet>> {
        let packets = Encoder::drain(self)?;
        self.reset_stream();
        Ok(packets)
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
    /// Second half of the previous windowed inverse block, overlap-added
    /// into the next block's first half. Zeros before the first packet.
    tail: Vec<f32>,
    /// Whether a packet has been decoded. The first packet's block lies
    /// before the stream, so it completes nothing and emits no samples.
    primed: bool,
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
        if !BITS_PER_COEF.contains(&enc.bits_per_coef) {
            return Err(CodecError::InvalidConfig("bits_per_coef must be 4..=12"));
        }
        Ok(CinderDecoder {
            info: static_info(Capabilities {
                real_time_decoder: true,
                ..Capabilities::default()
            }),
            cfg: codec_cfg,
            tail: vec![0.0; enc.frame_size as usize],
            enc,
            last_spectrum: Vec::new(),
            plc_phase: 0,
            mdct_plan_128: FastMdctPlan::new(FRAME_128 as usize),
            mdct_plan_256: FastMdctPlan::new(FRAME_256 as usize),
            primed: false,
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

    /// Windowed inverse of one spectrum: `2N` samples, ready to overlap-add.
    fn synthesize(&mut self, spec: &[f32]) -> Vec<f32> {
        let two_n = 2 * spec.len();
        let mut x = vec![0f32; two_n];
        self.mdct_plan_for(spec.len()).inverse(spec, &mut x);
        let w = sine_window(two_n);
        for (xi, wi) in x.iter_mut().zip(w.iter()) {
            *xi *= *wi;
        }
        x
    }

    /// Overlap-add one windowed inverse block (`2N` samples). Returns the
    /// completed block of `N` samples and keeps the second half as the next
    /// tail. A change of frame size restarts the overlap from silence.
    fn overlap_add(&mut self, y: &[f32]) -> Vec<f32> {
        let n = y.len() / 2;
        if self.tail.len() != n {
            self.tail = vec![0.0; n];
        }
        let block: Vec<f32> = (0..n).map(|i| y[i] + self.tail[i]).collect();
        self.tail.copy_from_slice(&y[n..]);
        block
    }

    /// Packet-loss concealment for the packet timestamped `pts`: synthesise
    /// the block that packet would have carried, from the last decoded
    /// spectrum, and overlap-add it so the stream continues without a
    /// seam. The returned frame carries `pts`, as a received packet's frame
    /// would.
    pub fn conceal(&mut self, pts: Timestamp) -> CodecResult<AudioFrame> {
        let fs = self.enc.frame_size;
        let n = fs as usize;
        let mut spec = self.last_spectrum.clone();
        if spec.is_empty() {
            spec = alloc::vec![0.0; n];
        }
        for (i, s) in spec.iter_mut().enumerate() {
            let phase = self.plc_phase.wrapping_add(i as u32);
            if phase.wrapping_mul(2654435761) & 0x8000_0000 != 0 {
                *s = -*s;
            }
        }
        self.plc_phase = self.plc_phase.wrapping_add(1);
        let y = self.synthesize(&spec);
        let block = self.overlap_add(&y);
        self.primed = true;
        let rate = self.enc.sample_rate;
        Ok(audio_frame(&block, pts, rate))
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

    /// Hard reset: discard the overlap tail so the next packet starts a new
    /// stream. Nothing is held back, so there is nothing to drain.
    fn flush(&mut self) -> CodecResult<Vec<AudioFrame>> {
        self.tail.iter_mut().for_each(|s| *s = 0.0);
        self.primed = false;
        Ok(Vec::new())
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
        let coded = n_bands as usize * band_size;
        // A band count above the frame size floors `band_size` to one and
        // would claim more coefficients than the frame holds.
        if coded > frame_size as usize {
            return Err(CodecError::MalformedPacket);
        }
        let bits = self.enc.bits_per_coef as u32;
        let max_u = (1u32 << bits) - 2; // largest zigzag code a level can take
        let malformed = |_| CodecError::MalformedPacket;
        let uniform = r.read(1).map_err(malformed)? == 0;
        let n_steps = if uniform { 1 } else { n_bands as usize };
        let mut step_bits = Vec::with_capacity(n_steps);
        for _ in 0..n_steps {
            let s = r.read(16).map_err(malformed)? as u16;
            if s == 0 {
                // Zero is never written: the encoder clamps every step to at
                // least 1/STEP_UNIT, so a zero step is a malformed packet.
                return Err(CodecError::MalformedPacket);
            }
            step_bits.push(s);
        }
        let k = r.read(RICE_K_BITS).map_err(malformed)? as u32;
        let mut spec = vec![0f32; frame_size as usize];
        for i in 0..coded {
            let mut prefix = 0u32;
            while prefix < UNARY_LIMIT {
                if r.read(1).map_err(malformed)? == 0 {
                    break;
                }
                prefix += 1;
            }
            let u = if prefix < UNARY_LIMIT {
                let low = if k > 0 { r.read(k).map_err(malformed)? as u32 } else { 0 };
                (prefix << k) | low
            } else {
                r.read(bits).map_err(malformed)? as u32
            };
            if u > max_u {
                return Err(CodecError::MalformedPacket);
            }
            let s = if uniform { step_bits[0] } else { step_bits[i / band_size] };
            spec[i] = dequantize(unzigzag(u), s as f32 / STEP_UNIT);
        }
        let _keyframe = r.read(1).map_err(|_| CodecError::MalformedPacket)?;
        self.last_spectrum = spec.clone();
        let y = self.synthesize(&spec);
        let block = self.overlap_add(&y);
        if !self.primed {
            self.primed = true;
            return Ok(Vec::new());
        }
        // The packet's timestamp is the start of the block it completes
        // (see `CinderEncoder::encode_block`), so the output frame takes it
        // directly and its stamp depends only on this packet.
        Ok(vec![audio_frame(&block, packet.pts, self.enc.sample_rate)])
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
        Decoder::flush(&mut self.inner)?;
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

    /// Decode every packet in arrival order, collecting the frames emitted.
    fn decode_all(dec: &mut CinderDecoder, packets: &[Packet]) -> Vec<AudioFrame> {
        packets
            .iter()
            .flat_map(|p| dec.decode(p).expect("decode"))
            .collect()
    }

    fn total_samples(frames: &[AudioFrame]) -> usize {
        frames.iter().map(|f| f.sample_count as usize).sum()
    }

    /// SNR in dB of `test` against `reference`, both S16 mono frames
    /// concatenated in order, compared over the shorter of the two.
    fn snr_against(reference: &[i16], test: &[AudioFrame]) -> f64 {
        let decoded: Vec<f64> = test
            .iter()
            .flat_map(|f| (0..f.sample_count as usize).map(move |i| f.s16(0, i).unwrap() as f64))
            .collect();
        let n = reference.len().min(decoded.len());
        let mut sig = 0.0;
        let mut err = 0.0;
        for i in 0..n {
            let r = reference[i] as f64;
            sig += r * r;
            err += (r - decoded[i]).powi(2);
        }
        10.0 * (sig / err.max(1e-12)).log10()
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
        assert_eq!(packets.len(), 2, "one packet per 128-sample hop");
        assert!(packets[0].is_keyframe());
        // The first packet completes nothing; the second completes block 0.
        assert!(dec.decode(&packets[0]).unwrap().is_empty());
        let decoded = dec.decode(&packets[1]).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].sample_count, 128);
        assert_eq!(frame_s16_energy(&decoded[0]), 0);
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

        let frame = make_tone_audio_frame(1024, 0);
        let packets = enc.encode(&frame).unwrap();
        assert!(
            packets.iter().all(|p| !p.data.is_empty()),
            "encoded packets must not be empty"
        );

        let decoded = decode_all(&mut dec, &packets);
        let energy: i64 = decoded.iter().map(frame_s16_energy).sum();
        assert!(energy > 0, "decoded tone must not be silent");
    }

    #[test]
    fn fractional_step_round_trips_at_the_step_it_was_quantised_with() {
        // Regression test for the v2 wire desync: the encoder quantised at a
        // fractional step but wrote it truncated to an integer, and the
        // decoder reconstructed at that integer, so any step below 1.0
        // (or any fractional weighted step) decoded to near silence. A
        // correct round trip at this step is about 23 dB; a desync is ~0 dB.
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 256,
            n_bands: 16,
            band_steps: alloc::vec![0.05; 16],
            bits_per_coef: 12,
            ..Default::default()
        };
        let mut enc = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();
        let frame = make_tone_audio_frame(16_384, 0);
        let reference: Vec<i16> = (0..16_384).map(|i| frame.s16(0, i).unwrap()).collect();
        let mut all = enc.encode(&frame).unwrap();
        all.extend(enc.flush().unwrap());
        let snr = snr_against(&reference, &decode_all(&mut dec, &all));
        assert!(snr > 15.0, "fractional-step SNR {snr:.2} dB must clear 15 dB");
    }

    #[test]
    fn rejects_band_steps_the_wire_cannot_carry() {
        let cfg = CodecConfig::default();
        for bad in [0.0, -1.0, 0.0001, 65.0, f32::NAN, f32::INFINITY] {
            let enc_cfg = CinderEncoderConfig {
                band_steps: alloc::vec![bad; 16],
                ..Default::default()
            };
            assert!(
                CinderEncoder::new(cfg.clone(), enc_cfg).is_err(),
                "band step {bad} must be rejected"
            );
        }
    }

    #[test]
    fn band_count_above_frame_size_is_malformed_not_a_panic() {
        // Regression: a header with more bands than bins (here 200 bands in a
        // 128-bin frame) once indexed past the spectrum. It must be rejected.
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig::default();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();
        let mut buf = vec![0u8; 64];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            w.write(16, VERSION as u64).unwrap();
            w.write(1, 0).unwrap(); // 128-bin frame
            w.write(8, 200).unwrap(); // 200 bands
            w.write(1, 0).unwrap(); // uniform steps
            w.write(16, 1024).unwrap();
            w.write(RICE_K_BITS, 0).unwrap();
            w.finish().unwrap()
        };
        let packet = Packet {
            data: buf[..written].to_vec(),
            pts: Timestamp::zero(Timebase::new(1, 48_000)),
            dts: Timestamp::zero(Timebase::new(1, 48_000)),
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        assert_eq!(dec.decode(&packet).unwrap_err(), CodecError::MalformedPacket);
    }

    #[test]
    fn rice_escape_round_trips_levels_past_the_unary_limit() {
        // Silence with a few full-scale clicks at a fine step: the silent
        // blocks pick a small Rice parameter, so the click coefficients
        // overflow the unary limit and take the raw escape code. Every packet
        // must still fit its buffer and decode back to the encoder's levels.
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 256,
            n_bands: 16,
            band_steps: alloc::vec![1.0 / STEP_UNIT; 16],
            bits_per_coef: 12,
            ..Default::default()
        };
        let mut enc = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();
        let mut f = AudioFrame::alloc(16_384, SampleFormat::S16, ChannelLayout::Mono, 48_000);
        for &at in &[1_000usize, 5_000, 9_000, 13_000] {
            f.samples[at * 2..at * 2 + 2].copy_from_slice(&i16::MAX.to_le_bytes());
        }
        let reference: Vec<i16> = (0..16_384).map(|i| f.s16(0, i).unwrap()).collect();
        let mut all = enc.encode(&f).unwrap();
        all.extend(enc.flush().unwrap());
        assert!(all.iter().all(|p| !p.data.is_empty()));
        let snr = snr_against(&reference, &decode_all(&mut dec, &all));
        assert!(snr > 30.0, "escape-coded SNR {snr:.2} dB must clear 30 dB");
    }

    #[test]
    fn rice_parameter_follows_the_packet_not_a_global_state() {
        // The stateless design: a packet decodes on its own, whatever came
        // before it. Decode the second packet of a stream with a fresh
        // decoder and it must match the in-order decode.
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 128,
            n_bands: 8,
            band_steps: alloc::vec![0.05; 8],
            bits_per_coef: 10,
            ..Default::default()
        };
        let mut enc = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let frame = make_tone_audio_frame(2048, 0);
        let packets = enc.encode(&frame).unwrap();
        assert!(packets.len() >= 3);
        let mut in_order = CinderDecoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut spectra = Vec::new();
        for p in &packets {
            in_order.decode(p).unwrap();
            spectra.push(in_order.last_spectrum.clone());
        }
        // Drop packets 0 and 1 (lost) and decode packet 2 with a fresh decoder.
        // Its coefficients must match what the in-order decoder recovered:
        // the overlap-add tail is the only state a packet depends on.
        let mut after_loss = CinderDecoder::new(cfg, enc_cfg).unwrap();
        after_loss.decode(&packets[2]).unwrap();
        assert_eq!(after_loss.last_spectrum, spectra[2], "a later packet must decode without its predecessors");
    }

    #[test]
    fn overlap_add_beats_the_old_three_db_ceiling() {
        // The v1 decoder had no overlap-add, so every input decoded at about
        // 3 dB regardless of quantization. A tone at a fine step must now
        // reconstruct well above that ceiling.
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 256,
            n_bands: 16,
            band_steps: alloc::vec![1.0; 16],
            bits_per_coef: 10,
            ..Default::default()
        };
        let mut enc = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();
        let frame = make_tone_audio_frame(16_384, 0);
        let reference: Vec<i16> = (0..16_384).map(|i| frame.s16(0, i).unwrap()).collect();
        let packets = enc.encode(&frame).unwrap();
        let mut all = packets.clone();
        all.extend(enc.flush().unwrap());
        let frames = decode_all(&mut dec, &all);
        // The stream's first block is the first `N` reference samples.
        let snr = snr_against(&reference, &frames);
        assert!(snr > 10.0, "overlap-add SNR {snr:.2} dB must clear 10 dB");
    }

    #[test]
    fn plc_synthesises_a_frame() {
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig::default();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();
        let pts = Timestamp::zero(Timebase::new(1, 48_000));
        let f = dec.conceal(pts).unwrap();
        assert_eq!(f.sample_count, 256);
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

        let mut frame = AudioFrame::alloc(1024, SampleFormat::S16, ChannelLayout::Mono, 48_000);
        for (i, s) in frame.samples.chunks_mut(2).enumerate() {
            let v = (0.4 * (i as f32 * 0.07).sin() * i16::MAX as f32) as i16;
            s.copy_from_slice(&v.to_le_bytes());
        }
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));

        let packets = enc.encode(&frame).unwrap();
        assert_eq!(packets.len(), 4, "one packet per 256-sample hop");
        // Packet k yields the block from (k - 1) hops, so the first two
        // packets share the origin: packet 0 yields nothing.
        for (k, p) in packets.iter().enumerate() {
            let yields_from = k.max(1) - 1;
            assert_eq!(p.pts.ticks, (yields_from * 256) as i64, "packet {k} pts");
        }
        let decoded = decode_all(&mut dec, &packets);
        assert_eq!(decoded.len(), 3, "the first packet completes no block");
        assert!(decoded.iter().all(|f| f.sample_count == 256));
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
    fn rejects_bits_per_coef_outside_4_to_12() {
        for bits in [0u8, 3, 13, 255] {
            let enc_cfg = CinderEncoderConfig {
                bits_per_coef: bits,
                ..Default::default()
            };
            assert!(
                CinderEncoder::new(CodecConfig::default(), enc_cfg.clone()).is_err(),
                "encoder must reject {bits} bits"
            );
            assert!(
                CinderDecoder::new(CodecConfig::default(), enc_cfg).is_err(),
                "decoder must reject {bits} bits"
            );
        }
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
        assert_eq!(packets.len(), 2);

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
        // End-to-end drop-and-conceal: encode a steady tone, lose one
        // packet, substitute `conceal()` at its slot, and confirm the stream
        // survives — the concealed frame carries the lost packet's timestamp
        // and energy (it reuses the last decoded spectrum), and the packet
        // after the gap still decodes cleanly with its own timestamp.
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
        assert_eq!(packets.len(), 8);

        let lost = 3usize;
        let mut decoded = Vec::new();
        for (idx, pkt) in packets.iter().enumerate() {
            if idx == lost {
                continue;
            }
            let frames = dec.decode(pkt).unwrap();
            for f in &frames {
                assert_eq!(f.sample_count as usize, fs);
                assert_eq!(f.frame.pts, pkt.pts);
            }
            decoded.extend(frames);
        }
        assert_eq!(decoded.len(), 6, "the first packet completes nothing");

        let lost_pts = packets[lost].pts;
        let concealed = dec.conceal(lost_pts).unwrap();
        assert_eq!(concealed.sample_count as usize, fs);
        assert_eq!(concealed.frame.pts, lost_pts);
        assert!(
            frame_s16_energy(&concealed) > 0,
            "concealed frame must carry energy (realised from last decoded spectrum)"
        );

        let tail = dec.decode(&packets[lost + 1]).unwrap();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].sample_count as usize, fs);
        assert_eq!(tail[0].frame.pts, packets[lost + 1].pts);
    }

    #[test]
    fn jittered_pts_are_preserved_not_reordered() {
        // Deliver valid packets out of PTS order (arrival: 0, 2, 3, 1) as a
        // jitter-bound real-time transport might. The decoder must not
        // panic. Each emitted frame is stamped from the packet that carried
        // it, so the stamps do not depend on arrival order.
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
            if idx == 0 {
                assert!(decoded.is_empty(), "the first packet completes nothing");
                continue;
            }
            assert_eq!(decoded.len(), 1);
            assert_eq!(decoded[0].frame.pts, packets[idx].pts);
            assert_eq!(decoded[0].sample_count as usize, fs);
        }
    }

    /// Parse a Cinder packet's header far enough to recover the transmitted
    /// per-band step values (skipping over each band's coefficients),
    /// without going through a full decode.
    fn extract_band_steps(pkt: &Packet) -> alloc::vec::Vec<u16> {
        let mut r = BitReader::new(&pkt.data);
        let _wire_version = r.read(16).unwrap();
        let _fs_code = r.read(1).unwrap();
        let n_bands = r.read(8).unwrap() as usize;
        let uniform = r.read(1).unwrap() == 0;
        let count = if uniform { 1 } else { n_bands };
        let steps: alloc::vec::Vec<u16> = (0..count).map(|_| r.read(16).unwrap() as u16).collect();
        if uniform {
            alloc::vec![steps[0]; n_bands]
        } else {
            steps
        }
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
        let steps = extract_band_steps(&packet);
        assert_eq!(
            steps,
            alloc::vec![(12.0 * STEP_UNIT) as u16; 8],
            "with perceptual_weighting off, every band must keep its configured step exactly"
        );
    }

    #[test]
    fn perceptual_weighting_coarsens_less_sensitive_bands_never_below_configured() {
        // §29: "optional perceptual weighting". A band covering ~22kHz
        // (far less sensitive than the 1kHz reference) must end up with a
        // coarser step than the configured baseline, while no band is
        // ever sharpened below what was configured — see
        // `perceptual_band_weights`.
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
        let steps = extract_band_steps(&packet);

        assert!(
            steps.iter().all(|&s| s >= (12.0 * STEP_UNIT) as u16),
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
        let frame = make_tone_audio_frame(1024, 0);
        let packets = enc.encode(&frame).unwrap();
        let decoded = decode_all(&mut dec, &packets);
        let energy: i64 = decoded.iter().map(frame_s16_energy).sum();
        assert!(
            energy > 0,
            "decoded tone must not be silent with perceptual weighting enabled"
        );
    }

    #[test]
    fn low_bit_depth_round_trips_with_clamped_levels() {
        // Regression: the offset-binary level was written as `q + 128` into
        // `bits_per_coef` bits, so any depth below 8 wrapped and decoded to
        // garbage (negative SNR). Levels now clamp to the range the width
        // can carry, so a 4-bit tone must still reconstruct as a tone.
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 256,
            n_bands: 16,
            band_steps: alloc::vec![1.0; 16],
            bits_per_coef: 4,
            ..Default::default()
        };
        let mut enc = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();
        let frame = make_tone_audio_frame(4096, 0);
        let reference: Vec<i16> = (0..4096).map(|i| frame.s16(0, i).unwrap()).collect();
        let mut all = enc.encode(&frame).unwrap();
        all.extend(enc.flush().unwrap());
        let frames = decode_all(&mut dec, &all);
        let snr = snr_against(&reference, &frames);
        assert!(snr > 0.0, "4-bit SNR {snr:.2} dB must be positive");
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
    fn decode_rejects_version_1_packets() {
        // Version 1 used the same layout with non-overlapping blocks. Its
        // packets must be refused, not overlap-added as if they were v2.
        let mut buf = alloc::vec![0u8; 4];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            w.write(16, 1).unwrap();
            w.write(1, 0).unwrap();
            w.write(8, 1).unwrap();
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
        assert!(matches!(dec.decode(&pkt), Err(CodecError::Unsupported(_))));
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
    fn packet_count_and_length_are_exact_after_flush() {
        // One packet per hop, and once flushed the decoder returns every
        // received sample. Lengths that are not a multiple of the hop get
        // up to one hop of zero padding at the end, never less than input.
        for &len in &[512usize, 1000, 1024] {
            let cfg = CodecConfig::default();
            let enc_cfg = CinderEncoderConfig {
                frame_size: 128,
                n_bands: 8,
                band_steps: alloc::vec![4.0; 8],
                bits_per_coef: 10,
                ..Default::default()
            };
            let mut enc = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
            let mut dec = CinderDecoder::new(cfg, enc_cfg).unwrap();
            let frame = make_tone_audio_frame(len, 0);
            let mut packets = enc.encode(&frame).unwrap();
            packets.extend(enc.flush().unwrap());
            let frames = decode_all(&mut dec, &packets);
            let out = total_samples(&frames);
            assert!(out >= len, "len {len}: decoded {out} < input");
            assert!(
                out < len + 128,
                "len {len}: decoded {out} exceeds input by a hop"
            );
            if len % 128 == 0 {
                assert_eq!(out, len, "exact multiple of the hop must not pad");
            }
        }
    }

    #[test]
    fn flush_restarts_the_stream_cleanly() {
        // After a flush the encoder is a fresh stream: the same input must
        // give the same packets as a newly built encoder.
        let cfg = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            frame_size: 128,
            n_bands: 8,
            band_steps: alloc::vec![4.0; 8],
            bits_per_coef: 10,
            ..Default::default()
        };
        let frame = make_tone_audio_frame(700, 0);
        let mut fresh = CinderEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut reused = CinderEncoder::new(cfg, enc_cfg).unwrap();
        let mut expected = fresh.encode(&frame).unwrap();
        expected.extend(fresh.flush().unwrap());
        let _ = reused.encode(&frame).unwrap();
        let _ = reused.flush().unwrap();
        let mut actual = reused.encode(&frame).unwrap();
        actual.extend(reused.flush().unwrap());
        assert_eq!(actual.len(), expected.len());
        for (a, e) in actual.iter().zip(expected.iter()) {
            assert_eq!(a.data, e.data);
            assert_eq!(a.pts, e.pts);
        }
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
