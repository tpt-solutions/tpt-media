//! `tpt-media-probe` — container and codec introspection.
//!
//! Given raw bytes (e.g. the contents of a file), `probe` identifies the
//! container format and extracts the most important metadata: tracks, codecs,
//! sample rates, dimensions, timestamps, durations, metadata, integrity and
//! the availability of codec implementations.
//!
//! For `.tpt` files the report is produced from the container's real
//! `TRAK`/`TMTA` descriptors and a full packet scan, so every field reflects
//! what is actually in the file rather than a guess.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use tpt_container::TptDemuxer;
use tpt_media_codec::Registry;
use tpt_media_container::Container;
use tpt_media_core::{
    ChannelLayout, CodecId, MediaType, MetadataValue, PixelFormat, SampleFormat, Timebase,
    Timestamp, Track,
};

/// Probed media container kind.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ContainerKind {
    /// A `.tpt` native container (`TPTM` magic).
    Tpt,
    /// A RIFF/WAVE file.
    Wav,
    /// Anything else, or the input was empty.
    #[default]
    Unknown,
}

/// A probed audio track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioTrackInfo {
    /// Container-local track ID.
    pub id: u8,
    /// The 4-byte codec id from the track descriptor.
    pub codec_id: CodecId,
    /// Bitstream version of the codec in the file (`major << 8 | minor`).
    pub codec_version: u16,
    /// Sample format; only reported when the container or codec carries it
    /// (`.tpt` does not serialize it, so this is `None` for `.tpt`).
    pub sample_format: Option<SampleFormat>,
    /// Channel arrangement from the track descriptor.
    pub channel_layout: Option<ChannelLayout>,
    /// Sample rate in Hz from the track descriptor.
    pub sample_rate: Option<u32>,
    /// Best-effort sample count derived from duration × sample rate.
    pub sample_count: Option<u64>,
    /// Estimated compressed-audio bitrate in bits/second.
    pub bitrate: Option<u64>,
    /// Number of packets in this track.
    pub frame_count: Option<u64>,
    /// First and last packet timestamps (track timebase).
    /// PTS of the first packet on the track.
    pub first_pts: Option<Timestamp>,
    /// PTS of the last packet on the track.
    pub last_pts: Option<Timestamp>,
    /// Human track name, when tagged.
    pub name: Option<String>,
    /// BCP-47 language tag, when tagged.
    pub language: Option<String>,
    /// Free-form typed metadata pairs from the `TMTA` chunk.
    pub metadata: Vec<(String, MetadataValue)>,
}

/// A probed video track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoTrackInfo {
    /// Container-local track ID.
    pub id: u8,
    /// The 4-byte codec id from the track descriptor.
    pub codec_id: CodecId,
    /// Bitstream version of the codec in the file.
    pub codec_version: u16,
    /// Frame width in pixels, when the track carries it.
    pub width: Option<u32>,
    /// Frame height in pixels, when the track carries it.
    pub height: Option<u32>,
    /// Pixel layout, when the track carries one.
    pub pixel_format: Option<PixelFormat>,
    /// Frame rate × 1000, estimated from packet spacing.
    pub framerate_x1000: Option<u32>,
    /// Number of packets on the track.
    pub frame_count: Option<u64>,
    /// Estimated compressed bitrate in bits/second.
    pub bitrate: Option<u64>,
    /// PTS of the first packet on the track.
    pub first_pts: Option<Timestamp>,
    /// PTS of the last packet on the track.
    pub last_pts: Option<Timestamp>,
    /// Human track name, when tagged.
    pub name: Option<String>,
    /// BCP-47 language tag, when tagged.
    pub language: Option<String>,
    /// Free-form typed metadata pairs from the `TMTA` chunk.
    pub metadata: Vec<(String, MetadataValue)>,
}

/// A probed non-audio/video track (subtitle or metadata).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackInfo {
    /// Container-local track ID.
    pub id: u8,
    /// Subtitle or metadata.
    pub media_type: MediaType,
    /// The 4-byte codec id from the track descriptor.
    pub codec_id: CodecId,
    /// Bitstream version of the codec in the file.
    pub codec_version: u16,
    /// Number of packets on the track.
    pub frame_count: Option<u64>,
    /// Estimated bitrate in bits/second.
    pub bitrate: Option<u64>,
    /// PTS of the first packet on the track.
    pub first_pts: Option<Timestamp>,
    /// PTS of the last packet on the track.
    pub last_pts: Option<Timestamp>,
    /// Human track name, when tagged.
    pub name: Option<String>,
    /// BCP-47 language tag, when tagged.
    pub language: Option<String>,
    /// Free-form typed metadata pairs from the `TMTA` chunk.
    pub metadata: Vec<(String, MetadataValue)>,
}

