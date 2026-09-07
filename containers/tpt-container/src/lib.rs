//! `tpt-container` — the native `.tpt` container format.
//!
//! ## Binary layout (little-endian)
//!
//! ```text
//! FileHeader (16 bytes)
//!   magic        u8[4]    = "TPTM"
//!   version      u16      = 1
//!   header_size  u16      = 16
//!   timebase_num u32
//!   timebase_den u32
//!
//! Chunk (TLV)
//!   chunk_id     u8[4]
//!   chunk_size   u32
//!   body         u8[chunk_size]
//! ```
//!
//! Known chunk types:
//!
//! - `TRAK` — track descriptor: `[track_id:u8, codec_id:u8[4],
//!   codec_private:...]`
//! - `CLUS` — data cluster: `[timestamp:u64, flags:u8, track_id:u8,
//!   payload:...]`. Flags bit 0 = keyframe.
//!
//! See `docs/container-design.md` for the full specification.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::fmt;
use tpt_media_container::{Container, ContainerError, ContainerResult};
use tpt_media_core::{
    CodecConfig, CodecId, MediaType, Packet, PacketFlags, Stream, Timebase, Timestamp, Track,
};

/// The 4-byte magic for `.tpt` files.
pub const MAGIC: [u8; 4] = *b"TPTM";

/// Current container version.
pub const VERSION: u16 = 1;

/// Container-level error type.
#[derive(Debug)]
pub enum TptError {
    /// I/O or EOF issue while reading bytes.
    Truncated,
    /// Header is missing or malformed.
    BadHeader,
    /// A chunk was malformed.
    BadChunk,
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
            TptError::ResourceExceeded => f.write_str("resource limit exceeded"),
            TptError::Other(s) => write!(f, "tpt error: {}", s),
        }
    }
}

impl From<TptError> for ContainerError {
    fn from(_: TptError) -> ContainerError {
        ContainerError::MalformedChunk
    }
}

/// File header.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FileHeader {
    /// Magic bytes.
    pub magic: [u8; 4],
    /// Container version.
    pub version: u16,
    /// Size of this header in bytes.
    pub header_size: u16,
    /// Global timebase.
    pub timebase: Timebase,
}

impl FileHeader {
    /// Build a header with the given timebase.
    pub fn new(timebase: Timebase) -> Self {
        FileHeader {
            magic: MAGIC,
            version: VERSION,
            header_size: 16,
            timebase,
        }
    }

    /// Serialize the header to bytes.
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&self.magic);
        out[4..6].copy_from_slice(&self.version.to_le_bytes());
        out[6..8].copy_from_slice(&self.header_size.to_le_bytes());
        out[8..12].copy_from_slice(&self.timebase.num.to_le_bytes());
        out[12..16].copy_from_slice(&self.timebase.den.to_le_bytes());
        out
    }

    /// Parse a header from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TptError> {
        if bytes.len() < 16 {
            return Err(TptError::BadHeader);
        }
        if &bytes[0..4] != &MAGIC {
            return Err(TptError::BadHeader);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        let header_size = u16::from_le_bytes([bytes[6], bytes[7]]);
        let timebase_num = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let timebase_den = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        Ok(FileHeader {
            magic: MAGIC,
            version,
            header_size,
            timebase: Timebase::new(timebase_num, timebase_den),
        })
    }
}

/// Encode a `TRAK` chunk body.
pub fn encode_trak(track_id: u8, codec_id: &[u8; 4], codec_private: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + codec_private.len());
    out.push(track_id);
    out.extend_from_slice(codec_id);
    out.extend_from_slice(codec_private);
    out
}

/// Encode a `CLUS` chunk body.
pub fn encode_clus(timestamp: u64, flags: u8, track_id: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(10 + payload.len());
    out.extend_from_slice(&timestamp.to_le_bytes());
    out.push(flags);
    out.push(track_id);
    out.extend_from_slice(payload);
    out
}

/// Decode a `CLUS` chunk body.
pub fn decode_clus(body: &[u8]) -> Option<(u64, u8, u8, &[u8])> {
    if body.len() < 10 { return None; }
    let ts = u64::from_le_bytes([body[0], body[1], body[2], body[3], body[4], body[5], body[6], body[7]]);
    let flags = body[8];
    let track_id = body[9];
    Some((ts, flags, track_id, &body[10..]))
}

