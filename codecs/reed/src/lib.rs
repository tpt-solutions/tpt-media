//! `tpt-audio-reed` — Reed: extremely low-bandwidth speech codec.
//!
//! ## Design (v0.1)
//!
//! A source-filter (LPC vocoder) model of speech, in the spirit of the
//! classic 2.4 kbps LPC-10 analysis/synthesis codecs: the encoder models
//! each 20 ms frame as an all-pole spectral envelope (LPC analysis →
//! line spectral pair quantization) plus a low-rate excitation
//! description (voiced/unvoiced, pitch period, excitation gain), and the
//! decoder re-synthesizes speech by filtering a reconstructed excitation
//! through the quantized envelope. Quality is "robotic" by design — that
//! is the entire point of a ~4 kbps codec — but envelope and pitch are
//! preserved, which the test suite pins.
//!
//! Per 20 ms frame the encoder transmits:
//!
//! - **Spectral envelope** — order-p LPC (autocorrelation +
//!   Levinson-Durbin via `tpt-dsp-core`), converted to line spectral
//!   pairs ([`lpc_to_lsp`], which only exists for stable predictors) and
//!   uniformly quantized per coefficient; the decoder inverts with
//!   [`lsp_to_lpc`] after re-enforcing the strictly-increasing LSP
//!   ordering, so an arbitrary bit stream can never synthesize through an
//!   unstable filter. LSP representation is what makes coarse
//!   quantization + interpolation safe; raw LPC coefficients are neither.
//! - **Voicing + pitch** — `tpt-dsp-core`'s normalized autocorrelation
//!   pitch estimator ([`autocorrelation_pitch`], speech F0 range,
//!   confidence-thresholded) classifies the frame and estimates its
//!   period; the decoder drives an impulse train at that period.
//! - **Excitation gain** — the LPC residual's RMS, quantized in 1 dB
//!   steps over a 62 dB range. Index 0 is *exact mute*: the decoder
//!   emits digital silence, so all-zero input round-trips to all-zero
//!   output rather than to dithering noise.
//! - **Unvoiced excitation shape** — a shape-gain VQ over a fixed
//!   stochastic codebook of [`CODEBOOK_ENTRIES`] = 1024 unit-RMS
//!   noise shapes (the classic CELP fixed-codebook idea). The codebook
//!   is *not* transmitted: both encoder and decoder derive the identical
//!   entry for an index deterministically from a SplitMix64 stream (see
//!   [`codebook_entry`]), so it costs 10 bits per unvoiced frame and
//!   nothing else on the wire, and no training corpus is required —
//!   the entry distribution is defined by construction.
//!
//! Wire layout (bit-exact, little-endian bit packing by
//! `tpt-media-bitstream`): 16-bit version, 3-bit LPC-order code, 4-bit
//! per-coefficient LSF bit depth, 1-bit voicing, 6-bit gain index,
//! 10-bit pitch period (voiced) or codebook index (unvoiced), then
//! `order` LSF indices of `lsf_bits` bits each, then a CRC-32 trailer
//! over the preceding bytes (the same per-packet integrity convention
//! as Aura/Cinder). The default configuration's parameter payload is a
//! fixed 90 bits per 20 ms frame = **4.5 kbps** of coded parameters;
//! packets round up to 12 payload bytes and add the 4-byte CRC, so the
//! container-level rate (what `tpt probe` reports) is 16 bytes/frame =
//! 6.4 kbps.
//!
//! Packet loss: [`ReedDecoder::conceal`] repeats the last good frame's
//! envelope/voicing/pitch with the excitation gain decayed 6 dB per
//! concealed frame, so gaps fade out instead of buzzing.
//!
//! This crate builds `no_std` + `alloc` (CI compiles it for
//! `thumbv7em-none-eabi`); all float math is `libm`-backed there. It is
//! *not* a fixed-point implementation — see `todo.md` §31.
//!
//! See `docs/provenance.md` for algorithm sources.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use tpt_dsp_core::{
    autocorrelation, autocorrelation_pitch, levinson_durbin, lpc_to_lsp, lsp_is_stable,
    lsp_quantize_uniform, lsp_to_lpc, residual as lpc_residual, synthesize as lpc_synthesize,
    windowed, WindowType, DEFAULT_GRID_POINTS, DEFAULT_VOICED_THRESHOLD,
};
use tpt_media_bitstream::{BitReader, BitWriter};
use tpt_media_codec::{
    Capabilities, CodecConfig, CodecError, CodecInfo, CodecResult, Decoder, DynDecoder,
    DynDecoderFactory, DynEncoder, DynEncoderFactory, Encoder, SupportedFormat,
};
use tpt_media_core::{
    AudioFrame, ChannelLayout, Frame, Packet, PacketFlags, SampleFormat, Timebase, Timestamp,
};

/// Codec id for Reed.
pub const CODEC_ID: [u8; 4] = *b"REED";

/// Bitstream version.
pub const VERSION: u16 = 1;

/// Size in bytes of the per-packet CRC-32 trailer.
pub const CRC_TRAILER_BYTES: usize = 4;

/// Stochastic-codebook entries searched per unvoiced frame (§31's
/// "initial codebook target"). Never transmitted — see [`codebook_entry`].
pub const CODEBOOK_ENTRIES: usize = 1024;
/// Bits used to transmit a codebook index: `log2(CODEBOOK_ENTRIES)`.
pub const CODEBOOK_BITS: u32 = 10;

/// Shortest transmittable pitch period, in samples (the wire field is
/// `period - PITCH_MIN_PERIOD`). 16 samples = 1000 Hz at 16 kHz and 500 Hz
/// at 8 kHz — the top of any plausible speech F0 either way.
pub const PITCH_MIN_PERIOD: usize = 16;
/// Bits used to transmit a pitch period (10 bits covers the full
/// searchable period range at either supported rate).
pub const PITCH_BITS: u32 = 10;

/// Bits used to transmit the excitation-gain index.
pub const GAIN_BITS: u32 = 6;
/// Gain index 0 means *exact mute* (digital silence out); indices 1..=63
/// map to excitation gains of `-62 dB .. 0 dB` relative to full scale, in
/// 1 dB steps. The floor doubles as the silence threshold: a decoded
/// gain that low is inaudible on any output device.
pub const GAIN_FLOOR_DB: f64 = -62.0;