/// Capabilities reported for one codec ID found in the file (the "report
/// codec capabilities" item): identity, whether a registered implementation
/// is available, and the framework-level capability flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecCapabilityReport {
    /// The 4-byte codec id found in the file.
    pub codec_id: CodecId,
    /// Human name (registered implementation, or the known native family).
    pub name: Option<&'static str>,
    /// `true` for a TPT-native codec ID.
    pub native: bool,
    /// `true` if a decoder implementation is registered in the codec `Registry`.
    pub decoder_available: bool,
    /// `true` if an encoder implementation is registered in the codec `Registry`.
    pub encoder_available: bool,
    /// The registered implementation declares lossless capability.
    pub lossless: bool,
    /// The registered implementation declares lossy capability.
    pub lossy: bool,
    /// The registered implementation declares real-time decode.
    pub real_time_safe: bool,
}

/// Combined probe result.
#[derive(Clone, Debug, Default)]
pub struct ProbeReport {
    /// The identified container kind.
    pub container: ContainerKind,
    /// Input size in bytes.
    pub file_size: u64,
    /// Container version (`.tpt` only), major = `>> 8`, minor = `& 0xFF`.
    pub version: Option<u16>,
    /// The `.tpt` header's global timebase.
    pub global_timebase: Option<Timebase>,
    /// Longest playback across all tracks, in seconds.
    pub duration_seconds: Option<f64>,
    /// Overall compressed bitrate in bits/second.
    pub bitrate_bits_per_second: Option<u64>,
    /// Container-level `CRC1` footer check: `None` when no footer is present.
    pub checksum_ok: Option<bool>,
    /// Per-cluster checksum failures as `(track_id, chunk_offset)`.
    pub cluster_checksum_failures: Vec<(u8, u64)>,
    /// Every probed audio track.
    pub audio: Vec<AudioTrackInfo>,
    /// Every probed video track.
    pub video: Vec<VideoTrackInfo>,
    /// All subtitle and metadata tracks.
    pub other: Vec<TrackInfo>,
    /// One report per distinct codec id found in the file.
    pub codec_capabilities: Vec<CodecCapabilityReport>,
    /// Human-readable problems found while probing (parse errors,
    /// truncation, checksum mismatches). Never panics on malformed input.
    pub corruption: Vec<String>,
}

impl ProbeReport {
    /// An empty report (the `Default`).
    pub fn new() -> Self {
        ProbeReport::default()
    }
    /// `true` when the container could not be identified.
    pub fn is_unknown(&self) -> bool {
        self.container == ContainerKind::Unknown
    }
    /// Total tracks across all media types.
    pub fn track_count(&self) -> usize {
        self.audio.len() + self.video.len() + self.other.len()
    }
    /// `true` if the container-level and per-cluster checksums all pass.
    pub fn integrity_ok(&self) -> bool {
        self.checksum_ok != Some(false) && self.cluster_checksum_failures.is_empty()
    }
}