/// Encode a complete TLV chunk (header + body) into bytes.
pub fn encode_chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Read the next chunk header.
pub fn read_chunk_header(bytes: &[u8], pos: usize) -> Option<([u8; 4], usize, usize)> {
    if pos + 8 > bytes.len() { return None; }
    let mut id = [0u8; 4];
    id.copy_from_slice(&bytes[pos..pos + 4]);
    let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
    Some((id, size, pos + 8))
}

/// Demuxer for `.tpt` byte streams.
pub struct TptDemuxer<'a> {
    bytes: &'a [u8],
    pos: usize,
    header: FileHeader,
    stream: Stream,
    track_indices: BTreeMap<u8, usize>,
    index: Vec<ChunkIndexEntry>,
    cursor: usize,
    seek_to: Option<u64>,
}

#[derive(Copy, Clone, Debug)]
struct ChunkIndexEntry {
    offset: usize,
    keyframe: bool,
    timestamp: u64,
    // Reserved for per-track seeking (todo.md §23); not yet consumed since
    // `next_packet`/`seek_to_keyframe` are single-track-stream oriented.
    #[allow(dead_code)]
    track_id: u8,
}

impl<'a> TptDemuxer<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, TptError> {
        let header = FileHeader::from_bytes(bytes)?;
        let mut d = TptDemuxer {
            bytes,
            pos: header.header_size as usize,
            header,
            stream: Stream::empty(header.timebase),
            track_indices: BTreeMap::new(),
            index: Vec::new(),
            cursor: 0,
            seek_to: None,
        };
        d.scan()?;
        Ok(d)
    }

    fn scan(&mut self) -> Result<(), TptError> {
        while self.pos + 8 <= self.bytes.len() {
            let (id, size, body_start) = match read_chunk_header(self.bytes, self.pos) {
                Some(x) => x,
                None => break,
            };
            if body_start + size > self.bytes.len() {
                return Err(TptError::Truncated);
            }
            let body = &self.bytes[body_start..body_start + size];
            if &id == b"TRAK" {
                self.index_trak(body)?;
            } else if &id == b"CLUS" {
                if let Some((ts, flags, track_id, _payload)) = decode_clus(body) {
                    self.index.push(ChunkIndexEntry {
                        offset: self.pos,
                        keyframe: flags & 0x01 != 0,
                        timestamp: ts,
                        track_id,
                    });
                }
            }
            self.pos = body_start + size;
        }
        Ok(())
    }

    fn index_trak(&mut self, body: &[u8]) -> Result<(), TptError> {
        if body.len() < 5 {
            return Err(TptError::BadChunk);
        }
        let track_id = body[0];
        let mut codec_id = [0u8; 4];
        codec_id.copy_from_slice(&body[1..5]);
        let codec_private = body[5..].to_vec();
        let mut track = Track::new(
            track_id,
            MediaType::Audio,
            CodecId::new(codec_id),
            self.header.timebase,
        );
        track.codec_config = CodecConfig {
            codec_id: CodecId::new(codec_id),
            version: 1,
            data: codec_private,
        };
        if !self.track_indices.contains_key(&track_id) {
            let idx = self.stream.tracks.len();
            self.track_indices.insert(track_id, idx);
            self.stream.tracks.push(track);
        }
        Ok(())
    }
}

impl<'a> Container for TptDemuxer<'a> {
    fn stream(&self) -> &Stream { &self.stream }

    fn next_packet(&mut self) -> ContainerResult<Packet> {
        if let Some(target) = self.seek_to.take() {
            self.cursor = match self.index.iter().position(|e| e.keyframe && e.timestamp >= target) {
                Some(i) => i,
                None => self.index.len(),
            };
        }
        while self.cursor < self.index.len() {
            let entry = self.index[self.cursor];
            self.cursor += 1;
            let (_id, size, body_start) = read_chunk_header(self.bytes, entry.offset)
                .ok_or(ContainerError::MalformedChunk)?;
            let body = &self.bytes[body_start..body_start + size];
            if let Some((ts, flags, track_id, payload)) = decode_clus(body) {
                let mut pkt = Packet::empty(
                    track_id,
                    Timestamp { ticks: ts as i64, timebase: self.header.timebase },
                );
                pkt.dts = pkt.pts;
                pkt.data = payload.to_vec();
                if flags & 0x01 != 0 { pkt.flags |= PacketFlags::KEYFRAME; }
                if flags & 0x02 != 0 { pkt.flags |= PacketFlags::END_OF_FRAME; }
                if flags & 0x04 != 0 { pkt.flags |= PacketFlags::DISCONTINUITY; }
                return Ok(pkt);
            }
        }
        Err(ContainerError::Truncated)
    }