/// Excitation-gain decay per consecutively concealed frame, in dB — a
/// packet gap fades toward silence instead of holding its last level.
pub const PLC_DECAY_DB: f64 = 6.0;
/// Gains below this (after decay) conceal as exact silence.
const PLC_MUTE_DB: f64 = -100.0;

/// Hamming-window analysis convention for LPC (the classical choice for
/// autocorrelation LPC); see [`windowed`].
const ANALYSIS_WINDOW: WindowType = WindowType::Hamming;

/// Upper bound on bits per LSF coefficient accepted from the wire —
/// bounds the packet-parsing work regardless of what a hostile packet
/// claims.
const MAX_LSF_BITS: u32 = 12;

/// CRC-32 (IEEE 802.3, polynomial `0xEDB88320`, reflected) — the same
/// per-packet integrity check the other native codecs use, kept local so
/// the codec layer can verify packet-sized integrity without depending
/// on the container crate.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// One SplitMix64 step: the same deterministic primitive the corpus
/// generators (`tpt-cli`'s `signals.rs`) use — pure integer math,
/// identical on every platform, so both encoder and decoder derive
/// bit-identical codebook entries from an index with no shared state and
/// no transmission.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Unit-RMS codebook entry `entry` of `len` samples, written into `out`.
///
/// A pure function of `(entry, len)`: the entry's samples are a
/// SplitMix64 stream seeded from the index, normalized by their own RMS.
/// Encoder and decoder call this with the same arguments and get
/// bit-identical shapes, which is why the codebook itself never appears
/// on the wire. Entries are independent (different seeds), so the search
/// sees 1024 genuinely distinct noise shapes.
pub fn codebook_entry(entry: usize, len: usize, out: &mut [f64]) {
    assert!(out.len() >= len, "codebook output buffer too small");
    let mut state = 0x0123_4567_89AB_CDEFu64 ^ (entry as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut sumsq = 0.0f64;
    for slot in &mut out[..len] {
        // 53-bit uniform in [0, 1), remapped to [-1, 1).
        let u = ((splitmix64(&mut state) >> 11) as f64) / 9007199254740992.0;
        let v = u * 2.0 - 1.0;
        *slot = v;
        sumsq += v * v;
    }
    let rms = libm::sqrt(sumsq / len as f64).max(1e-12);
    for slot in &mut out[..len] {
        *slot /= rms;
    }
}

/// Reed configuration.
#[derive(Clone, Debug)]
pub struct ReedConfig {
    /// Sample rate: 8 kHz or 16 kHz (§31's initial targets).
    pub sample_rate: u32,
    /// LPC order (even, 2..=16); quantized as LSPs, `order` indices per
    /// frame.
    pub lpc_order: u8,
    /// Bits per quantized LSP coefficient (2..=12); maps the wire size of
    /// the spectral envelope and is carried in every packet header, so
    /// encoder and decoder need not agree beyond the wire format.
    pub lsf_bits: u8,
}

impl Default for ReedConfig {
    fn default() -> Self {
        ReedConfig {
            sample_rate: 16_000,
            lpc_order: 10,
            lsf_bits: 5,
        }
    }
}

impl ReedConfig {
    /// Frame size in samples: a fixed 20 ms frame at any supported rate
    /// (160 samples at 8 kHz, 320 at 16 kHz).
    pub fn frame_size(&self) -> usize {
        (self.sample_rate / 50) as usize
    }

    /// Longest pitch period searchable in one frame: one sample of
    /// overlap must remain at the largest lag (`autocorrelation_pitch`'s
    /// contract). 50 Hz at either rate.
    fn max_pitch_period(&self) -> usize {
        self.frame_size() - 1
    }

    fn validate(&self) -> CodecResult<()> {
        if self.sample_rate != 8_000 && self.sample_rate != 16_000 {
            return Err(CodecError::InvalidConfig(
                "reed sample_rate must be 8000 or 16000",
            ));
        }
        if self.lpc_order < 2 || self.lpc_order > 16 || self.lpc_order % 2 != 0 {
            return Err(CodecError::InvalidConfig(
                "reed lpc_order must be even, 2..=16",
            ));
        }
        if (self.lsf_bits as u32) < 2 || (self.lsf_bits as u32) > MAX_LSF_BITS {
            return Err(CodecError::InvalidConfig("reed lsf_bits must be 2..=12"));
        }
        // The wire carries one strictly-increasing index per LSP
        // coefficient, so there must be strictly more levels than
        // coefficients for a valid interlaced set to be representable at
        // all (e.g. 10 coefficients need at least 4 bits/coefficient).
        if (1u32 << self.lsf_bits) as usize <= self.lpc_order as usize {
            return Err(CodecError::InvalidConfig(
                "reed needs 2^lsf_bits > lpc_order so LSP indices stay strictly increasing",
            ));
        }
        Ok(())
    }

    /// Encode a validated config into the codec-private bytes the muxer
    /// stores in the track (see `tpt-cli`'s `run_encode_job`); the
    /// decoder rebuilds its config from these + the track sample rate.
    pub fn to_codec_private(&self) -> Vec<u8> {
        vec![self.lpc_order, self.lsf_bits]
    }

    /// Inverse of [`ReedConfig::to_codec_private`] for the fields it
    /// carries; `sample_rate` comes from the container track instead.
    pub fn from_codec_private(sample_rate: u32, private: &[u8]) -> CodecResult<Self> {
        let mut cfg = ReedConfig {
            sample_rate,
            ..ReedConfig::default()
        };
        if private.len() >= 2 {
            cfg.lpc_order = private[0];
            cfg.lsf_bits = private[1];
        }
        cfg.validate()?;
        Ok(cfg)
    }
}

fn static_info(caps: Capabilities) -> CodecInfo {
    CodecInfo {
        name: "Reed",
        codec_id: CODEC_ID,
        version: VERSION,
        capabilities: caps,
        supported_formats: alloc::vec![
            SupportedFormat::Audio {
                sample_format: SampleFormat::S16,
                channel_layout: ChannelLayout::Mono,
                sample_rate: 8_000,
            },
            SupportedFormat::Audio {
                sample_format: SampleFormat::S16,
                channel_layout: ChannelLayout::Mono,
                sample_rate: 16_000,
            },
        ],
    }
}

/// Quantize an LSP set (radians in `(0, π)`) to per-coefficient indices of
/// `lsf_bits` bits, using [`lsp_quantize_uniform`]'s collision-safe
/// uniform quantization and returning the index each quantized value
/// lands on. Indices are enforced strictly increasing and confined to
/// `1..=levels-1`: level 0 would dequantize to exactly 0 and level
/// `levels-1`... to just under π, and [`lsp_is_stable`] requires every
/// LSP strictly inside `(0, π)` — reserving the endpoints is what makes
/// every transmittable index set rebuild into a stable filter. Returns
/// `None` if enforcement overflows (the set doesn't fit at this bit
/// depth) — callers fall back to their previous good frame in that case.
fn quantize_lsf_indices(lsp: &[f64], lsf_bits: u8, out: &mut [u16]) -> Option<()> {
    let levels = 1u32 << lsf_bits;
    let step = core::f64::consts::PI / levels as f64;
    let mut q = vec![0f64; lsp.len()];
    lsp_quantize_uniform(lsp, levels, &mut q);
    let mut prev = 0u32;
    for (i, slot) in out.iter_mut().enumerate() {
        let mut idx = libm::round(q[i] / step);
        if !(0.0..levels as f64).contains(&idx) {
            return None;
        }
        // Endpoint reservation + strict ordering, in one clamp.
        idx = idx.max(prev as f64 + 1.0);
        if idx >= levels as f64 {
            return None;
        }
        prev = idx as u32;
        *slot = prev as u16;
    }
    Some(())
}

/// Rebuild LSP frequencies (radians) from transmitted indices, re-enforcing
/// the strictly-increasing ordering [`lsp_quantize_uniform`] guarantees on
/// the encoder side: a hostile or corrupted index set is nudged back into
/// interlace order rather than handed to synthesis, where a collapsed or
/// inverted pair would produce an unstable filter.
fn dequantize_lsf_indices(indices: &[u16], lsf_bits: u8, out: &mut [f64]) {
    let levels = 1u32 << lsf_bits;
    let step = core::f64::consts::PI / levels as f64;
    let mut prev = 0.0f64;
    for (i, slot) in out.iter_mut().enumerate() {
        let mut v = (indices[i] as f64).min((levels - 1) as f64) * step;
        if i > 0 && v <= prev {
            v = prev + step;
        }
        *slot = v;
        prev = v;
    }
}

/// Reed encoder.
pub struct ReedEncoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: ReedConfig,
    /// Precomputed analysis window (one-time cost, reused every frame).
    win: Vec<f32>,
    /// Last successfully transmitted LSF index vector — the fallback when
    /// a frame's own analysis fails to produce a representable stable
    /// envelope (first frame falls back to a flat mid-range envelope).
    last_lsf: Option<Vec<u16>>,
}

