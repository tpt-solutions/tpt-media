//! `tpt-media-io` — PCM/WAV/raw audio I/O.
//!
//! Provides a minimal but correct WAV reader/writer for the standard
//! PCM and IEEE-float variants. Extensions (`WAVEFORMATEXTENSIBLE`,
//! etc.) are rejected with a clear error rather than partially parsed.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use tpt_media_core::{AudioFrame, ChannelLayout, SampleFormat, Timestamp, Timebase};

/// I/O error type.
#[derive(Debug)]
pub enum IoError {
    UnexpectedEof,
    InvalidHeader,
    Unsupported(&'static str),
    Overflow,
    Other(&'static str),
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoError::UnexpectedEof => f.write_str("unexpected end of input"),
            IoError::InvalidHeader => f.write_str("invalid header"),
            IoError::Unsupported(s) => write!(f, "unsupported: {}", s),
            IoError::Overflow => f.write_str("integer overflow"),
            IoError::Other(s) => write!(f, "io error: {}", s),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for IoError {}

/// Result alias.
pub type IoResult<T> = Result<T, IoError>;

/// WAV `fmt ` chunk data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WavFormat {
    pub sample_format: SampleFormat,
    pub channels: u16,
    pub sample_rate: u32,
    pub byte_rate: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
}

/// Parse a WAV file from raw bytes.
pub fn parse_wav(bytes: &[u8]) -> IoResult<(WavFormat, Vec<u8>)> {
    if bytes.len() < 12 {
        return Err(IoError::InvalidHeader);
    }
    if &bytes[0..4] != b"RIFF" {
        return Err(IoError::InvalidHeader);
    }
    if &bytes[8..12] != b"WAVE" {
        return Err(IoError::InvalidHeader);
    }

    let mut pos = 12;
    let mut format: Option<WavFormat> = None;
    let mut data: Option<Vec<u8>> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        pos += 8;
        if pos + size > bytes.len() {
            return Err(IoError::UnexpectedEof);
        }
        let body = &bytes[pos..pos + size];
        match id {
            b"fmt " => format = Some(parse_fmt(body)?),
            b"data" => data = Some(body.to_vec()),
            _ => {}
        }
        pos += size;
        if size & 1 == 1 && pos < bytes.len() {
            pos += 1;
        }
        if format.is_some() && data.is_some() {
            break;
        }
    }

    let format = format.ok_or(IoError::InvalidHeader)?;
    let data = data.ok_or(IoError::InvalidHeader)?;
    Ok((format, data))
}

fn parse_fmt(body: &[u8]) -> IoResult<WavFormat> {
    if body.len() < 16 {
        return Err(IoError::InvalidHeader);
    }
    let format_tag = u16::from_le_bytes([body[0], body[1]]);
    let channels = u16::from_le_bytes([body[2], body[3]]);
    let sample_rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
    let byte_rate = u32::from_le_bytes([body[8], body[9], body[10], body[11]]);
    let block_align = u16::from_le_bytes([body[12], body[13]]);
    let bits_per_sample = u16::from_le_bytes([body[14], body[15]]);

    let sample_format = match (format_tag, bits_per_sample) {
        (1, 8) => SampleFormat::U8,
        (1, 16) => SampleFormat::S16,
        (1, 24) => SampleFormat::S24,
        (1, 32) => SampleFormat::S32,
        (3, 32) => SampleFormat::F32,
        (3, 64) => SampleFormat::F64,
        _ => return Err(IoError::Unsupported("WAV format")),
    };

    Ok(WavFormat {
        sample_format,
        channels,
        sample_rate,
        byte_rate,
        block_align,
        bits_per_sample,
    })
}

/// Decode WAV bytes into an [`AudioFrame`].
pub fn decode_wav(bytes: &[u8]) -> IoResult<AudioFrame> {
    let (fmt, samples) = parse_wav(bytes)?;
    let channels = fmt.channels as u8;
    let channel_layout = ChannelLayout::from_channels(channels);
    let bytes_per_sample = (fmt.bits_per_sample / 8) as usize;
    let sample_count = if bytes_per_sample == 0 {
        0
    } else {
        (samples.len() / (channels as usize * bytes_per_sample)) as u32
    };
    Ok(AudioFrame {
        frame: tpt_media_core::Frame {
            pts: Timestamp::zero(Timebase::new(1, fmt.sample_rate)),
            duration: None,
            side_data: Vec::new(),
        },
        samples,
        sample_count,
        format: fmt.sample_format,
        channel_layout,
        sample_rate: fmt.sample_rate,
    })
}

/// Serialize a sequence of [`AudioFrame`]s into a WAV byte buffer.
pub fn write_wav(frames: &[AudioFrame]) -> IoResult<Vec<u8>> {
    if frames.is_empty() {
        return Err(IoError::Other("no frames"));
    }
    let first = &frames[0];
    let channels = first.channel_layout.channels() as u16;
    let bits_per_sample = (first.format.bytes_per_sample() * 8) as u16;
    let byte_rate = first.sample_rate * u32::from(channels) * u32::from(bits_per_sample) / 8;
    let block_align = channels * (bits_per_sample / 8);

    let format_tag: u16 = match first.format {
        SampleFormat::F32 | SampleFormat::F64 => 3,
        _ => 1,
    };

    let data_size: usize = frames.iter().map(|f| f.samples.len()).sum();
    let total_size: u32 = (4 + 8 + 16 + 8 + data_size).try_into().map_err(|_| IoError::Overflow)?;

    let mut out = Vec::with_capacity(total_size as usize + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&total_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&format_tag.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&first.sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_size as u32).to_le_bytes());
    for f in frames {
        out.extend_from_slice(&f.samples);
    }
    Ok(out)
}

