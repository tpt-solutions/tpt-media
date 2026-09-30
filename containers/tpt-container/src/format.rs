//! `tpt-container` wire format: encoders, decoders, constants.
//!
//! All integers are little-endian. The format is a set of top-level TLV
//! chunks preceded by a fixed header. Every chunk length is bounds-checked
//! before it is read, so a corrupted or truncated file can never cause an
//! out-of-bounds access.
//!
//! ```text
//! FileHeader (20 bytes)
//!   magic        u8[4]    "TPTM"
//!   version      u16      = 0x0001 (major = >> 8, minor = & 0xFF)
//!   header_size  u16      = 20      (16 is still readable; chunk parsing
//!                                    starts at header_size)
//!   timebase_num u32                (global timebase numerator)
//!   timebase_den u32                (global timebase denominator)
//!   reserved     u32      = 0       (ignored on read; reserved for future
//!                                    header flags)
//!
//! Chunk (TLV)
//!   chunk_id     u8[4]              (IDs whose first byte is 0x00..=0x1F
//!                                    are reserved and skipped silently)
//!   chunk_size   u32
//!   body         u8[chunk_size]
//! ```

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use tpt_media_core::{
    ChannelLayout, ChannelMask, CodecConfig, CodecId, MediaType, MetadataValue, PixelFormat,
    Timebase, Track, TrackMedia,
};

use crate::crc32::crc32;

/// The 4-byte magic for `.tpt` files.
pub const MAGIC: [u8; 4] = *b"TPTM";

/// Current container version. `major = version >> 8`, `minor = version & 0xFF`.
pub const VERSION: u16 = 0x0001;

/// Minimum supported header size (version 0.1 shipped a 16-byte header).
pub const MIN_HEADER_SIZE: usize = 16;
/// Header size written by the current encoder.
pub const HEADER_SIZE: u16 = 20;

/// Chunk IDs whose first byte is at most this value are reserved.
/// They are skipped silently by readers (see `docs/container-design.md`).
pub const RESERVED_CHUNK_PREFIX: u8 = 0x1F;

/// Track (`TRAK`) chunk with the full track descriptor.
pub const CHUNK_TRAK: [u8; 4] = *b"TRAK";
/// Track metadata (`TMTA`) chunk: typed key/value pairs for one track.
pub const CHUNK_TMTA: [u8; 4] = *b"TMTA";
/// Data cluster (`CLUS`) chunk: one compressed frame.
pub const CHUNK_CLUS: [u8; 4] = *b"CLUS";
/// Per-track index (`IDX `) chunk, written by `finalize()`.
pub const CHUNK_IDX: [u8; 4] = *b"IDX ";
/// Container-level CRC-32 footer chunk, written by `finalize()`.
pub const CHUNK_CRC1: [u8; 4] = *b"CRC1";

/// `CLUS` flags bit 0: packet is a keyframe.
pub const CLUS_KEYFRAME: u8 = 0x01;
/// `CLUS` flags bit 1: packet ends a logical access unit.
pub const CLUS_END_OF_FRAME: u8 = 0x02;
/// `CLUS` flags bit 2: stream discontinuity (splice/resume).
pub const CLUS_DISCONTINUITY: u8 = 0x04;
/// `CLUS` flags bit 3: a `duration` field follows the track ID.
pub const CLUS_HAS_DURATION: u8 = 0x08;
/// `CLUS` flags bit 4: a CRC-32 footer is appended to the payload.
pub const CLUS_HAS_CHECKSUM: u8 = 0x10;

/// `TRAK` flags bit 0: a length-prefixed name field is present.
pub const TRAK_HAS_NAME: u8 = 0x01;
/// `TRAK` flags bit 1: a length-prefixed language field is present.
pub const TRAK_HAS_LANGUAGE: u8 = 0x02;

/// Current on-disk index version.
pub const INDEX_VERSION: u8 = 1;

/// Byte tag for a missing pixel format.
pub const PIXEL_FORMAT_NONE: u8 = 0xFF;

/// Fixed-length prefix of a `CLUS` body (pts, dts, flags, track ID).
pub const CLUS_FIXED_PREFIX: usize = 8 + 8 + 1 + 1;
/// Fixed length of a `TRAK` body excluding codec-private data.
pub const TRAK_FIXED: usize = 1 + 1 + 4 + 2 + 2 + 4 + 4 + 4 + 1 + 2 + 4 + 4 + 1 + 1;