impl ReedEncoder {
    pub fn new(codec_cfg: CodecConfig, enc: ReedConfig) -> CodecResult<Self> {
        enc.validate()?;
        let mut win = vec![0f32; enc.frame_size()];
        windowed(ANALYSIS_WINDOW, enc.frame_size(), &mut win);
        Ok(ReedEncoder {
            info: static_info(Capabilities {
                // The unvoiced shape-gain search (1024 candidate
                // excitations per frame) is deliberately not claimed
                // real-time-safe; the decoder path is.
                real_time_decoder: true,
                lossy: true,
                ..Capabilities::default()
            }),
            cfg: codec_cfg,
            enc,
            win,
            last_lsf: None,
        })
    }

    /// Fallback envelope for a frame whose own analysis did not yield a
    /// representable stable LSP set: the previous frame's indices, or a
    /// flat-ish mid-range spread on the very first frame (indices in
    /// `1..=levels-1`, strictly increasing — see
    /// [`quantize_lsf_indices`]).
    fn fallback_lsf(&self) -> Vec<u16> {
        if let Some(prev) = &self.last_lsf {
            return prev.clone();
        }
        let levels = 1u32 << self.enc.lsf_bits;
        let order = self.enc.lpc_order as usize;
        (0..order)
            .map(|i| (1 + i * (levels as usize - 2) / (order - 1)) as u16)
            .collect()
    }
}

impl Encoder for ReedEncoder {
    type Frame = AudioFrame;
    fn info(&self) -> &CodecInfo {
        &self.info
    }

