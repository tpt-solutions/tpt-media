//! `tpt-audio-aura` — Aura: high-fidelity / archival audio codec.
//!
//! ## Modes
//!
//! - **Lossless** — linear-predictive coding (LPC) up to order 32
//!   followed by Exp-Golomb-coded residuals (zigzag-mapped). Bit-exact
//!   round-trip.
//! - **Lossy** — large-window MDCT (2048/4096) with Bark-band,
//!   masking-driven quantisation and spectral noise shaping. Uses `f64`
//!   internally.
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
//! - **Lossy quantization is psychoacoustically driven** (`tpt-dsp-core`'s
//!   `bark`/`psychoacoustic`/`noise_shaping` modules, §10-11 of
//!   `todo.md`): each MDCT bin is mapped to a Bark critical band
//!   ([`hz_to_bark`]), bands are classified tonal/noise
//!   ([`classify_tonal_bins`]) and their energy is spread across the Bark
//!   axis to model simultaneous masking ([`simultaneous_masking`],
//!   [`combined_threshold_db`]); the resulting per-band masking threshold
//!   sets that band's quantization step (coarser where masking hides more
//!   noise), and coefficients within a band are quantized with first-order
//!   spectral noise shaping ([`noise_shape_step`]) so the residual
//!   quantization error is pushed toward already-noisy neighbors rather
//!   than left independent per bin. The per-band step table is
//!   transmitted (32-bit float per band) so the decoder needs no side
//!   information beyond what's in the packet — see [`AuraEncoder::encode`]
//!   / the `AuraMode::Lossy` decode arm for the exact wire layout. This is
//!   a breaking change to the v0.1 lossy wire format (see `CHANGELOG.md`);
//!   there is no stereo model since Aura is mono-only (no consumer needs
//!   stereo yet).
//! - Per-packet integrity: every packet ends with a CRC-32 over its
//!   payload (`crc32` + [`CRC_TRAILER_BYTES`]); the decoder verifies it
//!   before parsing anything.
//!
//! See `docs/provenance.md` for algorithm sources.

#![warn(missing_docs)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use tpt_dsp_core::{
    aggregate_bands, autocorrelation, bark_to_hz, classify_tonal_bins, combined_threshold_db,
    db_to_power, hz_to_bark, imdct, levinson_durbin, mdct, noise_shape_step, simultaneous_masking,
    threshold_in_quiet,
};
use tpt_media_bitstream::{BitReader, BitWriter};
use tpt_media_codec::{
    Capabilities, CodecConfig, CodecError, CodecInfo, CodecResult, Decoder, DynDecoder,
    DynDecoderFactory, DynEncoder, DynEncoderFactory, Encoder, SupportedFormat,
};
use tpt_media_core::{
    AudioFrame, ChannelLayout, Frame, Packet, PacketFlags, SampleFormat, Timebase, Timestamp,
};

/// Codec id for Aura.
pub const CODEC_ID: [u8; 4] = *b"AURA";

/// Bitstream version.
pub const VERSION: u16 = 1;

/// Size in bytes of the per-packet CRC-32 trailer.
pub const CRC_TRAILER_BYTES: usize = 4;

/// CRC-32 (IEEE 802.3, polynomial `0xEDB88320`, reflected) — the same
/// check used by `tpt-container`'s per-cluster integrity (`crc32.rs` in
/// that crate), kept here so the codec layer can verify granular,
/// packet-sized integrity without depending on the container crate.
///
/// Every Aura packet ends with this 4-byte value over all preceding
/// bytes of the packet data; [`AuraDecoder`] refuses to parse a packet
/// whose CRC does not match.
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

/// Prominence (dB) a local spectral peak must exceed its neighbors by to be
/// classified tonal (vs. noise-like) for masking purposes — see
/// [`classify_tonal_bins`]. `6.0` is a middle-of-the-road choice consistent
/// with the range used by MPEG-style psychoacoustic models.
const TONAL_PROMINENCE_DB: f64 = 6.0;

/// First-order spectral noise-shaping feedback (see [`noise_shape_step`]):
/// the classic `h=[1]` shaper documented in `tpt-dsp-core::noise_shaping`,
/// applied across MDCT bins in frequency order so each bin's quantization
/// error is fed into the next bin's decision.
const NS_FEEDBACK: [f64; 1] = [1.0];