    fn write_packet(&mut self, _packet: &Packet) -> ContainerResult<()> {
        Err(ContainerError::Unsupported("TptDemuxer is read-only"))
    }

    fn finalize(&mut self) -> ContainerResult<()> { Ok(()) }

    fn seek_to_keyframe(&mut self, target_pts: i64) -> ContainerResult<()> {
        self.seek_to = Some(target_pts as u64);
        Ok(())
    }

    fn tracks(&self) -> &[Track] { &self.stream.tracks }
}

/// In-memory muxer for `.tpt` byte streams.
#[derive(Default)]
pub struct TptMuxer {
    bytes: Vec<u8>,
    finalized: bool,
}

impl TptMuxer {
    pub fn new(timebase: Timebase) -> Self {
        let mut m = TptMuxer { bytes: Vec::new(), finalized: false };
        m.bytes.extend_from_slice(&FileHeader::new(timebase).to_bytes());
        m
    }

    pub fn add_track(&mut self, track_id: u8, codec_id: &[u8; 4], codec_private: &[u8]) {
        let body = encode_trak(track_id, codec_id, codec_private);
        self.bytes.extend_from_slice(&encode_chunk(b"TRAK", &body));
    }

    pub fn add_cluster(&mut self, timestamp: u64, flags: u8, track_id: u8, payload: &[u8]) {
        let body = encode_clus(timestamp, flags, track_id, payload);
        self.bytes.extend_from_slice(&encode_chunk(b"CLUS", &body));
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.finalized = true;
        core::mem::take(&mut self.bytes)
    }
}

/// Convenience: build a complete `.tpt` byte buffer.
pub fn build_tpt(timebase: Timebase, tracks: &[(u8, &[u8; 4], &[u8])], packets: &[Packet]) -> Vec<u8> {
    let mut m = TptMuxer::new(timebase);
    for &(id, codec, priv_data) in tracks {
        m.add_track(id, codec, priv_data);
    }
    for p in packets {
        let flags = if p.is_keyframe() { 0x01 } else { 0 }
            | if p.flags.contains(PacketFlags::END_OF_FRAME) { 0x02 } else { 0 }
            | if p.is_discontinuity() { 0x04 } else { 0 };
        let ts = p.pts.ticks as u64;
        m.add_cluster(ts, flags, p.track_id, &p.data);
    }
    m.finish()
}

/// Parse a complete `.tpt` byte buffer.
pub fn parse_tpt(bytes: &[u8]) -> Result<(FileHeader, Vec<Track>, Vec<Packet>), TptError> {
    let mut d = TptDemuxer::new(bytes)?;
    let header = d.header;
    let tracks = d.stream.tracks.clone();
    let mut packets = Vec::new();
    while let Ok(p) = d.next_packet() {
        packets.push(p);
    }
    Ok((header, tracks, packets))
}