impl fmt::Display for ProbeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Container: {:?}", self.container)?;
        if let Some(v) = self.version {
            writeln!(f, "Container version: {}.{}", v >> 8, v & 0xFF)?;
        }
        writeln!(f, "File size: {} bytes", self.file_size)?;
        if let Some(tb) = self.global_timebase {
            writeln!(f, "Timebase: {}", tb)?;
        }
        if let Some(d) = self.duration_seconds {
            writeln!(f, "Duration: {:.3} s", d)?;
        }
        if let Some(b) = self.bitrate_bits_per_second {
            writeln!(f, "Bitrate: {} bits/s", b)?;
        }
        match self.checksum_ok {
            Some(true) => writeln!(f, "Container checksum: ok")?,
            Some(false) => writeln!(f, "Container checksum: MISMATCH")?,
            None => {}
        }
        if !self.cluster_checksum_failures.is_empty() {
            writeln!(
                f,
                "Per-cluster checksums: {} failure(s)",
                self.cluster_checksum_failures.len()
            )?;
        }
        writeln!(f, "Tracks: {}", self.track_count())?;
        for a in &self.audio {
            writeln!(
                f,
                "  Audio #{}: {} v{}.{} @ {} Hz, {} ch, {} pkt(s), {:.3} s",
                a.id,
                a.codec_id,
                a.codec_version >> 8,
                a.codec_version & 0xFF,
                a.sample_rate.map_or(String::from("?"), |r| r.to_string()),
                a.channel_layout
                    .map_or(String::from("?"), |l| l.channels().to_string()),
                a.frame_count.unwrap_or(0),
                // first/last PTS are in the track's own timebase —
                // convert to seconds for the "{:.3} s" label (both
                // timestamps share the track timebase, so either carries
                // it).
                a.last_pts.zip(a.first_pts).map_or(0.0, |(l, first)| {
                    let tb = l.timebase;
                    (l.ticks.saturating_sub(first.ticks) as f64) * (tb.num as f64) / (tb.den as f64)
                }),
            )?;
        }
        for v in &self.video {
            writeln!(
                f,
                "  Video #{}: {} v{}.{} {}x{} @ {}.{:03} fps, {} pkt(s)",
                v.id,
                v.codec_id,
                v.codec_version >> 8,
                v.codec_version & 0xFF,
                v.width.map_or(String::from("?"), |w| w.to_string()),
                v.height.map_or(String::from("?"), |h| h.to_string()),
                v.framerate_x1000.unwrap_or(0) / 1000,
                v.framerate_x1000.unwrap_or(0) % 1000,
                v.frame_count.unwrap_or(0),
            )?;
        }
        for t in &self.other {
            writeln!(
                f,
                "  {:?} #{}: {} v{}.{}, {} pkt(s)",
                t.media_type,
                t.id,
                t.codec_id,
                t.codec_version >> 8,
                t.codec_version & 0xFF,
                t.frame_count.unwrap_or(0),
            )?;
        }
        for c in &self.codec_capabilities {
            writeln!(
                f,
                "  Codec {}: native={}, decoder={}, encoder={}, lossless={}, lossy={}, rt={}",
                c.codec_id,
                c.native,
                c.decoder_available,
                c.encoder_available,
                c.lossless,
                c.lossy,
                c.real_time_safe,
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
///
/// Capability availability is queried against an empty codec [`Registry`];
/// use [`probe_with_registry`] when implementations are registered.
pub fn probe(bytes: &[u8]) -> ProbeReport {
    probe_with_registry(bytes, &Registry::new())
}

/// Probe a byte buffer, resolving codec capabilities against `registry`.
pub fn probe_with_registry(bytes: &[u8], registry: &Registry) -> ProbeReport {
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
        ContainerKind::Tpt => probe_tpt_into(bytes, &mut report, registry),
        ContainerKind::Unknown => {}
    }
    report
}

/// Probe a file from disk. Requires the `std` feature (on by default).
#[cfg(feature = "std")]
pub fn probe_file<P: AsRef<std::path::Path>>(path: P) -> Result<ProbeReport, std::io::Error> {
    let bytes = std::fs::read(path)?;
    Ok(probe(&bytes))
}

fn probe_wav(bytes: &[u8]) -> Option<AudioTrackInfo> {
    let header = parse_wav_header(bytes).ok()?;
    let sample_count = wav_sample_count(bytes);
    let bitrate = (header.sample_rate as u64)
        * u64::from(header.channels)
        * u64::from(header.bits_per_sample);
    Some(AudioTrackInfo {
        id: 0,
        codec_id: CodecId::new(*b"WAV "),
        codec_version: 1,
        sample_format: Some(header.sample_format),
        channel_layout: Some(ChannelLayout::from_channels(header.channels as u8)),
        sample_rate: Some(header.sample_rate),
        sample_count,
        bitrate: Some(bitrate),
        frame_count: None,
        first_pts: None,
        last_pts: None,
        name: None,
        language: None,
        metadata: Vec::new(),
    })
}

fn probe_tpt_into(bytes: &[u8], report: &mut ProbeReport, registry: &Registry) {
    let mut demuxer = match TptDemuxer::new(bytes) {
        Ok(d) => d,
        Err(err) => {
            report.corruption.push(format!("TPT parse error: {}", err));
            if let Ok(header) = tpt_container::FileHeader::from_bytes(bytes) {
                report.version = Some(header.version);
                report.global_timebase = Some(header.timebase);
            }
            return;
        }
    };

    let header = *demuxer.header();
    report.version = Some(header.version);
    report.global_timebase = Some(header.timebase);

    report.checksum_ok = demuxer.verify_checksum();
    match demuxer.verify_cluster_checksums() {
        Ok(()) => {}
        Err(bad) => {
            report.cluster_checksum_failures = bad.iter().map(|&(t, o)| (t, o as u64)).collect();
            report
                .corruption
                .push(format!("{} cluster checksum failure(s)", bad.len()));
        }
    }
    if report.checksum_ok == Some(false) {
        report.corruption.push("container checksum mismatch".into());
    }

    let tracks: Vec<Track> = demuxer.tracks().to_vec();
    let mut accums: BTreeMap<u8, Acc> = BTreeMap::new();
    let mut total_payload: u64 = 0;
    while let Ok(packet) = demuxer.next_packet() {
        let a = accums.entry(packet.track_id).or_default();
        a.count += 1;
        a.payload += packet.data.len() as u64;
        if let Some(d) = packet.duration {
            a.last_duration = Some(d.ticks as u64);
        }
        let ticks = packet.pts.ticks.max(0) as u64;
        a.pts_timebase = Some(packet.pts.timebase);
        a.first_pts = Some(a.first_pts.map_or(ticks, |f| f.min(ticks)));
        a.last_pts = Some(a.last_pts.map_or(ticks, |l| l.max(ticks)));
        total_payload += packet.data.len() as u64;
    }

    let duration_seconds = tracks
        .iter()
        .filter_map(|t| accums.get(&t.id).and_then(Acc::duration_seconds))
        .reduce(|a, b| a.max(b));
    report.duration_seconds = duration_seconds;
    if let Some(secs) = duration_seconds {
        if secs > 0.0 {
            report.bitrate_bits_per_second = Some((total_payload as f64 * 8.0 / secs) as u64);
        }
    }

    for track in &tracks {
        let acc = accums.get(&track.id).copied().unwrap_or_default();
        let codec_id = track.codec_config.codec_id;
        let dur = acc.duration_seconds();
        let bitrate = acc.bitrate_bits(dur);
        let tb = acc
            .pts_timebase
            .unwrap_or_else(|| report.global_timebase.unwrap_or(Timebase::ONE_PER_SECOND));
        let first_pts = acc.first_pts.map(|t| Timestamp {
            ticks: t as i64,
            timebase: tb,
        });
        let last_pts = acc.last_pts.map(|t| Timestamp {
            ticks: t as i64,
            timebase: tb,
        });

        let base = TrackBase {
            id: track.id,
            codec_id,
            codec_version: track.codec_config.version,
            frame_count: (acc.count > 0).then_some(acc.count),
            bitrate,
            first_pts,
            last_pts,
            name: track.name.clone(),
            language: track.language.clone(),
            metadata: track.metadata.clone(),
        };
        match track.media_type {
            MediaType::Audio => report.audio.push(AudioTrackInfo {
                id: base.id,
                codec_id: base.codec_id,
                codec_version: base.codec_version,
                sample_format: None,
                channel_layout: track.media.channel_layout,
                sample_rate: track.media.sample_rate,
                sample_count: track
                    .media
                    .sample_rate
                    .zip(dur)
                    .map(|(r, s)| ((r as f64) * s).round().max(0.0) as u64),
                bitrate: base.bitrate,
                frame_count: base.frame_count,
                first_pts: base.first_pts,
                last_pts: base.last_pts,
                name: base.name,
                language: base.language,
                metadata: base.metadata,
            }),
            MediaType::Video => report.video.push(VideoTrackInfo {
                id: base.id,
                codec_id: base.codec_id,
                codec_version: base.codec_version,
                width: track.media.width,
                height: track.media.height,
                pixel_format: track.media.pixel_format,
                framerate_x1000: acc.framerate_x1000(),
                frame_count: base.frame_count,
                bitrate: base.bitrate,
                first_pts: base.first_pts,
                last_pts: base.last_pts,
                name: base.name,
                language: base.language,
                metadata: base.metadata,
            }),
            other => report.other.push(TrackInfo {
                id: base.id,
                media_type: other,
                codec_id: base.codec_id,
                codec_version: base.codec_version,
                frame_count: base.frame_count,
                bitrate: base.bitrate,
                first_pts: base.first_pts,
                last_pts: base.last_pts,
                name: base.name,
                language: base.language,
                metadata: base.metadata,
            }),
        }

        // Report capabilities once per codec ID.
        if !report
            .codec_capabilities
            .iter()
            .any(|c| c.codec_id == codec_id)
        {
            report
                .codec_capabilities
                .push(codec_capability(codec_id, registry));
        }
    }
}

/// Fields common to every track kind.
struct TrackBase {
    id: u8,
    codec_id: CodecId,
    codec_version: u16,
    frame_count: Option<u64>,
    bitrate: Option<u64>,
    first_pts: Option<Timestamp>,
    last_pts: Option<Timestamp>,
    name: Option<String>,
    language: Option<String>,
    metadata: Vec<(String, MetadataValue)>,
}

/// Per-track packet accumulation during the scan.
#[derive(Copy, Clone, Default)]
struct Acc {
    count: u64,
    payload: u64,
    pts_timebase: Option<Timebase>,
    first_pts: Option<u64>,
    last_pts: Option<u64>,
    last_duration: Option<u64>,
}

impl Acc {
    /// Playback duration of this track in seconds (last PTS + duration, or
    /// average frame spacing when the last cluster has no duration).
    fn duration_seconds(&self) -> Option<f64> {
        let tb = self.pts_timebase?;
        let first = self.first_pts?;
        let last = self.last_pts?;
        if !tb.is_valid() {
            return None;
        }
        let end = last
            + self.last_duration.unwrap_or_else(|| {
                if self.count > 1 {
                    (last - first) / (self.count - 1)
                } else {
                    0
                }
            });
        Some((end as f64) * (tb.num as f64) / (tb.den as f64))
    }

    fn bitrate_bits(&self, seconds: Option<f64>) -> Option<u64> {
        let secs = seconds?;
        if secs <= 0.0 {
            return None;
        }
        Some((self.payload as f64 * 8.0 / secs) as u64)
    }

    /// Frame rate × 1000, estimated from packet spacing.
    fn framerate_x1000(&self) -> Option<u32> {
        let tb = self.pts_timebase?;
        let first = self.first_pts?;
        let last = self.last_pts?;
        let n = self.count;
        if n < 2 || last <= first || !tb.is_valid() {
            return None;
        }
        let span = ((last - first) as f64) * (tb.num as f64) / (tb.den as f64);
        if span <= 0.0 {
            return None;
        }
        let x1000 = (((n - 1) as f64) / span * 1000.0).round() as u32;
        (x1000 > 0).then_some(x1000)
    }
}

/// Known TPT-native codec families (identity + display name only; real
/// capabilities come from the codec `Registry` when implementations register).
const NATIVE_CODECS: &[(&[u8; 4], &str)] = &[
    (b"CIND", "Cinder"),
    (b"AURA", "Aura"),
    (b"REED", "Reed"),
    (b"THRM", "Thrum"),
    (b"VAUL", "Vault"),
    (b"MOTE", "Mote"),
];

fn native_name(id: [u8; 4]) -> Option<&'static str> {
    NATIVE_CODECS
        .iter()
        .find(|(codec, _)| **codec == id)
        .map(|(_, name)| *name)
}