/// Generate a 16-bit PCM mono sine-wave frame — useful for testing.
pub fn sine_wave(freq_hz: f64, sample_rate: u32, duration_seconds: f64, amplitude: f64) -> AudioFrame {
    let sample_count = (sample_rate as f64 * duration_seconds) as u32;
    let mut samples = vec![0i16; sample_count as usize];
    for (i, s) in samples.iter_mut().enumerate() {
        let t = i as f64 / sample_rate as f64;
        let v = (t * freq_hz * 2.0 * core::f64::consts::PI).sin() * amplitude;
        *s = (v * i16::MAX as f64) as i16;
    }
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    AudioFrame {
        frame: tpt_media_core::Frame {
            pts: Timestamp::zero(Timebase::new(1, sample_rate)),
            duration: None,
            side_data: Vec::new(),
        },
        samples: bytes,
        sample_count,
        format: SampleFormat::S16,
        channel_layout: ChannelLayout::Mono,
        sample_rate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_wave_then_wav_round_trip() {
        let f = sine_wave(440.0, 48_000, 0.01, 0.5);
        let bytes = write_wav(&[f.clone()]).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let decoded = decode_wav(&bytes).unwrap();
        assert_eq!(decoded.sample_rate, 48_000);
        assert_eq!(decoded.format, SampleFormat::S16);
        assert_eq!(decoded.channel_layout, ChannelLayout::Mono);
    }

    #[test]
    fn parse_wav_rejects_bad_magic() {
        let bytes = b"NOTRIFFXXXX";
        assert!(matches!(parse_wav(bytes), Err(IoError::InvalidHeader)));
    }

    #[test]
    fn parse_wav_too_short() {
        assert!(matches!(parse_wav(&[1, 2, 3]), Err(IoError::InvalidHeader)));
    }

    #[test]
    fn write_wav_rejects_empty() {
        let r = write_wav(&[]);
        assert!(matches!(r, Err(IoError::Other(_))));
    }

    #[test]
    fn parse_wav_with_float32() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&36u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&48_000u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        let fmt = parse_wav(&bytes).unwrap().0;
        assert_eq!(fmt.sample_format, SampleFormat::F32);
        assert_eq!(fmt.sample_rate, 48_000);
    }
}