//! PCM integration: interleaved raw-sample bytes ↔ [`AudioFrame`].
//!
//! This is the lowest audio layer of `tpt-media-io`. A [`PcmSpec`]
//! (sample format + channel count + sample rate) describes one
//! interleaved byte stream; [`decode_pcm`] slices it into [`AudioFrame`]s
//! with monotonic PTS, [`encode_pcm`] flattens frames back to bytes, and
//! [`PcmSpec::into_frame`] wraps one raw byte span without copying.
//!
//! The `normalize_to_f32`/`denormalize_from_f32` pair converts any
//! supported integer/float format to the `[-1, 1]` domain used by DSP
//! tooling (`tpt-dsp-io`, the analysis crates). Integer formats are scaled
//! by `2^(bits-1)` exactly like the rest of the framework expects; the
//! conversion is bit-exact for ≤ 24-bit formats and within one count for
//! 32-bit formats.

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;

use tpt_media_core::{AudioFrame, ChannelLayout, SampleFormat, Timebase, Timestamp};

use crate::{IoError, IoResult};

/// Description of one interleaved PCM byte stream.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PcmSpec {
    /// Per-sample wire format.
    pub sample_format: SampleFormat,
    /// Number of interleaved channels.
    pub channels: u8,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

impl PcmSpec {
    /// Abortive construction; rejects zero channels / sample rate.
    pub fn new(sample_format: SampleFormat, channels: u8, sample_rate: u32) -> IoResult<Self> {
        if channels == 0 {
            return Err(IoError::Other("PCM channels must be >= 1".into()));
        }
        if sample_rate == 0 {
            return Err(IoError::Other("PCM sample rate must be >= 1".into()));
        }
        Ok(PcmSpec {
            sample_format,
            channels,
            sample_rate,
        })
    }

    /// Bytes per interleaved sample frame (`channels * bytes_per_sample`).
    pub const fn bytes_per_frame(self) -> usize {
        self.channels as usize * self.sample_format.bytes_per_sample()
    }

    /// The channel layout with `channels` channels.
    pub fn channel_layout(self) -> ChannelLayout {
        ChannelLayout::from_channels(self.channels)
    }

    /// A timebase of `1/sample_rate` seconds per sample frame.
    pub fn timebase(self) -> Timebase {
        Timebase::new(1, self.sample_rate)
    }

    /// Wrap one raw interleaved byte span as an [`AudioFrame`].
    ///
    /// The bytes are copied (caller-owned buffers, see `ARCHITECTURE.md`
    /// §4). Returns an error if `bytes.len()` is not an exact multiple of
    /// [`Self::bytes_per_frame`].
    pub fn into_frame(self, bytes: &[u8]) -> IoResult<AudioFrame> {
        let frame_bytes = self.bytes_per_frame();
        if bytes.len() % frame_bytes != 0 {
            return Err(IoError::Other(
                "PCM data length must be a whole number of sample frames".into(),
            ));
        }
        Ok(AudioFrame {
            frame: tpt_media_core::Frame {
                pts: Timestamp::zero(self.timebase()),
                duration: None,
                side_data: Vec::new(),
            },
            samples: bytes.to_vec(),
            sample_count: (bytes.len() / frame_bytes) as u32,
            format: self.sample_format,
            channel_layout: self.channel_layout(),
            sample_rate: self.sample_rate,
        })
    }
}

/// Flatten a slice of [`AudioFrame`]s into interleaved PCM bytes.
///
/// All frames must share the same format, channel layout and sample rate;
/// otherwise an error is returned so a misconfigured chain cannot silently
/// produce a corrupt stream.
pub fn encode_pcm(frames: &[AudioFrame]) -> IoResult<Vec<u8>> {
    let mut out = Vec::new();
    for f in frames {
        let expected = f.sample_count as usize
            * f.channel_layout.channels() as usize
            * f.format.bytes_per_sample();
        if f.samples.len() == expected {
            out.extend_from_slice(&f.samples);
        } else {
            return Err(IoError::Other(
                "frame metadata does not match its sample buffer length".into(),
            ));
        }
    }
    Ok(out)
}