    fn encode(&mut self, frame: &AudioFrame) -> CodecResult<Vec<Packet>> {
        let fs = self.enc.frame_size();
        let order = self.enc.lpc_order as usize;
        self.cfg.limits.check_packet(64 * 1024)?;
        if frame.samples.len() / 2 < fs {
            return Err(CodecError::InvalidConfig(
                "not enough samples for one block",
            ));
        }
        let x: Vec<f64> = (0..fs)
            .map(|i| {
                i16::from_le_bytes([frame.samples[2 * i], frame.samples[2 * i + 1]]) as f64
                    / i16::MAX as f64
            })
            .collect();

        // Silence short-circuit: an all-zero frame mutes exactly (gain
        // index 0), skipping the pitch search and codebook search — but
        // still carrying an envelope so the packet layout stays
        // fixed-size and simple.
        let silent = frame.samples[..fs * 2].iter().all(|&b| b == 0);

        // Envelope: Hamming-windowed autocorrelation LPC → LSP → indices.
        let mut xw = x.clone();
        for (i, slot) in xw.iter_mut().enumerate() {
            *slot *= self.win[i] as f64;
        }
        let mut r = vec![0f64; order + 1];
        autocorrelation(&xw, order, &mut r);
        let mut a = vec![0f64; order];
        let mut scratch = vec![0f64; order];
        levinson_durbin(&r, &mut a, &mut scratch);
        let mut lsp = vec![0f64; order];
        let lsf_indices = if lpc_to_lsp(&a, &mut lsp, DEFAULT_GRID_POINTS) {
            let mut idx = vec![0u16; order];
            if quantize_lsf_indices(&lsp, self.enc.lsf_bits, &mut idx).is_some() {
                self.last_lsf = Some(idx.clone());
                idx
            } else {
                self.fallback_lsf()
            }
        } else {
            // Unstable predictor (e.g. near-silent frame): repeat the
            // last good envelope rather than transmit garbage.
            self.fallback_lsf()
        };

        // Excitation target: the LPC residual of the *unwindowed* signal.
        let mut resid = vec![0f64; fs];
        lpc_residual(&x, &a, &mut resid);
        let mean_sq = resid.iter().map(|v| v * v).sum::<f64>() / fs as f64;
        let rms = libm::sqrt(mean_sq);
        let gain_db = (20.0 * libm::log10(rms.max(1e-12))).clamp(GAIN_FLOOR_DB, 0.0);
        let gain_idx = if silent {
            0u64
        } else {
            (1 + libm::round(gain_db - GAIN_FLOOR_DB) as i64).clamp(1, 63) as u64
        };

        // Voicing + pitch on the unwindowed signal.
        let pitch = (!silent).then(|| {
            autocorrelation_pitch(
                &x,
                PITCH_MIN_PERIOD,
                self.enc.max_pitch_period(),
                DEFAULT_VOICED_THRESHOLD,
            )
        });
        let voiced = pitch.as_ref().is_some_and(|p| p.voiced);
        let pitch_idx = if voiced {
            (pitch.as_ref().unwrap().period_samples.unwrap() - PITCH_MIN_PERIOD) as u64
        } else if silent {
            0
        } else {
            // Unvoiced frames carry a codebook index in the same field.
            best_codebook_index(&resid, fs) as u64
        };

        // Fixed-size payload: header + excitation + envelope.
        let max_bits = 16 + 3 + 4 + 1 + GAIN_BITS + PITCH_BITS + order as u32 * 15;
        let mut buf = vec![0u8; max_bits.div_ceil(8) as usize];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            // §35: versioned bitstream, rejected up front by decoders
            // that don't understand this layout.
            w.write(16, VERSION as u64)
                .map_err(|_| CodecError::Internal)?;
            w.write(3, (order / 2 - 1) as u64)
                .map_err(|_| CodecError::Internal)?;
            w.write(4, self.enc.lsf_bits as u64)
                .map_err(|_| CodecError::Internal)?;
            w.write(1, voiced as u64)
                .map_err(|_| CodecError::Internal)?;
            w.write(GAIN_BITS, gain_idx)
                .map_err(|_| CodecError::Internal)?;
            w.write(PITCH_BITS, pitch_idx)
                .map_err(|_| CodecError::Internal)?;
            for &idx in &lsf_indices {
                w.write(self.enc.lsf_bits as u32, idx as u64)
                    .map_err(|_| CodecError::Internal)?;
            }
            w.finish().map_err(|_| CodecError::Internal)?
        };
        buf.truncate(written);
        let crc = crc32(&buf[..written]);
        buf.extend_from_slice(&crc.to_le_bytes());
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

/// Shape-gain VQ search: the index of the codebook entry whose unit-RMS
/// shape best matches the residual target, i.e. the entry maximizing the
/// normalized correlation `(c·t)/‖c‖`. The optimal scalar gain for the
/// winning shape is then `(c·t)/‖c‖²`; only the (signless, dB-quantized)
/// residual RMS rides on the wire, so entries with negative correlation
/// simply score badly — a deliberate v0.1 simplification that saves the
/// sign bit.
fn best_codebook_index(target: &[f64], len: usize) -> usize {
    let mut entry = vec![0f64; len];
    let mut best_idx = 0usize;
    let mut best_score = f64::NEG_INFINITY;
    for e in 0..CODEBOOK_ENTRIES {
        codebook_entry(e, len, &mut entry);
        let mut dot = 0.0f64;
        let mut norm_sq = 0.0f64;
        for i in 0..len {
            dot += entry[i] * target[i];
            norm_sq += entry[i] * entry[i];
        }
        let score = dot / libm::sqrt(norm_sq).max(1e-12);
        if score > best_score {
            best_score = score;
            best_idx = e;
        }
    }
    best_idx
}

/// Decoder-side state carried between packets for concealment.
struct DecodeState {
    /// Synthesis filter (LPC coefficients from the last good packet).
    lpc: Vec<f64>,
    /// Last frame's voicing.
    voiced: bool,
    /// Last frame's pitch period (voiced) — carried so concealment can
    /// keep buzzing at the last known F0.
    period: usize,
    /// Last frame's excitation gain, in dB.
    gain_db: f64,
    /// Last frame's codebook index (unvoiced).
    cb_idx: usize,
}

/// Reed decoder.
pub struct ReedDecoder {
    info: CodecInfo,
    cfg: CodecConfig,
    enc: ReedConfig,
    state: Option<DecodeState>,
    /// Consecutively concealed frames — drives [`PLC_DECAY_DB`]/frame.
    concealed: u32,
}

impl ReedDecoder {
    pub fn new(codec_cfg: CodecConfig, enc: ReedConfig) -> CodecResult<Self> {
        enc.validate()?;
        Ok(ReedDecoder {
            info: static_info(Capabilities {
                real_time_decoder: true,
                lossy: true,
                ..Capabilities::default()
            }),
            cfg: codec_cfg,
            enc,
            state: None,
            concealed: 0,
        })
    }

    /// Packet-loss concealment: re-synthesize a frame from the last good
    /// packet's parameters with the excitation gain decayed
    /// [`PLC_DECAY_DB`] per consecutive concealed frame (fading to exact
    /// silence). With no prior state there is nothing to repeat, so the
    /// first-ever concealment is digital silence.
    pub fn conceal(&mut self, pts: Timestamp) -> CodecResult<AudioFrame> {
        let fs = self.enc.frame_size();
        let Some(s) = &self.state else {
            return Ok(self.silent_frame(pts));
        };
        self.concealed += 1;
        let gain_db = s.gain_db - PLC_DECAY_DB * self.concealed as f64;
        if gain_db < PLC_MUTE_DB {
            return Ok(self.silent_frame(pts));
        }
        let samples = self.synthesize(&s.lpc, s.voiced, s.period, gain_db, s.cb_idx, fs);
        Ok(AudioFrame {
            frame: Frame {
                pts,
                duration: None,
                side_data: Vec::new(),
            },
            samples,
            sample_count: fs as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: self.enc.sample_rate,
        })
    }

