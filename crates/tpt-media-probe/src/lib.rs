//! `tpt-media-probe` — container and codec introspection.
//!
//! Given raw bytes (e.g. the contents of a file), `probe` attempts to
//! identify the container format and extract the most important
//! metadata: tracks, codecs, sample rates, dimensions, durations.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use tpt_media_core::{ChannelLayout, SampleFormat, Timebase};

/// Probed media container kind.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ContainerKind {
    Tpt,
    Wav,
    #[default]
    Unknown,
}

/// A probed audio track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioTrackInfo {
    pub codec_id: [u8; 4],
    pub sample_format: SampleFormat,
    pub channel_layout: ChannelLayout,
    pub sample_rate: u32,
    pub sample_count: Option<u64>,
    pub bitrate: Option<u32>,
}

/// A probed video track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoTrackInfo {
    pub codec_id: [u8; 4],
    pub width: u32,
    pub height: u32,
    /// Frame rate × 1000.
    pub framerate_x1000: u32,
    pub frame_count: Option<u64>,
}

/// Combined probe result.
#[derive(Clone, Debug, Default)]
pub struct ProbeReport {
    pub container: ContainerKind,
    pub file_size: u64,
    pub corruption: Vec<String>,
    pub audio: Vec<AudioTrackInfo>,
    pub video: Vec<VideoTrackInfo>,
    pub global_timebase: Option<Timebase>,
}

impl ProbeReport {
    pub fn new() -> Self {
        ProbeReport::default()
    }
    pub fn is_unknown(&self) -> bool {
        self.container == ContainerKind::Unknown
    }
    pub fn track_count(&self) -> usize {
        self.audio.len() + self.video.len()
    }
}

impl fmt::Display for ProbeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Container: {:?}", self.container)?;
        writeln!(f, "File size: {} bytes", self.file_size)?;
        writeln!(f, "Tracks: {}", self.track_count())?;
        for a in &self.audio {
            writeln!(
                f,
                "  Audio: {} @ {} Hz, {} ch",
                String::from_utf8_lossy(&a.codec_id),
                a.sample_rate,
                a.channel_layout.channels(),
            )?;
        }
        for v in &self.video {
            writeln!(
                f,
                "  Video: {} {}x{} @ {}.{:03} fps",
                String::from_utf8_lossy(&v.codec_id),
                v.width,
                v.height,
                v.framerate_x1000 / 1000,
                v.framerate_x1000 % 1000,
            )?;
        }
        for c in &self.corruption {
            writeln!(f, "WARNING: {}", c)?;
        }
        Ok(())
    }
}

/// Sniff the container kind from the first few bytes.
pub fn sniff(bytes: &[u8]) -> ContainerKind {
    if bytes.len() >= 4 && &bytes[0..4] == b"TPTM" {
        ContainerKind::Tpt
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        ContainerKind::Wav
    } else {
        ContainerKind::Unknown
    }
}

/// Probe a byte buffer and return a report.
pub fn probe(bytes: &[u8]) -> ProbeReport {
    let mut report = ProbeReport::new();
    report.file_size = bytes.len() as u64;
    report.container = sniff(bytes);
    match report.container {
        ContainerKind::Wav => {
            if let Some(audio) = probe_wav(bytes) {
                report.audio.push(audio);
            } else {
                report.corruption.push("WAV header invalid".into());
            }
        }
        ContainerKind::Tpt => probe_tpt_into(bytes, &mut report),
        ContainerKind::Unknown => {}
    }
    report
}

fn probe_wav(bytes: &[u8]) -> Option<AudioTrackInfo> {
    let header = parse_wav_header(bytes).ok()?;
    let sample_count = wav_sample_count(bytes);
    let bitrate = (header.sample_rate as u64)
        * u64::from(header.channels)
        * u64::from(header.bits_per_sample);
    Some(AudioTrackInfo {
        codec_id: *b"WAV ",
        sample_format: header.sample_format,
        channel_layout: ChannelLayout::from_channels(header.channels as u8),
        sample_rate: header.sample_rate,
        sample_count,
        bitrate: Some(bitrate as u32),
    })
}

fn probe_tpt_into(bytes: &[u8], report: &mut ProbeReport) {
    if bytes.len() < 16 {
        report.corruption.push("TPT file too short".into());
        return;
    }
    if &bytes[0..4] != b"TPTM" {
        report.corruption.push("TPT magic mismatch".into());
        return;
    }
    let timebase_num = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let timebase_den = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    report.global_timebase = Some(Timebase::new(timebase_num, timebase_den));
    let mut pos = 16;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        pos += 8;
        if pos + size > bytes.len() {
            report.corruption.push("truncated chunk".into());
            break;
        }
        let body = &bytes[pos..pos + size];
        if id == b"TRAK" && body.len() >= 5 {
            let codec_id = [body[1], body[2], body[3], body[4]];
            report.audio.push(AudioTrackInfo {
                codec_id,
                sample_format: SampleFormat::S16,
                channel_layout: ChannelLayout::Stereo,
                sample_rate: 48_000,
                sample_count: None,
                bitrate: None,
            });
        }
        pos += size;
        if size & 1 == 1 && pos < bytes.len() {
            pos += 1;
        }
    }
}

struct WavHeader {
    sample_format: SampleFormat,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
}

fn parse_wav_header(bytes: &[u8]) -> Result<WavHeader, ()> {
    if bytes.len() < 44 {
        return Err(());
    }
    if &bytes[0..4] != b"RIFF" {
        return Err(());
    }
    if &bytes[8..12] != b"WAVE" {
        return Err(());
    }
    let format_tag = u16::from_le_bytes([bytes[20], bytes[21]]);
    let channels = u16::from_le_bytes([bytes[22], bytes[23]]);
    let sample_rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    let bits_per_sample = u16::from_le_bytes([bytes[34], bytes[35]]);
    let sample_format = match (format_tag, bits_per_sample) {
        (1, 8) => SampleFormat::U8,
        (1, 16) => SampleFormat::S16,
        (1, 24) => SampleFormat::S24,
        (1, 32) => SampleFormat::S32,
        (3, 32) => SampleFormat::F32,
        (3, 64) => SampleFormat::F64,
        _ => return Err(()),
    };
    Ok(WavHeader {
        sample_format,
        channels,
        sample_rate,
        bits_per_sample,
    })
}

fn wav_sample_count(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 44 {
        return None;
    }
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as u64;
        pos += 8;
        if id == b"data" {
            let hdr = parse_wav_header(bytes).ok()?;
            let bps = (hdr.bits_per_sample / 8) as u64;
            let frame_size = bps * hdr.channels as u64;
            return if frame_size == 0 {
                None
            } else {
                Some(size.checked_div(frame_size)?)
            };
        }
        pos += size as usize;
        if size & 1 == 1 && pos < bytes.len() {
            pos += 1;
        }
    }
    None
}