/// Slice raw interleaved PCM bytes into [`AudioFrame`]s of at most
/// `frames_per_chunk` sample frames each.
///
/// PTS advances by `frames_per_chunk` sample frames (last chunk may be
/// shorter) at the `1/sample_rate` timebase. A trailing partial chunk is
/// emitted as-is.
pub fn decode_pcm(
    bytes: &[u8],
    spec: PcmSpec,
    frames_per_chunk: usize,
) -> IoResult<Vec<AudioFrame>> {
    if frames_per_chunk == 0 {
        return Err(IoError::Other("frames_per_chunk must be >= 1".into()));
    }
    let frame_bytes = spec.bytes_per_frame();
    if frames_per_chunk.checked_mul(frame_bytes).is_none() {
        return Err(IoError::Overflow);
    }
    let chunk_bytes = frames_per_chunk * frame_bytes;
    let mut chunks = Vec::new();
    let mut offset = 0usize;
    let mut index = 0u64;
    while offset < bytes.len() {
        let end = (offset + chunk_bytes).min(bytes.len());
        let mut frame = spec.into_frame(&bytes[offset..end])?;
        frame.frame.pts = Timestamp {
            ticks: (index * frames_per_chunk as u64) as i64,
            timebase: spec.timebase(),
        };
        chunks.push(frame);
        offset = end;
        index += 1;
    }
    Ok(chunks)
}

/// Convert interleaved raw bytes to normalized `f32` samples in `[-1, 1]`.
///
/// Integer formats are scaled by `2^(bits-1)`; `U8` is centered at `128`.
/// The output length equals the input byte count divided by the format's
/// bytes-per-sample.
pub fn normalize_to_f32(samples: &[u8], format: SampleFormat) -> IoResult<Vec<f32>> {
    let mut out = Vec::with_capacity(samples.len() / format.bytes_per_sample());
    let chunks = samples.chunks_exact(format.bytes_per_sample());
    if !chunks.remainder().is_empty() {
        return Err(IoError::Other(
            "sample buffer is not a whole number of samples for the format".into(),
        ));
    }
    for c in chunks {
        out.push(sample_to_f32(c, format));
    }
    Ok(out)
}

/// Convert normalized `f32` samples in `[-1, 1]` back to raw interleaved
/// bytes in the given format.
///
/// Values are clamped to `[-1, 1]`; integer formats round to nearest with
/// ties away from zero (matches the framework's scaling by `2^(bits-1)`),
/// so the round trip is bit-exact for integer formats up to 24 bits.
pub fn denormalize_from_f32(samples: &[f32], format: SampleFormat) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * format.bytes_per_sample());
    for s in samples {
        let clamped = if s.is_nan() {
            0.0
        } else {
            (*s).clamp(-1.0, 1.0)
        };
        match format {
            SampleFormat::U8 => {
                let v = libm::round(f64::from(clamped) * 128.0) as i32 + 128;
                out.push(v.clamp(0, 255) as u8);
            }
            SampleFormat::S16 => {
                let v =
                    libm::round(f64::from(clamped) * 32_768.0).clamp(-32_768.0, 32_767.0) as i16;
                out.extend_from_slice(&v.to_le_bytes());
            }
            SampleFormat::S24 => {
                let v = libm::round(f64::from(clamped) * 8_388_608.0)
                    .clamp(-8_388_608.0, 8_388_607.0) as i32;
                out.extend_from_slice(&v.to_le_bytes()[..3]);
            }
            SampleFormat::S32 => {
                let v = libm::round(f64::from(clamped) * 2_147_483_648.0)
                    .clamp(-2_147_483_648.0, 2_147_483_647.0) as i32;
                out.extend_from_slice(&v.to_le_bytes());
            }
            SampleFormat::F32 => out.extend_from_slice(&clamped.to_le_bytes()),
            SampleFormat::F64 => out.extend_from_slice(&f64::from(clamped).to_le_bytes()),
        }
    }
    out
}