    fn silent_frame(&self, pts: Timestamp) -> AudioFrame {
        let fs = self.enc.frame_size();
        AudioFrame {
            frame: Frame {
                pts,
                duration: None,
                side_data: Vec::new(),
            },
            samples: vec![0u8; fs * 2],
            sample_count: fs as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: self.enc.sample_rate,
        }
    }

    /// Shared synthesis: excitation (pulse train / scaled codebook shape)
    /// through the LPC synthesis filter, clamped and scaled back to S16.
    fn synthesize(
        &self,
        lpc: &[f64],
        voiced: bool,
        period: usize,
        gain_db: f64,
        cb_idx: usize,
        fs: usize,
    ) -> Vec<u8> {
        let g = libm::pow(10.0, gain_db / 20.0);
        let mut exc = vec![0f64; fs];
        if voiced {
            // Impulse train at the transmitted period; a single impulse
            // per period has RMS g/√period, so amplitude g·√period makes
            // the excitation RMS equal the transmitted g.
            let amp = g * libm::sqrt(period as f64);
            let mut pos = 0usize;
            while pos < fs {
                exc[pos] = amp;
                pos += period;
            }
        } else {
            codebook_entry(cb_idx, fs, &mut exc);
            for slot in exc.iter_mut() {
                *slot *= g;
            }
        }
        let mut out = vec![0f64; fs];
        lpc_synthesize(&exc, lpc, &mut out);
        let mut bytes = vec![0u8; fs * 2];
        for (i, v) in out.iter().enumerate() {
            let s = (v.clamp(-1.0, 1.0) * i16::MAX as f64) as i16;
            bytes[2 * i..2 * i + 2].copy_from_slice(&s.to_le_bytes());
        }
        bytes
    }
}

impl Decoder for ReedDecoder {
    type Frame = AudioFrame;
    fn info(&self) -> &CodecInfo {
        &self.info
    }

    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<AudioFrame>> {
        self.cfg.limits.check_packet(packet.data.len())?;
        self.decode_inner(packet)
    }
}

impl ReedDecoder {
    fn decode_inner(&mut self, packet: &Packet) -> CodecResult<Vec<AudioFrame>> {
        if packet.data.len() < CRC_TRAILER_BYTES {
            return Err(CodecError::MalformedPacket);
        }
        let (body, checksum) = packet.data.split_at(packet.data.len() - CRC_TRAILER_BYTES);
        let expected = u32::from_le_bytes(checksum.try_into().expect("4-byte slice"));
        if crc32(body) != expected {
            return Err(CodecError::MalformedPacket);
        }
        let mut r = BitReader::new(body);
        // §35: reject a wire format version this decoder doesn't
        // understand before interpreting any further bits as if they
        // were this version's layout.
        let wire_version = r.read(16).map_err(|_| CodecError::MalformedPacket)? as u16;
        if wire_version != VERSION {
            return Err(CodecError::Unsupported("unsupported Reed packet version"));
        }
        let order = (r.read(3).map_err(|_| CodecError::MalformedPacket)? as usize + 1) * 2;
        let lsf_bits = r.read(4).map_err(|_| CodecError::MalformedPacket)? as u32;
        if !(2..=MAX_LSF_BITS).contains(&lsf_bits) {
            return Err(CodecError::MalformedPacket);
        }
        let voiced = r.read(1).map_err(|_| CodecError::MalformedPacket)? == 1;
        let gain_idx = r.read(GAIN_BITS).map_err(|_| CodecError::MalformedPacket)?;
        let field_idx = r
            .read(PITCH_BITS)
            .map_err(|_| CodecError::MalformedPacket)? as usize;
        let fs = self.enc.frame_size();
        let period = field_idx + PITCH_MIN_PERIOD;
        if voiced && period > self.enc.max_pitch_period() {
            return Err(CodecError::MalformedPacket);
        }
        let cb_idx = field_idx; // unvoiced interpretation of the same field

        self.cfg.limits.check_frame_bytes(fs * 2)?;
        self.cfg
            .limits
            .check_allocation(fs * core::mem::size_of::<f64>() * 3 + order * 8 * 4)?;

        // Envelope: indices → LSPs (ordering re-enforced) → LPC. A
        // transmitted index set that still fails the stability check
        // after the nudge is rejected outright.
        let mut lsf_indices = vec![0u16; order];
        for slot in lsf_indices.iter_mut() {
            let v = r.read(lsf_bits).map_err(|_| CodecError::MalformedPacket)?;
            *slot = v as u16;
        }
        let mut lsp = vec![0f64; order];
        dequantize_lsf_indices(&lsf_indices, lsf_bits as u8, &mut lsp);
        if !lsp_is_stable(&lsp) {
            return Err(CodecError::MalformedPacket);
        }
        let mut a = vec![0f64; order];
        lsp_to_lpc(&lsp, &mut a);

        let gain_db = if gain_idx == 0 {
            PLC_MUTE_DB // exact mute below
        } else {
            GAIN_FLOOR_DB + (gain_idx as f64 - 1.0)
        };

        let samples = if gain_idx == 0 {
            vec![0u8; fs * 2]
        } else {
            self.synthesize(&a, voiced, period, gain_db, cb_idx, fs)
        };

        self.state = Some(DecodeState {
            lpc: a,
            voiced,
            period,
            gain_db,
            cb_idx,
        });
        self.concealed = 0;

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
pub struct ReedDecoderFactory;
impl DynDecoderFactory for ReedDecoderFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        static_info(Capabilities {
            real_time_decoder: true,
            lossy: true,
            ..Capabilities::default()
        })
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<alloc::boxed::Box<dyn DynDecoder>> {
        let d = ReedDecoder::new(cfg.clone(), ReedConfig::default())?;
        Ok(alloc::boxed::Box::new(ReedDynDecoder { inner: d }))
    }
}
pub struct ReedDynDecoder {
    inner: ReedDecoder,
}
impl ReedDynDecoder {
    /// Wrap a concrete decoder for direct type-erased use (the FFI
    /// bridge, custom registries). The factory route stays
    /// available for config-from-codec-private construction.
    pub fn new(inner: ReedDecoder) -> Self {
        Self { inner }
    }
}

impl DynDecoder for ReedDynDecoder {
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

pub struct ReedEncoderFactory;
impl DynEncoderFactory for ReedEncoderFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        static_info(Capabilities {
            real_time_decoder: true,
            lossy: true,
            ..Capabilities::default()
        })
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<alloc::boxed::Box<dyn DynEncoder>> {
        let e = ReedEncoder::new(cfg.clone(), ReedConfig::default())?;
        Ok(alloc::boxed::Box::new(ReedDynEncoder { inner: e }))
    }
}
pub struct ReedDynEncoder {
    inner: ReedEncoder,
}
impl ReedDynEncoder {
    /// Wrap a concrete encoder for direct type-erased use (the FFI
    /// bridge, custom registries). The factory route stays
    /// available for config-from-codec-private construction.
    pub fn new(inner: ReedEncoder) -> Self {
        Self { inner }
    }
}

impl DynEncoder for ReedDynEncoder {
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
            _ => return Err(CodecError::Unsupported("Reed expects audio frames")),
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