/// Container-level error type.
#[derive(Debug)]
pub enum TptError {
    /// I/O or EOF issue while reading bytes.
    Truncated,
    /// Header is missing or malformed.
    BadHeader,
    /// A chunk was malformed.
    BadChunk,
    /// The file version is not supported by this reader.
    UnsupportedVersion,
    /// Resource limit exceeded while parsing.
    ResourceExceeded,
    /// Other (with description).
    Other(&'static str),
}

impl fmt::Display for TptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TptError::Truncated => f.write_str("truncated"),
            TptError::BadHeader => f.write_str("bad header"),
            TptError::BadChunk => f.write_str("bad chunk"),
            TptError::UnsupportedVersion => f.write_str("unsupported container version"),
            TptError::ResourceExceeded => f.write_str("resource limit exceeded"),
            TptError::Other(s) => write!(f, "tpt error: {}", s),
        }
    }
}

impl From<TptError> for tpt_media_container::ContainerError {
    fn from(e: TptError) -> tpt_media_container::ContainerError {
        use tpt_media_container::ContainerError;
        match e {
            TptError::Truncated => ContainerError::Truncated,
            TptError::UnsupportedVersion => {
                ContainerError::Unsupported("unsupported container version")
            }
            _ => ContainerError::MalformedChunk,
        }
    }
}

/// File header.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FileHeader {
    /// Magic bytes.
    pub magic: [u8; 4],
    /// Container version (major = >> 8, minor = & 0xFF).
    pub version: u16,
    /// Size of this header in bytes. Chunk parsing begins here.
    pub header_size: u16,
    /// Global timebase.
    pub timebase: Timebase,
    /// Reserved header bytes. Must be zero when writing; ignored on read.
    pub reserved: u32,
}

impl FileHeader {
    /// Build a header with the given timebase.
    pub fn new(timebase: Timebase) -> Self {
        FileHeader {
            magic: MAGIC,
            version: VERSION,
            header_size: HEADER_SIZE,
            timebase,
            reserved: 0,
        }
    }

    /// Serialize the header to bytes.
    pub fn to_bytes(&self) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.version.to_le_bytes());
        out[6..8].copy_from_slice(&self.header_size.to_le_bytes());
        out[8..12].copy_from_slice(&self.timebase.num.to_le_bytes());
        out[12..16].copy_from_slice(&self.timebase.den.to_le_bytes());
        out[16..20].copy_from_slice(&self.reserved.to_le_bytes());
        out
    }

    /// Parse a header from bytes.
    ///
    /// Version checking is performed here:
    /// - version must be non-zero and no greater than [`VERSION`];
    /// - `header_size` must be at least the private `MIN_HEADER_SIZE` constant;
    /// - the timebase numerator and denominator must both be non-zero;
    /// - reserved bytes are read if present (16-byte v0.1 headers parse as
    ///   `reserved == 0`) and otherwise ignored (forward compatibility).
    ///
    /// The timebase check is not cosmetic. `Timebase::new` — the constructor
    /// this used to hand the parsed values straight to — documents that a
    /// zero component "panics in debug; in release it is treated as 1/1",
    /// which meant a crafted file either aborted the process (debug) or was
    /// silently re-interpreted as a *different* timebase (release), depending
    /// on the build profile. `decode_trak` already validates its own timebase
    /// and falls back to the header's; the header must be trustworthy for
    /// that fallback to be one. A malformed header is an error, not a panic
    /// three crates downstream.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TptError> {
        if bytes.len() < MIN_HEADER_SIZE {
            return Err(TptError::BadHeader);
        }
        if bytes[..4] != MAGIC {
            return Err(TptError::BadHeader);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        let header_size = u16::from_le_bytes([bytes[6], bytes[7]]);
        if version == 0 || version > VERSION {
            return Err(TptError::UnsupportedVersion);
        }
        if (header_size as usize) < MIN_HEADER_SIZE {
            return Err(TptError::BadHeader);
        }
        let timebase_num = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let timebase_den = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        // See the doc comment: a zero component is rejected here rather than
        // reaching `Timebase::new`, whose contract is to panic (debug) or
        // substitute `1/1` (release) depending on the build profile.
        if timebase_num == 0 || timebase_den == 0 {
            return Err(TptError::BadHeader);
        }
        let reserved = if bytes.len() >= 20 {
            u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]])
        } else {
            0
        };
        Ok(FileHeader {
            magic: MAGIC,
            version,
            header_size,
            timebase: Timebase::new(timebase_num, timebase_den),
            reserved,
        })
    }
}