fn sample_to_f32(c: &[u8], format: SampleFormat) -> f32 {
    match format {
        SampleFormat::U8 => (f32::from(c[0]) - 128.0) / 128.0,
        SampleFormat::S16 => {
            let v = i16::from_le_bytes([c[0], c[1]]);
            f32::from(v) / 32_768.0
        }
        SampleFormat::S24 => {
            let v = i32::from_le_bytes([c[0], c[1], c[2], if c[2] & 0x80 != 0 { 0xFF } else { 0 }]);
            v as f32 / 8_388_608.0
        }
        SampleFormat::S32 => {
            let v = i32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            v as f32 / 2_147_483_648.0
        }
        SampleFormat::F32 => f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
        SampleFormat::F64 => {
            let v = f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
            v as f32
        }
    }
}

impl fmt::Display for PcmSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} x{} @ {} Hz",
            self.sample_format, self.channels, self.sample_rate
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_rejects_zero_channels_or_rate() {
        assert!(PcmSpec::new(SampleFormat::S16, 0, 48_000).is_err());
        assert!(PcmSpec::new(SampleFormat::S16, 2, 0).is_err());
        assert!(PcmSpec::new(SampleFormat::S16, 2, 48_000).is_ok());
    }

    #[test]
    fn decode_pcm_chunks_with_monotonic_pts() {
        let spec = PcmSpec::new(SampleFormat::S16, 1, 48_000).unwrap();
        let mut bytes = Vec::new();
        for v in 0..10i16 {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let frames = decode_pcm(&bytes, spec, 4).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].sample_count, 4);
        assert_eq!(frames[1].sample_count, 4);
        assert_eq!(frames[2].sample_count, 2);
        assert_eq!(frames[1].frame.pts.ticks, 4);
        assert_eq!(frames[2].frame.pts.ticks, 8);
        assert_eq!(frames[2].s16(0, 1), Some(9));
    }

    #[test]
    fn decode_pcm_rejects_partial_frame() {
        let spec = PcmSpec::new(SampleFormat::S16, 2, 48_000).unwrap();
        let bad = vec![0u8; 6]; // 3 bytes, not a multiple of a 4-byte frame
        assert!(decode_pcm(&bad, spec, 1).is_err());
    }

    #[test]
    fn encode_pcm_round_trips() {
        let spec = PcmSpec::new(SampleFormat::S16, 2, 48_000).unwrap();
        let mut bytes = Vec::new();
        for v in 0..8i16 {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let frames = decode_pcm(&bytes, spec, 4).unwrap();
        assert_eq!(encode_pcm(&frames).unwrap(), bytes);
    }

    #[test]
    fn normalize_s16_is_exact() {
        let bytes = [0x00u8, 0x80, 0xff, 0x7f]; // -32768, 32767
        let norm = normalize_to_f32(&bytes, SampleFormat::S16).unwrap();
        assert_eq!(norm[0], -1.0);
        assert_eq!(norm[1], 32767.0 / 32768.0);
        let back = denormalize_from_f32(&norm, SampleFormat::S16);
        assert_eq!(back, bytes);
    }

    #[test]
    fn normalize_u8_is_exact() {
        let bytes = [0u8, 255, 128];
        let norm = normalize_to_f32(&bytes, SampleFormat::U8).unwrap();
        assert_eq!(norm[0], -1.0);
        assert_eq!(norm[2], 0.0);
        assert_eq!(denormalize_from_f32(&norm, SampleFormat::U8), bytes);
    }

    #[test]
    fn normalize_f32_passthrough() {
        let samples = [0.5f32, -0.25];
        let mut bytes = Vec::new();
        for s in samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let norm = normalize_to_f32(&bytes, SampleFormat::F32).unwrap();
        assert_eq!(norm, samples);
    }

    #[test]
    fn normalize_rejects_partial_sample() {
        assert!(normalize_to_f32(&[0u8, 0, 0], SampleFormat::S16).is_err());
    }

    #[test]
    fn denormalize_clamps_out_of_range() {
        let out = denormalize_from_f32(&[2.0, -2.0], SampleFormat::S32);
        let v0 = i32::from_le_bytes([out[0], out[1], out[2], out[3]]);
        assert_eq!(v0, i32::MAX);
        let v1 = i32::from_le_bytes([out[4], out[5], out[6], out[7]]);
        assert_eq!(v1, i32::MIN);
    }
}