fn codec_capability(codec_id: CodecId, registry: &Registry) -> CodecCapabilityReport {
    let bytes = codec_id.as_bytes();
    let decoder = registry.find_decoder(&bytes);
    let encoder = registry.find_encoder(&bytes);
    let info = decoder
        .map(|d| d.info())
        .or_else(|| encoder.map(|e| e.info()));
    CodecCapabilityReport {
        codec_id,
        name: info.as_ref().map(|i| i.name).or_else(|| native_name(bytes)),
        native: codec_id.is_native(),
        decoder_available: decoder.is_some(),
        encoder_available: encoder.is_some(),
        lossless: info.as_ref().is_some_and(|i| i.capabilities.lossless),
        lossy: info.as_ref().is_some_and(|i| i.capabilities.lossy),
        real_time_safe: info
            .as_ref()
            .is_some_and(|i| i.capabilities.real_time_decoder || i.capabilities.real_time_encoder),
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use tpt_container::{encode_chunk, read_chunk_header, TptMuxer};
    use tpt_media_codec::{
        Capabilities, CodecConfig, CodecError, CodecInfo, CodecResult, DynDecoder,
        DynDecoderFactory, DynEncoder, DynEncoderFactory,
    };
    use tpt_media_core::{
        ChannelLayout, CodecId, MediaType, MetadataValue, Packet, PacketFlags, PixelFormat,
        SampleFormat,
    };

    fn timebase() -> Timebase {
        Timebase::new(1, 48_000)
    }

    fn audio_track() -> Track {
        let mut t = Track::new(0, MediaType::Audio, CodecId::new(*b"CIND"), timebase());
        t.codec_config.version = 0x01_00;
        t.media.sample_rate = Some(48_000);
        t.media.channel_layout = Some(ChannelLayout::Stereo);
        t.name = Some("main".into());
        t.language = Some("en".into());
        t.metadata = vec![
            ("encoder".into(), MetadataValue::Text("tpt v1".into())),
            ("lossless".into(), MetadataValue::Bool(true)),
        ];
        t
    }

    fn video_track() -> Track {
        let mut t = Track::new(1, MediaType::Video, CodecId::new(*b"THRM"), timebase());
        t.codec_config.version = 0x00_03;
        t.media.width = Some(1280);
        t.media.height = Some(720);
        t.media.pixel_format = Some(PixelFormat::Nv12);
        t
    }

    fn subtitle_track() -> Track {
        Track::new(2, MediaType::Subtitle, CodecId::new(*b"SUB "), timebase())
    }

    fn sample_buf() -> Vec<u8> {
        (0..16).map(|i| (i as u8).wrapping_mul(7)).collect()
    }

    /// A finalized container with two audio packets and one video packet.
    fn build_container() -> Vec<u8> {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.register_track(&audio_track());
        m.register_track(&video_track());
        let mut p = Packet::empty(
            0,
            Timestamp {
                ticks: 0,
                timebase: timebase(),
            },
        );
        for i in 0..10 {
            p.pts.ticks = i * 480;
            p.dts.ticks = i * 480;
            p.data = sample_buf();
            if i == 0 {
                p.flags |= PacketFlags::KEYFRAME;
            }
            m.write_packet_raw(&p).unwrap();
        }
        let mut vp = Packet::empty(
            1,
            Timestamp {
                ticks: 0,
                timebase: timebase(),
            },
        );
        vp.data = vec![0xAB; 32];
        vp.flags |= PacketFlags::KEYFRAME;
        m.write_packet_raw(&vp).unwrap();
        m.finish()
    }

    #[test]
    fn sniff_detects_containers() {
        assert_eq!(sniff(b"TPTM\x00\x01"), ContainerKind::Tpt);
        assert_eq!(sniff(b"RIFF\x24\x00\x00\x00WAVE"), ContainerKind::Wav);
        assert_eq!(sniff(b"random bytes"), ContainerKind::Unknown);
        assert_eq!(sniff(b""), ContainerKind::Unknown);
    }

    #[test]
    fn probe_full_tpt_report() {
        let bytes = build_container();
        let report = probe(&bytes);
        assert_eq!(report.container, ContainerKind::Tpt);
        assert_eq!(report.version, Some(0x0001));
        assert_eq!(report.global_timebase, Some(Timebase::MILLISECONDS));
        assert!(report.corruption.is_empty());
        assert_eq!(report.checksum_ok, Some(true));
        assert!(report.cluster_checksum_failures.is_empty());
        assert!(report.track_count() == 2);

        let a = &report.audio[0];
        assert_eq!(a.id, 0);
        assert_eq!(a.codec_id, CodecId::new(*b"CIND"));
        assert_eq!(a.codec_version, 0x01_00);
        assert_eq!(a.sample_format, None);
        assert_eq!(a.channel_layout, Some(ChannelLayout::Stereo));
        assert_eq!(a.sample_rate, Some(48_000));
        assert_eq!(a.frame_count, Some(10));
        assert_eq!(
            a.first_pts,
            Some(Timestamp {
                ticks: 0,
                timebase: timebase()
            })
        );
        assert_eq!(
            a.last_pts,
            Some(Timestamp {
                ticks: 4320,
                timebase: timebase()
            })
        );
        assert_eq!(a.name.as_deref(), Some("main"));
        assert_eq!(a.language.as_deref(), Some("en"));
        assert!(a.metadata.iter().any(|(k, _)| k.as_str() == "encoder"));
        assert!(a.bitrate.is_some());

        let v = &report.video[0];
        assert_eq!(v.id, 1);
        assert_eq!(v.codec_id, CodecId::new(*b"THRM"));
        assert_eq!(v.codec_version, 0x00_03);
        assert_eq!(v.width, Some(1280));
        assert_eq!(v.height, Some(720));
        assert_eq!(v.pixel_format, Some(PixelFormat::Nv12));
        assert_eq!(v.frame_count, Some(1));

        assert!(report.duration_seconds.unwrap() > 0.0);
        assert!(report.bitrate_bits_per_second.unwrap() > 0);

        let caps: Vec<_> = report
            .codec_capabilities
            .iter()
            .map(|c| c.codec_id)
            .collect();
        assert_eq!(caps, vec![CodecId::new(*b"CIND"), CodecId::new(*b"THRM")]);
        assert!(report.codec_capabilities[0].native);
        assert!(!report.codec_capabilities[0].decoder_available);
    }

    #[test]
    fn probe_audio_duration_matches_last_packet() {
        let bytes = build_container();
        let report = probe(&bytes);
        // Last audio PTS is 4320/48000 s plus a ~20ms correction.
        let dur = report.duration_seconds.unwrap();
        assert!(dur >= 0.089, "duration  too short: {}", dur);
        assert!(dur < 0.12, "duration too long: {}", dur);
    }

    #[test]
    fn probe_truncated_container_reports_corruption() {
        let bytes = build_container();
        // Cut into the trailer's CRC chunk body so the strict parser sees a
        // partial chunk (a truncation that stops at a chunk boundary is a
        // valid-but-unfinalized stream and is *not* corruption).
        let truncated = &bytes[..bytes.len() - 1];
        let report = probe(truncated);
        assert!(report
            .corruption
            .iter()
            .any(|c| c.contains("truncated") || c.contains("parse error")));
        assert_eq!(report.version, Some(0x0001));
    }

    #[test]
    fn probe_bad_magic_is_unknown() {
        let mut bytes = build_container();
        bytes[0] = b'Z';
        let report = probe(&bytes);
        assert_eq!(report.container, ContainerKind::Unknown);
        assert!(report.corruption.is_empty());
    }

    #[test]
    fn probe_tptm_magic_with_short_tail_reports_parse_error() {
        // Sniffing sees the magic, but the buffer cannot be a header.
        let report = probe(b"TPTM");
        assert_eq!(report.container, ContainerKind::Tpt);
        assert!(report.corruption.iter().any(|c| c.contains("parse error")));
        assert!(report.version.is_none());
    }

    #[test]
    fn probe_unsupported_version_reports_corruption() {
        let mut bytes = build_container();
        bytes[4] = 0xFA;
        bytes[5] = 0x00;
        let report = probe(&bytes);
        assert!(report.corruption.iter().any(|c| c.contains("version")));
        assert_eq!(report.version, None);
    }

    #[test]
    fn probe_skips_reserved_chunks_without_corruption() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.register_track(&audio_track());
        let mut p = Packet::empty(
            0,
            Timestamp {
                ticks: 0,
                timebase: timebase(),
            },
        );
        p.data = sample_buf();
        m.write_packet_raw(&p).unwrap();
        m.append_raw(&encode_chunk(&[0x00, 0x01, 0x02, 0x03], &[0xAA, 0xBB]));
        let bytes = m.finish();
        let report = probe(&bytes);
        assert!(report.corruption.is_empty());
        assert_eq!(report.audio.len(), 1);
    }

    #[test]
    fn probe_reports_cluster_checksum_corruption() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.enable_cluster_checksums();
        m.register_track(&audio_track());
        let mut p = Packet::empty(
            0,
            Timestamp {
                ticks: 0,
                timebase: timebase(),
            },
        );
        p.data = sample_buf();
        m.write_packet_raw(&p).unwrap();
        let mut bytes = m.finish();
        assert!(
            flip_first_payload_byte(&mut bytes),
            "could not find CLUS payload"
        );
        let report = probe(&bytes);
        assert!(!report.cluster_checksum_failures.is_empty());
        assert!(!report.integrity_ok());
        assert!(report.corruption.iter().any(|c| c.contains("checksum")));
    }

    #[test]
    fn probe_wav_regression() {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&44_100u32.to_le_bytes());
        wav.extend_from_slice(&(44_100u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(800u32).to_le_bytes());
        wav.extend_from_slice(&[0u8; 800]);

        let report = probe(&wav);
        assert_eq!(report.container, ContainerKind::Wav);
        assert_eq!(report.audio.len(), 1);
        let a = &report.audio[0];
        assert_eq!(a.sample_rate, Some(44_100));
        assert_eq!(a.sample_format, Some(SampleFormat::S16));
        assert_eq!(a.channel_layout, Some(ChannelLayout::Stereo));
        assert_eq!(a.sample_count, Some(200));
        assert_eq!(a.bitrate, Some(44_100 * 16 * 2));
        assert!(report.corruption.is_empty());
    }

    #[test]
    fn probe_header_only_container_has_no_packets() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.register_track(&audio_track());
        let bytes = m.finish();
        let report = probe(&bytes);
        assert_eq!(report.version, Some(0x0001));
        assert_eq!(report.audio.len(), 1);
        assert_eq!(report.audio[0].frame_count, None);
        assert_eq!(report.audio[0].first_pts, None);
        assert!(report.duration_seconds.is_none());
        assert!(report.bitrate_bits_per_second.is_none());
        assert!(report.corruption.is_empty());
    }

    #[test]
    fn probe_empty_and_tiny_inputs_do_not_panic() {
        let empty = probe(&[]);
        assert!(empty.is_unknown());
        assert_eq!(empty.file_size, 0);
        let short = probe(&[0u8; 4]);
        assert!(short.is_unknown());
    }

    #[test]
    fn subtitle_and_metadata_tracks_land_in_other() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.register_track(&subtitle_track());
        let bytes = m.finish();
        let report = probe(&bytes);
        assert_eq!(report.other.len(), 1);
        assert_eq!(report.other[0].media_type, MediaType::Subtitle);
        assert_eq!(report.other[0].codec_id, CodecId::new(*b"SUB "));
    }

    // ---- codec capability reporting via a populated registry ----------

    struct DummyFactory;

    impl DynDecoderFactory for DummyFactory {
        fn codec_id(&self) -> &[u8; 4] {
            b"TTST"
        }
        fn info(&self) -> CodecInfo {
            CodecInfo {
                name: "Test",
                codec_id: *b"TTST",
                version: 2,
                capabilities: Capabilities {
                    lossless: true,
                    real_time_decoder: true,
                    ..Default::default()
                },
                supported_formats: Vec::new(),
            }
        }
        fn build(&self, _cfg: &CodecConfig) -> CodecResult<Box<dyn DynDecoder>> {
            Err(CodecError::Unsupported("test only"))
        }
    }

    impl DynEncoderFactory for DummyFactory {
        fn codec_id(&self) -> &[u8; 4] {
            b"TTST"
        }
        fn info(&self) -> CodecInfo {
            CodecInfo {
                name: "Test",
                codec_id: *b"TTST",
                version: 2,
                capabilities: Capabilities {
                    lossless: true,
                    ..Default::default()
                },
                supported_formats: Vec::new(),
            }
        }
        fn build(&self, _cfg: &CodecConfig) -> CodecResult<Box<dyn DynEncoder>> {
            Err(CodecError::Unsupported("test only"))
        }
    }

    #[test]
    fn probe_reports_registered_codec_capabilities() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        let mut t = Track::new(0, MediaType::Audio, CodecId::new(*b"TTST"), timebase());
        t.codec_config.version = 2;
        m.register_track(&t);
        let bytes = m.finish();

        let mut registry = Registry::new();
        registry.register_decoder(DummyFactory);
        registry.register_encoder(DummyFactory);

        let report = probe_with_registry(&bytes, &registry);
        assert_eq!(report.codec_capabilities.len(), 1);
        let cap = &report.codec_capabilities[0];
        assert_eq!(cap.codec_id, CodecId::new(*b"TTST"));
        assert_eq!(cap.name, Some("Test"));
        assert!(cap.decoder_available);
        assert!(cap.encoder_available);
        assert!(cap.lossless);
        assert!(cap.real_time_safe);
        assert!(!cap.lossy);
    }

    #[test]
    #[cfg(feature = "std")]
    fn probe_file_round_trip() {
        let bytes = build_container();
        let path = {
            let mut dir = std::env::temp_dir();
            let unique = format!(
                "tpt-probe-test-{}-{}.tpt",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            dir.push(unique);
            dir
        };
        std::fs::write(&path, &bytes).unwrap();
        let report = probe_file(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(report.container, ContainerKind::Tpt);
        assert_eq!(report.track_count(), 2);
    }

    /// Flip one byte inside the payload of the first `CLUS` chunk.
    fn flip_first_payload_byte(bytes: &mut [u8]) -> bool {
        let mut pos = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        while pos + 8 <= bytes.len() {
            let (id, size, body_start) = match read_chunk_header(bytes, pos) {
                Some(x) => x,
                None => return false,
            };
            let body_end = match body_start.checked_add(size) {
                Some(e) if e <= bytes.len() => e,
                _ => return false,
            };
            if id == *b"CLUS" {
                if body_end - body_start < 19 {
                    return false;
                }
                // Payload starts after the fixed 18-byte prefix (no duration).
                let hit = body_start + 18;
                bytes[hit] ^= 0xFF;
                return true;
            }
            pos = body_end;
        }
        false
    }

    #[test]
    fn probe_never_panics_on_corrupted_or_arbitrary_bytes() {
        // §38 "probe fuzzing": a deterministic corruption sweep, in the
        // same spirit as `tpt-container`'s
        // `corruption_never_panics_and_truncation_never_panics` (§24) —
        // no external fuzz-harness dependency. `probe()` never returns a
        // `Result` (corruption is reported *in* `ProbeReport`, e.g.
        // `integrity_ok()`), so the only thing to check here is that it
        // never panics.
        let valid = build_container();
        assert!(valid.len() > 32, "fixture too small: {}", valid.len());

        // Truncation sweep: every prefix length.
        for cut in 0..valid.len() {
            let _ = probe(&valid[..cut]);
        }

        // Deterministic LCG bit-flip sweep.
        let mut state: u64 = 0x1656_67B1_9E37_79F9;
        for _ in 0..512 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let pos = (state as usize) % valid.len();
            let bit = 1u8 << ((state >> 32) % 8);
            let mut corrupt = valid.clone();
            corrupt[pos] ^= bit;
            let _ = probe(&corrupt);
        }

        // Arbitrary bytes entirely unrelated to any real container.
        let mut state: u64 = 0xC0FF_EE12_3456_789A;
        for _ in 0..256 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let len = (state % 200) as usize;
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                bytes.push(state as u8);
            }
            let _ = probe(&bytes);
        }

        // The pristine container still probes cleanly after the sweep.
        let report = probe(&valid);
        assert_eq!(report.container, ContainerKind::Tpt);
        assert_eq!(report.track_count(), 2);
    }
}