/// Read the next chunk header at `pos`.
///
/// Returns `(id, size, body_start)`; `None` if fewer than 8 bytes remain.
///
/// `pos` is a caller-supplied offset, and this is a `pub` primitive, so an
/// offset near `usize::MAX` must return `None` rather than overflow. The
/// check is written as a subtraction so it cannot itself overflow: `pos + 8`
/// wraps in release (turning a hostile offset into an in-bounds one that then
/// panics on the slice) and aborts in debug.
pub fn read_chunk_header(bytes: &[u8], pos: usize) -> Option<([u8; 4], usize, usize)> {
    let remaining = bytes.len().checked_sub(pos)?;
    if remaining < 8 {
        return None;
    }
    let mut id = [0u8; 4];
    id.copy_from_slice(&bytes[pos..pos + 4]);
    let size = u32::from_le_bytes([
        bytes[pos + 4],
        bytes[pos + 5],
        bytes[pos + 6],
        bytes[pos + 7],
    ]) as usize;
    Some((id, size, pos + 8))
}

/// Encode a complete TLV chunk (header + body) into bytes.
pub fn encode_chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// `true` if a chunk ID falls in the reserved range (first byte `0x00..=0x1F`).
///
/// Reserved chunks are skipped silently by readers per the compatibility
/// rules in `docs/container-design.md`.
pub fn chunk_id_is_reserved(id: &[u8; 4]) -> bool {
    id[0] <= RESERVED_CHUNK_PREFIX
}