/// Floor on a per-band quantization step: finer than this exceeds the
/// resolution of the 16-bit coefficient index anyway.
const MIN_QUANT_STEP: f64 = 1.0 / 32767.0;

/// Ceiling on a per-band quantization step, so a fully-masked band still
/// quantizes to *something* representable rather than an unbounded step.
const MAX_QUANT_STEP: f64 = 4.0;

/// Center frequency (Hz) of MDCT bin `bin` out of `n_half` bins spanning
/// `0..sample_rate/2`.
fn mdct_bin_hz(sample_rate: u32, n_half: usize, bin: usize) -> f64 {
    let nyquist = sample_rate as f64 / 2.0;
    (bin as f64 + 0.5) * nyquist / n_half as f64
}

/// Bark critical-band index of MDCT bin `bin` (see [`mdct_bin_hz`]).
fn mdct_bin_bark_band(sample_rate: u32, n_half: usize, bin: usize) -> usize {
    hz_to_bark(mdct_bin_hz(sample_rate, n_half, bin).max(1.0))
        .max(0.0)
        .floor() as usize
}

/// Deterministically map each of `n_half` MDCT bins to its Bark
/// critical-band index. Both encoder and decoder recompute this
/// independently from `(sample_rate, n_half)` alone — both values are
/// already known to the decoder via its own [`AuraConfig`] — so the band
/// layout itself is never transmitted, only the per-band quantization step
/// derived from it (see [`AuraEncoder::encode`]).
fn mdct_bin_bark_bands(sample_rate: u32, n_half: usize, out: &mut [usize]) {
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = mdct_bin_bark_band(sample_rate, n_half, i);
    }
}

/// Number of distinct Bark bands spanned by `n_half` MDCT bins (bark bands
/// are non-decreasing with bin index, so the last bin's band + 1 gives the
/// count without allocating the full mapping).
fn mdct_num_bark_bands(sample_rate: u32, n_half: usize) -> usize {
    if n_half == 0 {
        return 0;
    }
    mdct_bin_bark_band(sample_rate, n_half, n_half - 1) + 1
}