/// Iterate over raw chunks.
pub fn for_each_chunk(bytes: &[u8], mut cb: impl FnMut(&[u8; 4], &[u8])) -> Result<(), TptError> {
    let header = FileHeader::from_bytes(bytes)?;
    let mut pos = header.header_size as usize;
    while pos + 8 <= bytes.len() {
        let (id, size, body_start) = read_chunk_header(bytes, pos).ok_or(TptError::BadChunk)?;
        if body_start + size > bytes.len() {
            return Err(TptError::Truncated);
        }
        cb(&id, &bytes[body_start..body_start + size]);
        pos = body_start + size;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip() {
        let h = FileHeader::new(Timebase::MILLISECONDS);
        let b = h.to_bytes();
        let h2 = FileHeader::from_bytes(&b).unwrap();
        assert_eq!(h, h2);
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(b"XXXX");
        assert!(matches!(FileHeader::from_bytes(&b), Err(TptError::BadHeader)));
    }

    #[test]
    fn encode_decode_clus_round_trip() {
        let body = encode_clus(12345, 0x01, 2, &[10, 20, 30, 40]);
        let (ts, flags, tid, payload) = decode_clus(&body).unwrap();
        assert_eq!(ts, 12345);
        assert_eq!(flags, 0x01);
        assert_eq!(tid, 2);
        assert_eq!(payload, &[10, 20, 30, 40]);
    }

    #[test]
    fn full_mux_demux_round_trip() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.add_track(0, b"PULS", &[1, 2, 3]);
        m.add_cluster(100, 0x01, 0, &[0xDE, 0xAD, 0xBE, 0xEF]);
        m.add_cluster(200, 0x00, 0, &[0xCA, 0xFE]);
        let bytes = m.finish();
        let (header, tracks, packets) = parse_tpt(&bytes).unwrap();
        assert_eq!(header.timebase, Timebase::MILLISECONDS);
        assert_eq!(tracks.len(), 1);
        assert_eq!(packets.len(), 2);
        assert!(packets[0].is_keyframe());
        assert!(!packets[1].is_keyframe());
    }

    #[test]
    fn unknown_chunks_are_skipped() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.add_track(0, b"PULS", &[]);
        m.bytes.extend_from_slice(&encode_chunk(b"XYZ!", &[1, 2, 3, 4]));
        m.add_cluster(10, 0x01, 0, &[0xAA]);
        let bytes = m.finish();
        let (_h, _t, packets) = parse_tpt(&bytes).unwrap();
        assert_eq!(packets.len(), 1);
    }

    #[test]
    fn seek_to_keyframe_finds_first_at_or_after_target() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.add_track(0, b"PULS", &[]);
        m.add_cluster(10, 0x00, 0, &[1]);
        m.add_cluster(20, 0x01, 0, &[2]);
        m.add_cluster(30, 0x00, 0, &[3]);
        m.add_cluster(40, 0x01, 0, &[4]);
        let bytes = m.finish();
        let mut d = TptDemuxer::new(&bytes).unwrap();
        d.seek_to_keyframe(25).unwrap();
        let p = d.next_packet().unwrap();
        assert_eq!(p.data, vec![4]);
    }

    #[test]
    fn build_tpt_helper() {
        let tracks: [(u8, &[u8; 4], &[u8]); 1] = [(0, b"PULS", &[0])];
        let packets = vec![
            Packet { data: vec![1,2,3], pts: Timestamp { ticks: 0, timebase: Timebase::MILLISECONDS }, dts: Timestamp { ticks: 0, timebase: Timebase::MILLISECONDS }, flags: PacketFlags::KEYFRAME, track_id: 0, duration: None },
            Packet { data: vec![4,5,6], pts: Timestamp { ticks: 100, timebase: Timebase::MILLISECONDS }, dts: Timestamp { ticks: 100, timebase: Timebase::MILLISECONDS }, flags: PacketFlags::empty(), track_id: 0, duration: None },
        ];
        let bytes = build_tpt(Timebase::MILLISECONDS, &tracks, &packets);
        let (_h, _t, decoded) = parse_tpt(&bytes).unwrap();
        assert_eq!(decoded.len(), 2);
    }

    #[test]
    fn discontinuity_flag_round_trips_through_container() {
        // Regression test for the "second flags bit is actually
        // END_OF_FRAME, not discontinuity" issue from todo.md §22:
        // a packet marked DISCONTINUITY must round-trip through the v1
        // `.tpt` wire format and arrive back with the flag still set.
        let tracks: [(u8, &[u8; 4], &[u8]); 1] = [(0, b"PULS", &[])];
        let packets = vec![
            Packet {
                data: vec![0xAB],
                pts: Timestamp { ticks: 0, timebase: Timebase::MILLISECONDS },
                dts: Timestamp { ticks: 0, timebase: Timebase::MILLISECONDS },
                flags: PacketFlags::DISCONTINUITY | PacketFlags::KEYFRAME,
                track_id: 0,
                duration: None,
            },
            Packet {
                data: vec![0xCD],
                pts: Timestamp { ticks: 100, timebase: Timebase::MILLISECONDS },
                dts: Timestamp { ticks: 100, timebase: Timebase::MILLISECONDS },
                flags: PacketFlags::END_OF_FRAME,
                track_id: 0,
                duration: None,
            },
        ];
        let bytes = build_tpt(Timebase::MILLISECONDS, &tracks, &packets);
        let (_h, _t, decoded) = parse_tpt(&bytes).unwrap();
        assert_eq!(decoded.len(), 2);
        assert!(decoded[0].is_discontinuity());
        assert!(decoded[0].is_keyframe());
        assert!(!decoded[0].is_end_of_frame());
        assert!(!decoded[1].is_discontinuity());
        assert!(decoded[1].is_end_of_frame());
    }
}