/// Encode a `TRAK` chunk body from a track descriptor.
pub fn encode_trak(track: &Track) -> Vec<u8> {
    let mut out = Vec::with_capacity(TRAK_FIXED + track.codec_config.data.len() + 512);
    out.push(track.id);
    out.push(track.media_type as u8);
    out.extend_from_slice(&track.codec_config.codec_id.as_bytes());
    out.extend_from_slice(&track.codec_config.version.to_le_bytes());
    let priv_data = &track.codec_config.data;
    out.extend_from_slice(&(priv_data.len() as u16).to_le_bytes());
    out.extend_from_slice(priv_data);
    out.extend_from_slice(&track.timebase.num.to_le_bytes());
    out.extend_from_slice(&track.timebase.den.to_le_bytes());

    let media = &track.media;
    out.extend_from_slice(&media.sample_rate.unwrap_or(0).to_le_bytes());
    match media.channel_layout {
        None => {
            out.push(0);
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        Some(ChannelLayout::Mono) => {
            out.push(1);
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        Some(ChannelLayout::Stereo) => {
            out.push(2);
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        Some(ChannelLayout::Surround51) => {
            out.push(3);
            out.extend_from_slice(&0u16.to_le_bytes());
        }
        Some(ChannelLayout::Custom(m)) => {
            out.push(4);
            out.extend_from_slice(&m.bits().to_le_bytes());
        }
    }
    out.extend_from_slice(&media.width.unwrap_or(0).to_le_bytes());
    out.extend_from_slice(&media.height.unwrap_or(0).to_le_bytes());
    out.push(media.pixel_format.map_or(PIXEL_FORMAT_NONE, |p| p as u8));

    let mut flags = 0u8;
    if track.name.is_some() {
        flags |= TRAK_HAS_NAME;
    }
    if track.language.is_some() {
        flags |= TRAK_HAS_LANGUAGE;
    }
    out.push(flags);
    if let Some(name) = &track.name {
        write_lp_string(&mut out, name);
    }
    if let Some(lang) = &track.language {
        write_lp_string(&mut out, lang);
    }
    out
}

/// Decode a `TRAK` chunk body into a track descriptor.
///
/// The track's timebase falls back to `header_timebase` when the serialized
/// timebase is missing or invalid (zero numerator or denominator).
pub fn decode_trak(body: &[u8], header_timebase: Timebase) -> Option<Track> {
    const FIXED: usize = 1 + 1 + 4 + 2 + 2 + 4 + 4 + 4 + 1 + 2 + 4 + 4 + 1 + 1;
    if body.len() < FIXED {
        return None;
    }
    let track_id = body[0];
    let media_tag = body[1];
    let media_type = match media_tag {
        0 => MediaType::Audio,
        1 => MediaType::Video,
        2 => MediaType::Subtitle,
        3 => MediaType::Metadata,
        _ => return None,
    };
    let mut codec_bytes = [0u8; 4];
    codec_bytes.copy_from_slice(&body[2..6]);
    let codec_id = CodecId::new(codec_bytes);
    let codec_version = u16::from_le_bytes([body[6], body[7]]);
    let priv_len = u16::from_le_bytes([body[8], body[9]]) as usize;
    let mut p = 10;
    if body.len() < p + priv_len + 25 {
        return None;
    }
    let codec_private = body[p..p + priv_len].to_vec();
    p += priv_len;

    let tb_num = u32::from_le_bytes([body[p], body[p + 1], body[p + 2], body[p + 3]]);
    let tb_den = u32::from_le_bytes([body[p + 4], body[p + 5], body[p + 6], body[p + 7]]);
    p += 8;
    let timebase = if tb_num != 0 && tb_den != 0 {
        Timebase::new(tb_num, tb_den)
    } else {
        header_timebase
    };

    let sample_rate = u32::from_le_bytes([body[p], body[p + 1], body[p + 2], body[p + 3]]);
    p += 4;
    let channel_tag = body[p];
    p += 1;
    let channel_mask = u16::from_le_bytes([body[p], body[p + 1]]);
    p += 2;
    let channel_layout = match channel_tag {
        0 => None,
        1 => Some(ChannelLayout::Mono),
        2 => Some(ChannelLayout::Stereo),
        3 => Some(ChannelLayout::Surround51),
        4 => Some(ChannelLayout::Custom(ChannelMask::from_bits(channel_mask))),
        _ => None,
    };
    let width = u32::from_le_bytes([body[p], body[p + 1], body[p + 2], body[p + 3]]);
    p += 4;
    let height = u32::from_le_bytes([body[p], body[p + 1], body[p + 2], body[p + 3]]);
    p += 4;
    let pixel = body[p];
    p += 1;
    let pixel_format = match pixel {
        PIXEL_FORMAT_NONE => None,
        v => pixel_format_from_u8(v),
    };
    let flags = body[p];
    p += 1;

    let mut name = None;
    if flags & TRAK_HAS_NAME != 0 {
        let (s, next) = read_lp_string(body, p)?;
        name = Some(s);
        p = next;
    }
    let mut language = None;
    if flags & TRAK_HAS_LANGUAGE != 0 {
        let (s, _next) = read_lp_string(body, p)?;
        language = Some(s);
    }

    Some(Track {
        id: track_id,
        codec_config: CodecConfig {
            codec_id,
            version: codec_version,
            data: codec_private,
        },
        media_type,
        timebase,
        media: TrackMedia {
            sample_rate: (sample_rate != 0).then_some(sample_rate),
            channel_layout,
            width: (width != 0).then_some(width),
            height: (height != 0).then_some(height),
            pixel_format,
        },
        name,
        language,
        metadata: Vec::new(),
    })
}

/// Write a length-prefixed string (`len: u8`, then UTF-8 bytes).
fn write_lp_string(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    out.push(core::cmp::min(bytes.len(), 255) as u8);
    out.extend_from_slice(&bytes[..core::cmp::min(bytes.len(), 255)]);
}

/// Read a length-prefixed string.
fn read_lp_string(bytes: &[u8], p: usize) -> Option<(String, usize)> {
    let len = *bytes.get(p)? as usize;
    let end = p + 1 + len;
    if end > bytes.len() {
        return None;
    }
    let s = core::str::from_utf8(&bytes[p + 1..end])
        .unwrap_or_default()
        .to_string();
    Some((s, end))
}

fn pixel_format_from_u8(v: u8) -> Option<PixelFormat> {
    match v {
        0 => Some(PixelFormat::Gray8),
        1 => Some(PixelFormat::Rgb24),
        2 => Some(PixelFormat::Rgba32),
        3 => Some(PixelFormat::Yuv420),
        4 => Some(PixelFormat::Nv12),
        _ => None,
    }
}

/// A decoded `CLUS` body.
#[derive(Clone, Debug)]
pub struct DecodedCluster<'a> {
    /// Presentation timestamp, in the track timebase.
    pub pts: u64,
    /// Decode timestamp, in the track timebase.
    pub dts: u64,
    /// Cluster flags (see `CLUS_*`).
    pub flags: u8,
    /// Owning track ID.
    pub track_id: u8,
    /// Optional frame duration, in the track timebase.
    pub duration: Option<u32>,
    /// Compressed payload.
    pub payload: &'a [u8],
    /// `true` when there was no checksum or the embedded CRC-32 matched.
    pub crc_valid: bool,
}

/// Encode a `CLUS` chunk body.
///
/// `flags` should be `CLUS_KEYFRAME | CLUS_END_OF_FRAME | CLUS_DISCONTINUITY`
/// (the `HAS_DURATION` / `HAS_CHECKSUM` bits are derived from `duration` /
/// `checksum`). The checksum, when enabled, covers the whole body up to but
/// excluding the checksum field itself.
pub fn encode_clus(
    pts: u64,
    dts: u64,
    flags: u8,
    track_id: u8,
    duration: Option<u32>,
    checksum: bool,
    payload: &[u8],
) -> Vec<u8> {
    let mut flags = flags;
    let mut out = Vec::with_capacity(CLUS_FIXED_PREFIX + 4 + payload.len() + 4);
    out.extend_from_slice(&pts.to_le_bytes());
    out.extend_from_slice(&dts.to_le_bytes());
    if duration.is_some() {
        flags |= CLUS_HAS_DURATION;
    }
    if checksum {
        flags |= CLUS_HAS_CHECKSUM;
    }
    out.push(flags);
    out.push(track_id);
    if let Some(d) = duration {
        out.extend_from_slice(&d.to_le_bytes());
    }
    out.extend_from_slice(payload);
    if checksum {
        out.extend_from_slice(&crc32(&out).to_le_bytes());
    }
    out
}

/// Decode a `CLUS` chunk body.
pub fn decode_clus(body: &[u8]) -> Option<DecodedCluster<'_>> {
    if body.len() < CLUS_FIXED_PREFIX {
        return None;
    }
    let pts = u64::from_le_bytes(body[0..8].try_into().ok()?);
    let dts = u64::from_le_bytes(body[8..16].try_into().ok()?);
    let flags = body[16];
    let track_id = body[17];
    let has_duration = flags & CLUS_HAS_DURATION != 0;
    let has_checksum = flags & CLUS_HAS_CHECKSUM != 0;
    let prefix = CLUS_FIXED_PREFIX + if has_duration { 4 } else { 0 };
    let payload_end = body.len() - if has_checksum { 4 } else { 0 };
    if prefix > payload_end {
        return None;
    }
    let duration = if has_duration {
        let d = u32::from_le_bytes(
            body[CLUS_FIXED_PREFIX..CLUS_FIXED_PREFIX + 4]
                .try_into()
                .ok()?,
        );
        Some(d)
    } else {
        None
    };
    let crc = if has_checksum {
        u32::from_le_bytes(body[payload_end..body.len()].try_into().ok()?)
    } else {
        0
    };
    let crc_valid = !has_checksum || crc == crc32(&body[..body.len() - 4]);
    Some(DecodedCluster {
        pts,
        dts,
        flags,
        track_id,
        duration,
        payload: &body[prefix..payload_end],
        crc_valid,
    })
}

/// Value type tags for `TMTA` entries.
const META_TEXT: u8 = 0;
const META_INT: u8 = 1;
const META_BOOL: u8 = 2;
const META_BYTES: u8 = 3;

/// Encode a `TMTA` chunk body: typed metadata pairs for one track.
pub fn encode_tmta(track_id: u8, metadata: &[(String, MetadataValue)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(track_id);
    out.extend_from_slice(&(metadata.len() as u16).to_le_bytes());
    for (key, value) in metadata {
        write_lp_string(&mut out, key);
        match value {
            MetadataValue::Text(s) => {
                out.push(META_TEXT);
                let bytes = s.as_bytes();
                // Interior UTF-8 string; length is a u16 because metadata
                // values are unbounded, unlike name/language.
                out.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
                out.extend_from_slice(bytes);
            }
            MetadataValue::Int(i) => {
                out.push(META_INT);
                out.extend_from_slice(&(*i as u64).to_le_bytes());
            }
            MetadataValue::Bool(b) => {
                out.push(META_BOOL);
                out.push(*b as u8);
            }
            MetadataValue::Bytes(b) => {
                out.push(META_BYTES);
                out.extend_from_slice(&(b.len() as u32).to_le_bytes());
                out.extend_from_slice(b);
            }
        }
    }
    out
}

/// Decode a `TMTA` chunk body.
pub fn decode_tmta(body: &[u8]) -> Option<(u8, Vec<(String, MetadataValue)>)> {
    if body.len() < 3 {
        return None;
    }
    let track_id = body[0];
    let count = u16::from_le_bytes([body[1], body[2]]) as usize;
    let mut p = 3;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let (key, next) = read_lp_string(body, p)?;
        p = next;
        let value_type = *body.get(p)?;
        p += 1;
        let value = match value_type {
            META_TEXT => {
                let (s, next) = read_meta_str(body, p)?;
                p = next;
                MetadataValue::Text(s)
            }
            META_INT => {
                let end = p + 8;
                if end > body.len() {
                    return None;
                }
                let i = i64::from_le_bytes(body[p..end].try_into().ok()?);
                p = end;
                MetadataValue::Int(i)
            }
            META_BOOL => {
                let b = *body.get(p)?;
                p += 1;
                MetadataValue::Bool(b != 0)
            }
            META_BYTES => {
                if p + 4 > body.len() {
                    return None;
                }
                let len = u32::from_le_bytes(body[p..p + 4].try_into().ok()?) as usize;
                p += 4;
                // `len` is a fully attacker-controlled `u32` (up to
                // ~4.29 billion); `checked_add` (rather than `+`) keeps
                // this bounds check correct even on 32-bit `usize`
                // targets, where `p + len` could otherwise wrap around
                // and pass a check it should have failed.
                let end = p.checked_add(len)?;
                if end > body.len() {
                    return None;
                }
                let b = body[p..end].to_vec();
                p = end;
                MetadataValue::Bytes(b)
            }
            _ => return None,
        };
        out.push((key, value));
    }
    Some((track_id, out))
}

/// Read a u16-length-prefixed metadata string.
fn read_meta_str(bytes: &[u8], p: usize) -> Option<(String, usize)> {
    if p + 2 > bytes.len() {
        return None;
    }
    let len = u16::from_le_bytes([bytes[p], bytes[p + 1]]) as usize;
    let end = p + 2 + len;
    if end > bytes.len() {
        return None;
    }
    let s = core::str::from_utf8(&bytes[p + 2..end])
        .unwrap_or_default()
        .to_string();
    Some((s, end))
}

/// One entry of the on-disk index.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    /// Presentation timestamp, in the track timebase.
    pub pts: u64,
    /// Absolute byte offset of the `CLUS` chunk header.
    pub offset: u64,
    /// `true` if the cluster is a keyframe.
    pub keyframe: bool,
}