    const RATE: u32 = 16_000;
    const FS: usize = 320; // 20 ms at 16 kHz

    fn frame_from(samples: Vec<u8>, start_tick: i64) -> AudioFrame {
        AudioFrame {
            frame: Frame {
                pts: Timestamp {
                    ticks: start_tick,
                    timebase: Timebase::new(1, RATE),
                },
                duration: None,
                side_data: Vec::new(),
            },
            samples,
            sample_count: FS as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: RATE,
        }
    }

    fn s16_bytes(x: &[f64]) -> Vec<u8> {
        let mut b = Vec::with_capacity(x.len() * 2);
        for &v in x {
            let s = (v.clamp(-1.0, 1.0) * i16::MAX as f64) as i16;
            b.extend_from_slice(&s.to_le_bytes());
        }
        b
    }

    fn rms(frame: &AudioFrame) -> f64 {
        let n = frame.sample_count as usize;
        let sum: f64 = (0..n)
            .map(|i| {
                let v = frame.s16(0, i).unwrap() as f64;
                v * v
            })
            .sum();
        (sum / n as f64).sqrt() / i16::MAX as f64
    }

    /// A synthetic voiced frame: a 200 Hz glottal pulse train shaped by
    /// two formant-ish resonances (harmonic amplitudes peaked around
    /// 700 Hz and 1200 Hz) — "vowel-like" without needing a speech
    /// corpus (§36's honest stance: synthetic stand-ins, labelled as
    /// such).
    fn vowel_frame(start_tick: i64) -> AudioFrame {
        let period = 80usize; // 200 Hz at 16 kHz
        let x: Vec<f64> = (0..FS)
            .map(|n| {
                let t = n as f64 / RATE as f64;
                let mut v = 0.0;
                for k in 1..=40usize {
                    let f = 200.0 * k as f64;
                    let formant =
                        (-((f - 700.0) / 300.0).powi(2)) + 0.8 * (-((f - 1200.0) / 400.0).powi(2));
                    v += formant * (core::f64::consts::TAU * f * t).sin();
                }
                let pulse = if n % period == 0 { 3.0 } else { 1.0 };
                // 0.0025 (not a round 0.04): the 40-harmonic formant sum
                // peaks around 1700, so this lands the waveform at a ~0.5
                // peak — inside S16 range, no clipping.
                v * pulse * 0.0025
            })
            .collect();
        frame_from(s16_bytes(&x), start_tick)
    }

    fn white_noise_frame(start_tick: i64, seed: u64) -> AudioFrame {
        let mut state = seed;
        let mut x = vec![0f64; FS];
        for slot in x.iter_mut() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *slot = ((state >> 33) as f64 / 2147483648.0) - 1.0;
        }
        frame_from(s16_bytes(&x), start_tick)
    }

    fn pair(cfg: ReedConfig) -> (ReedEncoder, ReedDecoder) {
        let fw = CodecConfig::default();
        (
            ReedEncoder::new(fw.clone(), cfg.clone()).unwrap(),
            ReedDecoder::new(fw, cfg).unwrap(),
        )
    }

    #[test]
    fn vowel_round_trip_preserves_level_and_pitch() {
        let (mut enc, mut dec) = pair(ReedConfig::default());
        let input = vowel_frame(0);
        let in_rms = rms(&input);
        let packet = enc.encode(&input).unwrap().into_iter().next().unwrap();
        let out = &dec.decode(&packet).unwrap()[0];
        assert_eq!(out.sample_count, FS as u32);
        let out_rms = rms(out);
        assert!(
            out_rms > 0.0,
            "decoded vowel must not be silent (rms {out_rms})"
        );
        let level_db = 20.0 * (out_rms / in_rms).log10();
        assert!(
            level_db > -8.0 && level_db < 8.0,
            "decoded vowel level {level_db:.1} dB off the input"
        );
        // Pitch survives end to end: the estimator finds the true 80-sample
        // period on the input, the wire field carries exactly that period,
        // and the decoded samples stay strongly periodic at it. (The argmax
        // re-estimation on the *decoded* samples may lock onto a
        // high-Q formant ringing near half the period — a classic
        // narrow-band vocoder artifact — so periodicity is asserted at the
        // transmitted lag directly rather than via a second argmax.)
        let est = autocorrelation_pitch(
            &(0..FS)
                .map(|i| input.s16(0, i).unwrap() as f64 / i16::MAX as f64)
                .collect::<Vec<f64>>(),
            PITCH_MIN_PERIOD,
            FS - 1,
            DEFAULT_VOICED_THRESHOLD,
        );
        assert_eq!(
            est.period_samples,
            Some(80),
            "input pitch must analyze as the synthesized 200 Hz"
        );

        // Parse the transmitted pitch field out of the packet.
        let body_len = packet.data.len() - CRC_TRAILER_BYTES;
        let mut r = BitReader::new(&packet.data[..body_len]);
        let _version = r.read(16).unwrap();
        let _order = r.read(3).unwrap();
        let _bits = r.read(4).unwrap();
        let voiced_bit = r.read(1).unwrap();
        assert_eq!(voiced_bit, 1, "a vowel frame must be transmitted voiced");
        let _gain = r.read(GAIN_BITS).unwrap();
        let transmitted = r.read(PITCH_BITS).unwrap() as usize + PITCH_MIN_PERIOD;
        assert_eq!(transmitted, 80, "wire must carry the analyzed period");

        // Normalized autocorrelation of the decoded samples at the
        // transmitted period: the "still voiced at the right pitch" check.
        let y: Vec<f64> = (0..FS)
            .map(|i| out.s16(0, i).unwrap() as f64 / i16::MAX as f64)
            .collect();
        let r0: f64 = y.iter().map(|v| v * v).sum();
        let norm_at =
            |lag: usize| -> f64 { (0..FS - lag).map(|n| y[n] * y[n + lag]).sum::<f64>() / r0 };
        let at_pitch = norm_at(80);
        assert!(
            at_pitch > 0.5,
            "decoded vowel must stay periodic at the transmitted period 80, got {at_pitch:.3}"
        );
    }