fn static_info(caps: Capabilities) -> CodecInfo {
    CodecInfo {
        name: "Aura",
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
    /// Lossless (LPC + Exp-Golomb residuals) or lossy (MDCT +
    /// masking-driven quantisation).
    pub mode: AuraMode,
    /// Sample rate in Hz; 44.1/48 kHz per the supported-format table.
    pub sample_rate: u32,
    /// LPC filter order, 1..=32 (lossless residual prediction).
    pub lpc_order: u8,
    /// Samples per coded block (lossless) and the MDCT analysis
    /// hop input length basis; 1024 by default.
    pub block_size: u16,
    /// MDCT window size for the lossy path (2048 or 4096).
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
    /// Build an encoder for `enc` under the framework configuration
    /// (`version` + resource limits); `enc` is validated first.
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
    fn info(&self) -> &CodecInfo {
        &self.info
    }
    fn encode(&mut self, frame: &AudioFrame) -> CodecResult<Vec<Packet>> {
        self.cfg.limits.check_packet(64 * 1024)?;
        let fs = self.enc.block_size as usize;
        if self.enc.mode == AuraMode::Lossy && fs % 2 != 0 {
            return Err(CodecError::InvalidConfig(
                "aura lossy block_size must be even",
            ));
        }
        let n_samples = frame.samples.len() / 2;
        if n_samples < fs {
            return Err(CodecError::InvalidConfig(
                "not enough samples for one block",
            ));
        }
        let x: Vec<f64> = frame.samples[..fs * 2]
            .chunks_exact(2)
            .map(|s| i16::from_le_bytes([s[0], s[1]]) as f64 / i16::MAX as f64)
            .collect();
        let order = self.enc.lpc_order as usize;
        // Upper bound: ~64 bits of header slack, `order` 16-bit LPC
        // coefficients, and up to 96 bits/sample (a generous bound on the
        // exp-Golomb-coded lossless residual — see `integer_predict`'s doc
        // for why the residual can occasionally be much larger than the
        // sample range; lossy only needs 16 bits/coefficient).
        // `+ 64 * 32` covers the lossy per-band scale-factor table (a 32-bit
        // float per Bark band; ~25 bands max for any audible sample rate).
        // `+ 16` covers the leading wire-version field.
        let max_bits = 64 + order * 16 + fs * 96 + 64 * 32 + 16;
        let mut buf = vec![0u8; max_bits.div_ceil(8)];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            // §35: a versioned bitstream. Written first so a decoder that
            // doesn't understand this wire format version fails cleanly
            // (`CodecError::Unsupported`) before it ever tries to
            // interpret bits it wasn't built for, rather than silently
            // misparsing them.
            w.write(16, VERSION as u64)
                .map_err(|_| CodecError::Internal)?;
            let mode_bit = if self.enc.mode == AuraMode::Lossless {
                1u64
            } else {
                0
            };
            w.write(1, mode_bit).map_err(|_| CodecError::Internal)?;
            w.write(5, self.enc.lpc_order as u64)
                .map_err(|_| CodecError::Internal)?;
            w.write(8, (self.enc.mdct_size / 256 - 1) as u64)
                .map_err(|_| CodecError::Internal)?;
            match self.enc.mode {
                AuraMode::Lossless => {
                    let mut r = vec![0f64; order + 1];
                    autocorrelation(&x, order, &mut r);
                    let mut a = vec![0f64; order];
                    let mut scratch = vec![0f64; order];
                    levinson_durbin(&r, &mut a, &mut scratch);
                    let coeffs_q = quantize_coeffs(&a);
                    let samples_i32: Vec<i32> = (0..fs)
                        .map(|i| {
                            i16::from_le_bytes([frame.samples[2 * i], frame.samples[2 * i + 1]])
                                as i32
                        })
                        .collect();
                    for c in &coeffs_q {
                        w.write(16, (*c as i16 as u16) as u64)
                            .map_err(|_| CodecError::Internal)?;
                    }
                    for i in 0..fs {
                        let take = order.min(i);
                        let predicted = if take == 0 {
                            0
                        } else {
                            integer_predict(&samples_i32[..i], &coeffs_q[..take])
                        };
                        let residual = samples_i32[i] as i64 - predicted;
                        write_exp_golomb(&mut w, zigzag_encode(residual))
                            .map_err(|_| CodecError::Internal)?;
                    }
                }
                AuraMode::Lossy => {
                    let n_half = fs / 2;
                    let mut spec = vec![0f64; n_half];
                    mdct(&x, &mut spec);

                    let mut band_of_bin = vec![0usize; n_half];
                    mdct_bin_bark_bands(self.enc.sample_rate, n_half, &mut band_of_bin);
                    let num_bands = mdct_num_bark_bands(self.enc.sample_rate, n_half);

                    // Power proxy per bin (squared MDCT coefficient — not a
                    // true FFT power spectrum, but sufficient for relative
                    // energy/tonality within this codec's own Bark bands).
                    let power: Vec<f64> = spec.iter().map(|s| (*s * *s).max(1e-12)).collect();
                    let mut tonal = vec![false; n_half];
                    classify_tonal_bins(&power, TONAL_PROMINENCE_DB, &mut tonal);

                    let mut band_energy = vec![0f64; num_bands];
                    aggregate_bands(&power, &band_of_bin, &mut band_energy);
                    let mut band_tonal = vec![false; num_bands];
                    for (i, &b) in band_of_bin.iter().enumerate() {
                        band_tonal[b] |= tonal[i];
                    }
                    // Band `b` holds every bin whose floor(bark) == b, so
                    // `b + 0.5` approximates its center, both for the
                    // spreading function and for recovering an
                    // approximate Hz value for `threshold_in_quiet`.
                    let band_bark: Vec<f64> = (0..num_bands).map(|b| b as f64 + 0.5).collect();

                    let mut excitation = vec![0f64; num_bands];
                    simultaneous_masking(
                        &band_energy,
                        &band_bark,
                        &band_tonal,
                        &band_bark,
                        &mut excitation,
                    );

                    let mut step = vec![0f64; num_bands];
                    for b in 0..num_bands {
                        let band_hz = bark_to_hz(band_bark[b]).max(1.0);
                        let quiet_db = threshold_in_quiet(band_hz);
                        let combined_db = combined_threshold_db(excitation[b].max(1e-12), quiet_db);
                        // Uniform-quantizer noise power ~= step^2/12; pick
                        // the step whose quantization noise matches the
                        // masking threshold, so error is shaped to be
                        // inaudible rather than uniform across all bands.
                        let s = (12.0 * db_to_power(combined_db)).sqrt();
                        step[b] = s.clamp(MIN_QUANT_STEP, MAX_QUANT_STEP);
                    }
                    for &s in &step {
                        w.write(32, (s as f32).to_bits() as u64)
                            .map_err(|_| CodecError::Internal)?;
                    }

                    // Spectral noise shaping in bin order: each bin's
                    // quantization error is fed back (subtracted) into the
                    // next bin's decision, so the noise this per-band
                    // stepping introduces is reshaped rather than left
                    // independent per bin.
                    let mut ns_history = [0f64; 1];
                    for (i, s) in spec.iter().enumerate() {
                        let band_step = step[band_of_bin[i]];
                        let quantize = |v: f64| (v / band_step).round() * band_step;
                        let shaped = noise_shape_step(*s, &NS_FEEDBACK, &mut ns_history, quantize);
                        let idx = (shaped / band_step).round().clamp(-32768.0, 32767.0) as i16;
                        w.write(16, idx as u64).map_err(|_| CodecError::Internal)?;
                    }
                }
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
    /// Build a decoder matching the *encoder* configuration `enc`
    /// (mode, block/MDCT sizes, LPC order).
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
    fn info(&self) -> &CodecInfo {
        &self.info
    }
    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<AudioFrame>> {
        self.cfg.limits.check_packet(packet.data.len())?;
        self.decode_inner(packet)
    }
}

impl AuraDecoder {
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
        // understand *before* interpreting any further bits as if they
        // were this version's layout.
        let wire_version = r.read(16).map_err(|_| CodecError::MalformedPacket)? as u16;
        if wire_version != VERSION {
            return Err(CodecError::Unsupported("unsupported Aura packet version"));
        }
        let mode_bit = r.read(1).map_err(|_| CodecError::MalformedPacket)?;
        let mode = if mode_bit == 1 {
            AuraMode::Lossless
        } else {
            AuraMode::Lossy
        };
        let lpc_order = r.read(5).map_err(|_| CodecError::MalformedPacket)? as u8;
        let _mdct_size_bits = r.read(8).map_err(|_| CodecError::MalformedPacket)?;
        let fs = self.enc.block_size as usize;
        self.cfg.limits.check_frame_bytes(fs * 2)?;
        let samples = match mode {
            AuraMode::Lossless => {
                self.cfg.limits.check_allocation(
                    (fs * core::mem::size_of::<i32>())
                        .max(lpc_order as usize * core::mem::size_of::<i32>()),
                )?;
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
                let num_bands = mdct_num_bark_bands(self.enc.sample_rate, n_half);
                self.cfg.limits.check_allocation(
                    n_half * core::mem::size_of::<f64>() * 2
                        + n_half * core::mem::size_of::<usize>()
                        + num_bands * core::mem::size_of::<f64>(),
                )?;
                let mut band_of_bin = vec![0usize; n_half];
                mdct_bin_bark_bands(self.enc.sample_rate, n_half, &mut band_of_bin);
                let mut step = vec![0f64; num_bands];
                for s in step.iter_mut() {
                    let bits = r.read(32).map_err(|_| CodecError::MalformedPacket)? as u32;
                    *s = f32::from_bits(bits) as f64;
                }
                let mut spec = vec![0f64; n_half];
                for (i, s) in spec.iter_mut().enumerate() {
                    let q = r.read(16).map_err(|_| CodecError::MalformedPacket)? as i16;
                    *s = q as f64 * step[band_of_bin[i]];
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
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        static_info(Capabilities {
            lossless: true,
            lossy: true,
            ..Capabilities::default()
        })
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<alloc::boxed::Box<dyn DynDecoder>> {
        let enc = AuraConfig::default();
        let d = AuraDecoder::new(cfg.clone(), enc)?;
        Ok(alloc::boxed::Box::new(AuraDynDecoder { inner: d }))
    }
}
/// Type-erased wrapper around [`AuraDecoder`].
pub struct AuraDynDecoder {
    inner: AuraDecoder,
}
impl AuraDynDecoder {
    /// Wrap a concrete decoder for direct type-erased use (the FFI
    /// bridge, custom registries). The factory route stays
    /// available for config-from-codec-private construction.
    pub fn new(inner: AuraDecoder) -> Self {
        Self { inner }
    }
}

impl DynDecoder for AuraDynDecoder {
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

/// Type-erased encoder factory building Aura encoders from `Default`
/// parameters (the track's codec-private data carries the real
/// parameters on the decode side).
pub struct AuraEncoderFactory;
impl DynEncoderFactory for AuraEncoderFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        static_info(Capabilities {
            lossless: true,
            lossy: true,
            ..Capabilities::default()
        })
    }
    fn build(&self, cfg: &CodecConfig) -> CodecResult<alloc::boxed::Box<dyn DynEncoder>> {
        let enc = AuraConfig::default();
        let e = AuraEncoder::new(cfg.clone(), enc)?;
        Ok(alloc::boxed::Box::new(AuraDynEncoder { inner: e }))
    }
}
/// Type-erased wrapper around [`AuraEncoder`].
pub struct AuraDynEncoder {
    inner: AuraEncoder,
}
impl AuraDynEncoder {
    /// Wrap a concrete encoder for direct type-erased use (the FFI
    /// bridge, custom registries). The factory route stays
    /// available for config-from-codec-private construction.
    pub fn new(inner: AuraEncoder) -> Self {
        Self { inner }
    }
}

impl DynEncoder for AuraDynEncoder {
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
            _ => return Err(CodecError::Unsupported("Aura expects audio frames")),
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
        let enc_cfg = AuraConfig {
            mode: AuraMode::Lossless,
            block_size: 256,
            ..Default::default()
        };
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
        let enc_cfg = AuraConfig {
            mode: AuraMode::Lossy,
            block_size: 256,
            ..Default::default()
        };
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
        let enc_cfg = AuraConfig {
            mode: AuraMode::Lossless,
            block_size: 256,
            lpc_order: 8,
            ..Default::default()
        };
        let mut enc = AuraEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = AuraDecoder::new(cfg, enc_cfg).unwrap();
        let mut frame = tone_frame(256, 48_000);
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));
        let packets = enc.encode(&frame).unwrap();
        assert!(
            !packets[0].data.is_empty(),
            "encoded packet must not be empty"
        );
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
        let enc_cfg = AuraConfig {
            mode: AuraMode::Lossy,
            block_size: 256,
            ..Default::default()
        };
        let mut enc = AuraEncoder::new(cfg.clone(), enc_cfg.clone()).unwrap();
        let mut dec = AuraDecoder::new(cfg, enc_cfg).unwrap();
        let mut frame = tone_frame(256, 48_000);
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));
        let packets = enc.encode(&frame).unwrap();
        assert!(
            !packets[0].data.is_empty(),
            "encoded packet must not be empty"
        );
        let decoded = dec.decode(&packets[0]).unwrap();
        assert!(energy(&decoded[0]) > 0, "decoded tone must not be silent");
    }

    #[test]
    fn rejects_oversized_lpc_order() {
        let cfg = CodecConfig::default();
        let enc_cfg = AuraConfig {
            lpc_order: 64,
            ..Default::default()
        };
        assert!(AuraEncoder::new(cfg, enc_cfg).is_err());
    }

    fn encode_single_packet(mode: AuraMode) -> Packet {
        let cfg = CodecConfig::default();
        let enc_cfg = AuraConfig {
            mode,
            block_size: 256,
            lpc_order: 8,
            ..Default::default()
        };
        let mut enc = AuraEncoder::new(cfg, enc_cfg).unwrap();
        let mut frame = tone_frame(256, 48_000);
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));
        let packets = enc.encode(&frame).unwrap();
        packets.into_iter().next().unwrap()
    }

    #[test]
    fn packet_carries_crc_trailer() {
        let pkt = encode_single_packet(AuraMode::Lossless);
        assert!(pkt.data.len() > CRC_TRAILER_BYTES);
        let (body, checksum) = pkt.data.split_at(pkt.data.len() - CRC_TRAILER_BYTES);
        assert_eq!(
            crc32(body),
            u32::from_le_bytes(checksum.try_into().unwrap()),
            "trailing 4 bytes must be the CRC-32 of the body"
        );
    }

    #[test]
    fn decode_rejects_corrupted_body() {
        let mut pkt = encode_single_packet(AuraMode::Lossless);
        assert!(pkt.data.len() > CRC_TRAILER_BYTES + 1);
        let flip = pkt.data.len() / 2;
        pkt.data[flip] ^= 0x40;
        let mut dec = AuraDecoder::new(
            CodecConfig::default(),
            AuraConfig {
                mode: AuraMode::Lossless,
                block_size: 256,
                lpc_order: 8,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            dec.decode(&pkt).unwrap_err(),
            CodecError::MalformedPacket,
            "a body bit flip must be caught by the per-packet CRC"
        );
    }

    #[test]
    fn decode_rejects_corrupted_checksum() {
        let mut pkt = encode_single_packet(AuraMode::Lossy);
        let last = pkt.data.len() - 1;
        pkt.data[last] ^= 0x01;
        let mut dec = AuraDecoder::new(
            CodecConfig::default(),
            AuraConfig {
                mode: AuraMode::Lossy,
                block_size: 256,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(dec.decode(&pkt).unwrap_err(), CodecError::MalformedPacket);
    }

    #[test]
    fn decode_rejects_undersized_packet() {
        let mut dec = AuraDecoder::new(CodecConfig::default(), AuraConfig::default()).unwrap();
        let mut pkt = encode_single_packet(AuraMode::Lossless);
        pkt.data.clear();
        assert_eq!(dec.decode(&pkt).unwrap_err(), CodecError::MalformedPacket);
    }

    #[test]
    fn decode_enforces_resource_limits() {
        // §19: with a tight allocation budget, a structurally valid packet
        // must be refused as a resource-limit error rather than decodes
        // fully. Lossy: the decoder's spec + synthesis buffers for a 256
        // block need more than the 512-byte budget granted here.
        let enc_cfg_block = AuraConfig {
            mode: AuraMode::Lossy,
            block_size: 256,
            ..Default::default()
        };
        let mut enc = AuraEncoder::new(CodecConfig::default(), enc_cfg_block.clone()).unwrap();
        let mut frame = tone_frame(256, 48_000);
        frame.frame.pts = Timestamp::zero(Timebase::new(1, 48_000));
        let packet = enc.encode(&frame).unwrap().into_iter().next().unwrap();

        let tight = CodecConfig {
            limits: tpt_media_codec::ResourceLimits {
                max_allocation_bytes: 512,
                ..tpt_media_codec::ResourceLimits::default()
            },
            ..CodecConfig::default()
        };
        let mut dec = AuraDecoder::new(tight, enc_cfg_block).unwrap();
        assert_eq!(
            dec.decode(&packet).unwrap_err(),
            CodecError::ResourceLimit(tpt_media_codec::ResourceLimitKind::Allocation)
        );
    }

    #[test]
    fn mdct_bin_bark_bands_is_deterministic_and_nondecreasing() {
        let sample_rate = 48_000u32;
        let n_half = 128usize;
        let mut bands_a = vec![0usize; n_half];
        let mut bands_b = vec![0usize; n_half];
        mdct_bin_bark_bands(sample_rate, n_half, &mut bands_a);
        mdct_bin_bark_bands(sample_rate, n_half, &mut bands_b);
        assert_eq!(bands_a, bands_b, "band mapping must be deterministic");
        for w in bands_a.windows(2) {
            assert!(
                w[1] >= w[0],
                "bark band index must be non-decreasing with bin index: {w:?}"
            );
        }
        let num_bands = mdct_num_bark_bands(sample_rate, n_half);
        assert_eq!(num_bands, *bands_a.last().unwrap() + 1);
    }

    #[test]
    fn lossy_step_table_reflects_masking_not_flat() {
        // §30: a real masking model, wired in for the first time here,
        // must actually vary the per-band quantization step for a tone
        // (a strong masker in one band, near-silence elsewhere) — a
        // regression back to a constant/flat step (the old scheme) would
        // silently defeat the whole point of §10's psychoacoustic model.
        let cfg = CodecConfig::default();
        let block_size = 256usize;
        let sample_rate = 48_000u32;
        let enc_cfg = AuraConfig {
            mode: AuraMode::Lossy,
            block_size: block_size as u16,
            ..Default::default()
        };
        let mut enc = AuraEncoder::new(cfg, enc_cfg).unwrap();
        let mut frame = tone_frame(block_size, sample_rate);
        frame.frame.pts = Timestamp::zero(Timebase::new(1, sample_rate));
        let packet = enc.encode(&frame).unwrap().into_iter().next().unwrap();

        let body = &packet.data[..packet.data.len() - CRC_TRAILER_BYTES];
        let mut r = BitReader::new(body);
        let _wire_version = r.read(16).unwrap();
        let _mode_bit = r.read(1).unwrap();
        let _lpc_order = r.read(5).unwrap();
        let _mdct_size_bits = r.read(8).unwrap();
        let n_half = block_size / 2;
        let num_bands = mdct_num_bark_bands(sample_rate, n_half);
        let mut steps = Vec::with_capacity(num_bands);
        for _ in 0..num_bands {
            let bits = r.read(32).unwrap() as u32;
            steps.push(f32::from_bits(bits));
        }
        assert!(
            steps.iter().all(|s| s.is_finite() && *s > 0.0),
            "every band step must be a finite, positive value, got {steps:?}"
        );
        assert!(
            steps.iter().any(|&s| (s - steps[0]).abs() > 1e-6),
            "masking-driven step table must vary across bands for a tone, got {steps:?}"
        );
    }

    #[test]
    fn decode_rejects_unsupported_wire_version() {
        // §35: a decoder must fail cleanly on a wire version it doesn't
        // understand, rather than silently misinterpreting the rest of
        // the packet as if it were this decoder's own format.
        let mut buf = alloc::vec![0u8; 8];
        let written = {
            let mut w = BitWriter::new(&mut buf);
            w.write(16, 99).unwrap(); // bogus/future version
            w.write(1, 1).unwrap(); // mode bit (lossless); never reached
            w.write(5, 0).unwrap();
            w.write(8, 0).unwrap();
            w.finish().unwrap()
        };
        buf.truncate(written);
        let crc = crc32(&buf[..written]);
        buf.extend_from_slice(&crc.to_le_bytes());
        let pkt = Packet {
            data: buf,
            pts: Timestamp::zero(Timebase::new(1, 48_000)),
            dts: Timestamp::zero(Timebase::new(1, 48_000)),
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        let mut dec = AuraDecoder::new(CodecConfig::default(), AuraConfig::default()).unwrap();
        assert_eq!(
            dec.decode(&pkt).unwrap_err(),
            CodecError::Unsupported("unsupported Aura packet version")
        );
    }

    #[test]
    fn decode_never_panics_on_corrupted_or_arbitrary_packets() {
        // §38 "codec fuzzing" / "malformed input": a deterministic
        // corruption sweep, in the same spirit as `tpt-container`'s
        // `corruption_never_panics_and_truncation_never_panics` (§24) —
        // no external fuzz-harness dependency. Every call must return a
        // `Result`, never panic.
        // Matches `encode_single_packet`'s config: mode itself doesn't
        // matter here since `decode_inner` reads the actual mode from
        // the packet's own mode bit, not from `self.enc.mode`, but
        // `block_size`/`lpc_order` do need to match how the packets
        // below were actually encoded.
        let dec_cfg = AuraConfig {
            block_size: 256,
            lpc_order: 8,
            ..Default::default()
        };
        let mut dec = AuraDecoder::new(CodecConfig::default(), dec_cfg).unwrap();
        let mut state = 0x1234_5678_9ABC_DEF0u64;
        let mut lcg = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            state
        };
        let pts = Timestamp::zero(Timebase::new(1, 48_000));

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

        // A real valid packet (both modes), truncated at every length.
        for mode in [AuraMode::Lossless, AuraMode::Lossy] {
            let valid = encode_single_packet(mode);
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
}