/// A decoded `IDX ` chunk.
#[derive(Clone, Debug)]
pub struct IndexFile {
    /// On-disk index version ([`INDEX_VERSION`]).
    pub version: u8,
    /// Per-track index entries.
    pub tracks: BTreeMap<u8, Vec<IndexEntry>>,
}

/// Encode an on-disk per-track index (`IDX ` body).
pub fn encode_idx(version: u8, tracks: &BTreeMap<u8, Vec<IndexEntry>>) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(version);
    out.push(tracks.len() as u8);
    for (track_id, entries) in tracks {
        out.push(*track_id);
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for e in entries {
            out.extend_from_slice(&e.pts.to_le_bytes());
            out.extend_from_slice(&e.offset.to_le_bytes());
            out.push(if e.keyframe { 1 } else { 0 });
        }
    }
    out
}

/// Decode an on-disk per-track index (`IDX ` body).
pub fn decode_idx(body: &[u8]) -> Option<IndexFile> {
    if body.len() < 2 {
        return None;
    }
    let version = body[0];
    let track_count = body[1] as usize;
    let mut p = 2;
    let mut tracks = BTreeMap::new();
    for _ in 0..track_count {
        let track_id = *body.get(p)?;
        p += 1;
        if p + 4 > body.len() {
            return None;
        }
        let entry_count = u32::from_le_bytes(body[p..p + 4].try_into().ok()?) as usize;
        p += 4;
        // §47: `entry_count` is a fully attacker-controlled `u32` (up to
        // ~4.29 billion); each entry is a fixed 17 bytes, so an
        // `entry_count` claiming more entries than could possibly fit in
        // the remaining body is malformed. Reject it before
        // pre-allocating for it, rather than attempting a
        // decompression-bomb-style multi-gigabyte allocation from a
        // tiny input buffer.
        if entry_count > body.len().saturating_sub(p) / 17 {
            return None;
        }
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            if p + 17 > body.len() {
                return None;
            }
            let pts = u64::from_le_bytes(body[p..p + 8].try_into().ok()?);
            let offset = u64::from_le_bytes(body[p + 8..p + 16].try_into().ok()?);
            let keyframe = body[p + 16] != 0;
            p += 17;
            entries.push(IndexEntry {
                pts,
                offset,
                keyframe,
            });
        }
        tracks.insert(track_id, entries);
    }
    Some(IndexFile { version, tracks })
}