    #[test]
    fn silence_round_trips_to_exact_silence() {
        let (mut enc, mut dec) = pair(ReedConfig::default());
        let input = frame_from(vec![0u8; FS * 2], 0);
        let packet = enc.encode(&input).unwrap().into_iter().next().unwrap();
        let out = &dec.decode(&packet).unwrap()[0];
        assert!(
            out.samples.iter().all(|&b| b == 0),
            "digital silence must round-trip to digital silence"
        );
    }

    #[test]
    fn unvoiced_noise_carries_finite_energy() {
        let (mut enc, mut dec) = pair(ReedConfig::default());
        let input = white_noise_frame(0, 0xDEAD_BEEF);
        let in_rms = rms(&input);
        let packet = enc.encode(&input).unwrap().into_iter().next().unwrap();
        let out = &dec.decode(&packet).unwrap()[0];
        let out_rms = rms(out);
        assert!(out_rms > 0.0, "decoded noise must not be silent");
        let level_db = 20.0 * (out_rms / in_rms).log10();
        assert!(
            level_db > -10.0 && level_db < 10.0,
            "decoded noise level {level_db:.1} dB off the input"
        );
    }

    #[test]
    fn packet_carries_crc_trailer() {
        let (mut enc, _) = pair(ReedConfig::default());
        let pkt = enc
            .encode(&vowel_frame(0))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let (body, checksum) = pkt.data.split_at(pkt.data.len() - CRC_TRAILER_BYTES);
        assert_eq!(
            crc32(body),
            u32::from_le_bytes(checksum.try_into().unwrap()),
            "trailing 4 bytes must be the CRC-32 of the body"
        );
    }

    #[test]
    fn decode_rejects_corrupted_body() {
        let (mut enc, mut dec) = pair(ReedConfig::default());
        let mut pkt = enc
            .encode(&vowel_frame(0))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let flip = pkt.data.len() / 2;
        pkt.data[flip] ^= 0x40;
        assert_eq!(dec.decode(&pkt).unwrap_err(), CodecError::MalformedPacket);
    }

    #[test]
    fn decode_rejects_corrupted_checksum() {
        let (mut enc, mut dec) = pair(ReedConfig::default());
        let mut pkt = enc
            .encode(&vowel_frame(0))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let last = pkt.data.len() - 1;
        pkt.data[last] ^= 0x01;
        assert_eq!(dec.decode(&pkt).unwrap_err(), CodecError::MalformedPacket);
    }

    #[test]
    fn decode_rejects_undersized_packet() {
        let (mut enc, mut dec) = pair(ReedConfig::default());
        let mut pkt = enc
            .encode(&vowel_frame(0))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        pkt.data.clear();
        assert_eq!(dec.decode(&pkt).unwrap_err(), CodecError::MalformedPacket);
    }

    #[test]
    fn decode_rejects_unsupported_wire_version() {
        let mut buf = alloc::vec![0u8; 8];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            w.write(16, 99).unwrap(); // bogus/future version
            w.write(3, 4).unwrap();
            w.write(4, 5).unwrap();
            w.write(1, 1).unwrap();
            w.finish().unwrap()
        };
        buf.truncate(written);
        let crc = crc32(&buf[..written]);
        buf.extend_from_slice(&crc.to_le_bytes());
        let pkt = Packet {
            data: buf,
            pts: Timestamp::zero(Timebase::new(1, RATE)),
            dts: Timestamp::zero(Timebase::new(1, RATE)),
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        let (_, mut dec) = pair(ReedConfig::default());
        assert_eq!(
            dec.decode(&pkt).unwrap_err(),
            CodecError::Unsupported("unsupported Reed packet version")
        );
    }

    #[test]
    fn decode_enforces_resource_limits() {
        let (mut enc, _) = pair(ReedConfig::default());
        let packet = enc
            .encode(&vowel_frame(0))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let tight = CodecConfig {
            limits: tpt_media_codec::ResourceLimits {
                max_allocation_bytes: 512,
                ..tpt_media_codec::ResourceLimits::default()
            },
            ..CodecConfig::default()
        };
        let mut dec = ReedDecoder::new(tight, ReedConfig::default()).unwrap();
        assert_eq!(
            dec.decode(&packet).unwrap_err(),
            CodecError::ResourceLimit(tpt_media_codec::ResourceLimitKind::Allocation)
        );
    }

    #[test]
    fn rejects_bad_config() {
        let fw = CodecConfig::default();
        for cfg in [
            ReedConfig {
                sample_rate: 44_100,
                ..ReedConfig::default()
            },
            ReedConfig {
                lpc_order: 7, // odd
                ..ReedConfig::default()
            },
            ReedConfig {
                lsf_bits: 1, // below the 2-bit floor
                ..ReedConfig::default()
            },
            ReedConfig {
                lsf_bits: 2, // 4 levels < 10 coefficients: no valid
                // strictly-increasing LSP set fits
                ..ReedConfig::default()
            },
        ] {
            assert!(
                ReedEncoder::new(fw.clone(), cfg.clone()).is_err(),
                "config must be rejected: {cfg:?}"
            );
        }
    }

    #[test]
    fn lsf_index_quantization_round_trips_stable() {
        // Every frame the encoder transmits must rebuild, decoder-side,
        // to a stable LSP set and finite LPC coefficients, at every
        // supported LSF bit depth.
        // 4 bits is the floor for this order (2^4 = 16 > 10 coefficients);
        // see ReedConfig::validate.
        for lsf_bits in [4u8, 5, 8] {
            let cfg = ReedConfig {
                lsf_bits,
                ..ReedConfig::default()
            };
            for frame in [
                vowel_frame(0),
                white_noise_frame(0, 42),
                frame_from(vec![0u8; FS * 2], 0),
            ] {
                let (mut enc, _) = pair(cfg.clone());
                let pkt = enc.encode(&frame).unwrap().into_iter().next().unwrap();
                let body_len = pkt.data.len() - CRC_TRAILER_BYTES;
                let mut r = BitReader::new(&pkt.data[..body_len]);
                let _version = r.read(16).unwrap();
                let order = (r.read(3).unwrap() as usize + 1) * 2;
                let bits = r.read(4).unwrap() as u8;
                assert_eq!(bits, lsf_bits, "lsf bits must ride on the wire");
                let _voiced = r.read(1).unwrap();
                let _gain = r.read(GAIN_BITS).unwrap();
                let _field = r.read(PITCH_BITS).unwrap();
                let mut indices = Vec::with_capacity(order);
                for _ in 0..order {
                    indices.push(r.read(bits as u32).unwrap() as u16);
                }
                let mut lsp = vec![0f64; order];
                dequantize_lsf_indices(&indices, bits, &mut lsp);
                assert!(
                    lsp_is_stable(&lsp),
                    "dequantized LSPs must be stable at {bits} bits: {lsp:?}"
                );
                let mut a = vec![0f64; order];
                lsp_to_lpc(&lsp, &mut a);
                assert!(
                    a.iter().all(|v| v.is_finite()),
                    "reconstructed LPC coefficients must be finite"
                );
            }
        }
    }

    #[test]
    fn conceal_fades_after_packet_loss() {
        let (mut enc, mut dec) = pair(ReedConfig::default());
        let p0 = enc
            .encode(&vowel_frame(0))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let p1 = enc
            .encode(&vowel_frame(FS as i64))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        let p2 = enc
            .encode(&vowel_frame(2 * FS as i64))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        dec.decode(&p0).unwrap();
        // Two consecutive concealed frames must fade: audible but quieter
        // frame over frame, then a good packet restores full level.
        let c1 = dec.conceal(p1.pts).unwrap();
        assert_eq!(
            c1.sample_count, FS as u32,
            "concealed frame carries a full frame"
        );
        assert!(rms(&c1) > 0.0, "first concealed frame still audible");
        let c2 = dec.conceal(p2.pts).unwrap();
        assert!(
            rms(&c2) < rms(&c1),
            "concealment must decay frame over frame"
        );
        let good2 = dec.decode(&p2).unwrap().into_iter().next().unwrap();
        assert!(rms(&good2) > rms(&c2));
    }

    #[test]
    fn conceal_without_prior_state_is_silent() {
        let (_, mut dec) = pair(ReedConfig::default());
        let f = dec
            .conceal(Timestamp::zero(Timebase::new(1, RATE)))
            .unwrap();
        assert!(f.samples.iter().all(|&b| b == 0));
    }

    #[test]
    fn jittered_pts_are_preserved_not_reordered() {
        let (mut enc, mut dec) = pair(ReedConfig::default());
        let packets: Vec<Packet> = (0..4i64)
            .map(|i| {
                enc.encode(&vowel_frame(i * FS as i64))
                    .unwrap()
                    .into_iter()
                    .next()
                    .unwrap()
            })
            .collect();
        for i in [0usize, 2, 3, 1] {
            let out = dec.decode(&packets[i]).unwrap();
            assert_eq!(out.len(), 1);
            assert_eq!(
                out[0].frame.pts, packets[i].pts,
                "PTS must echo its own packet"
            );
            assert_eq!(out[0].sample_count, FS as u32);
        }
    }

    #[test]
    fn encoding_is_deterministic() {
        let (mut pa, _) = pair(ReedConfig::default());
        let (mut pb, _) = pair(ReedConfig::default());
        for i in 0..3i64 {
            let f = vowel_frame(i * FS as i64);
            let x = pa.encode(&f).unwrap();
            let y = pb.encode(&f).unwrap();
            assert_eq!(
                x[0].data, y[0].data,
                "independent encoders must agree byte-for-byte"
            );
        }
    }

    #[test]
    fn codebook_entry_is_deterministic_unit_rms_and_distinct() {
        let n = 64;
        let mut e0 = vec![0f64; n];
        let mut e0_again = vec![0f64; n];
        let mut e1 = vec![0f64; n];
        codebook_entry(0, n, &mut e0);
        codebook_entry(0, n, &mut e0_again);
        codebook_entry(1, n, &mut e1);
        assert_eq!(
            e0, e0_again,
            "entries must be pure functions of (index, len)"
        );
        let rms = (e0.iter().map(|v| v * v).sum::<f64>() / n as f64).sqrt();
        assert!(
            (rms - 1.0).abs() < 1e-9,
            "entries must be unit-RMS, got {rms}"
        );
        assert_ne!(e0, e1, "distinct entries must differ");
        assert!(e0.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn decode_never_panics_on_corrupted_or_arbitrary_packets() {
        // §38 "codec fuzzing": the project's deterministic corruption
        // sweep — arbitrary bytes, full truncation, bit flips — must
        // always return a `Result`, never panic.
        let (_, mut dec) = pair(ReedConfig::default());
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let mut lcg = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            state
        };
        let pts = Timestamp::zero(Timebase::new(1, RATE));

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

        let (mut enc, _) = pair(ReedConfig::default());
        let valid = enc
            .encode(&vowel_frame(0))
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        for cut in 0..=valid.data.len() {
            let pkt = Packet {
                data: valid.data[..cut].to_vec(),
                ..valid.clone()
            };
            let _ = dec.decode(&pkt);
        }
        for _ in 0..256 {
            let pos = (lcg() as usize) % valid.data.len();
            let bit = 1u8 << (lcg() % 8);
            let mut corrupt = valid.clone();
            corrupt.data[pos] ^= bit;
            let _ = dec.decode(&corrupt);
        }
        assert!(dec.decode(&valid).is_ok());
    }
}
