//! `.tpt` container format — reader, writer, and keyframe-scan seeking.
//!
//! Layout (all integers little-endian):
//!
//! File Header (16 bytes):
//!   magic     [4] : "TPTM"
//!   version   [2] : 0x0001
//!   hdr_size  [2] : 16
//!   tb_num    [4] : timebase numerator
//!   tb_den    [4] : timebase denominator
//!
//! TRAK chunk (variable):
//!   id        [4] : "TRAK"
//!   size      [4] : payload size in bytes (not counting id+size)
//!   track_id  [1] : 0 video, 1 audio
//!   codec_id  [1]
//!   private   [...] : codec-specific bytes
//!
//! CLUS chunk (variable):
//!   id        [4] : "CLUS"
//!   size      [4] : payload size in bytes
//!   timestamp [8]
//!   flags     [1] : bit 0 = keyframe
//!   track_id  [1]
//!   payload   [...]
//!
//! Seeking: scan the file linearly for the next CLUS chunk whose
//! `is_keyframe` flag is set.

use crate::{Error, Result};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Bytes of the file-header magic ("TPTM").
pub const MAGIC: &[u8; 4] = b"TPTM";
/// Current container version.
pub const VERSION: u16 = 1;
/// Fixed size of the file header (16 bytes).
pub const FILE_HEADER_SIZE: usize = 16;
/// Identifier of the TRAK chunk.
pub const CHUNK_TRAK: &[u8; 4] = b"TRAK";
/// Identifier of the CLUS chunk.
pub const CHUNK_CLUS: &[u8; 4] = b"CLUS";

/// Reserved track identifiers (from spec §3.2).
pub mod track {
    pub const VIDEO: u8 = 0;
    pub const AUDIO: u8 = 1;
}

/// Top-level container: holds parsed headers and a backing byte slice.
pub struct Container<'a> {
    pub timebase_num: u32,
    pub timebase_den: u32,
    pub tracks: Vec<TrackHeader>,
    raw: &'a [u8],
    clusters_start: usize,
}

/// Parsed TRAK chunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackHeader {
    pub track_id: u8,
    pub codec_id: u8,
    pub private: Vec<u8>,
}

/// Parsed CLUS chunk. The payload is borrowed from the underlying file.
#[derive(Clone, Debug)]
pub struct Cluster<'a> {
    pub timestamp: u64,
    pub flags: ClusterFlags,
    pub track_id: u8,
    pub payload: &'a [u8],
}

/// Flags byte of a CLUS chunk. A transparent newtype around `u8` so we
/// don't pull in the `bitflags` crate just for a single bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct ClusterFlags(u8);

impl ClusterFlags {
    pub const IS_KEYFRAME: ClusterFlags = ClusterFlags(0b0000_0001);
    pub const EMPTY: ClusterFlags = ClusterFlags(0);

    pub const fn from_bits_truncate(bits: u8) -> Self { Self(bits) }
    pub const fn bits(&self) -> u8 { self.0 }
    pub const fn empty() -> Self { Self(0) }
    pub const fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl<'a> Container<'a> {
    /// Parse an existing file from a byte slice.
    pub fn parse(raw: &'a [u8]) -> Result<Self> {
        if raw.len() < FILE_HEADER_SIZE {
            return Err(Error::Truncated { needed: FILE_HEADER_SIZE, available: raw.len() });
        }
        if &raw[0..4] != MAGIC {
            return Err(Error::InvalidMagic);
        }
        let version = u16::from_le_bytes([raw[4], raw[5]]);
        if version != VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        let hdr_size = u16::from_le_bytes([raw[6], raw[7]]) as usize;
        if hdr_size != FILE_HEADER_SIZE {
            return Err(Error::MalformedChunk {
                chunk: "FILE",
                reason: "unexpected header size",
            });
        }
        let tb_num = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
        let tb_den = u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]);

        let mut pos = FILE_HEADER_SIZE;
        let mut tracks = Vec::new();
        while pos < raw.len() {
            if pos + 8 > raw.len() {
                return Err(Error::Truncated { needed: 8, available: raw.len() - pos });
            }
            let id = &raw[pos..pos + 4];
            let size = u32::from_le_bytes([raw[pos + 4], raw[pos + 5], raw[pos + 6], raw[pos + 7]]) as usize;
            if pos + 8 + size > raw.len() {
                return Err(Error::Truncated {
                    needed: 8 + size,
                    available: raw.len() - pos,
                });
            }
            if id == CHUNK_TRAK {
                if size < 2 {
                    return Err(Error::MalformedChunk { chunk: "TRAK", reason: "too short" });
                }
                let track_id = raw[pos + 8];
                let codec_id = raw[pos + 9];
                let private = raw[pos + 10..pos + 8 + size].to_vec();
                tracks.push(TrackHeader { track_id, codec_id, private });
                pos += 8 + size;
            } else if id == CHUNK_CLUS {
                break;
            } else {
                return Err(Error::MalformedChunk {
                    chunk: "<unknown>",
                    reason: "unrecognised chunk id",
                });
            }
        }
        Ok(Self { timebase_num: tb_num, timebase_den: tb_den, tracks, raw, clusters_start: pos })
    }

    /// Iterate over every CLUS chunk in the file.
    pub fn clusters(&self) -> ClusterIter<'_> {
        ClusterIter { raw: &self.raw[self.clusters_start..], pos: 0 }
    }

    /// Seek to the first keyframe (CLUS with IS_KEYFRAME set) at or after
    /// the byte offset `from`. Returns the offset of the matching cluster's
    /// CLUS header, or `None` if no keyframe is found.
    pub fn seek_to_keyframe(&self, from: usize) -> Option<usize> {
        let mut pos = from.max(self.clusters_start);
        while pos + 18 <= self.raw.len() {
            if &self.raw[pos..pos + 4] != CHUNK_CLUS {
                return None;
            }
            let size = u32::from_le_bytes([
                self.raw[pos + 4],
                self.raw[pos + 5],
                self.raw[pos + 6],
                self.raw[pos + 7],
            ]) as usize;
            if pos + 8 + size > self.raw.len() { return None; }
            let flags_byte = self.raw[pos + 16];
            if flags_byte & ClusterFlags::IS_KEYFRAME.bits() != 0 {
                return Some(pos);
            }
            pos += 8 + size;
        }
        None
    }

    /// Append a single cluster chunk to `buf`. Returns the number of bytes written.
    pub fn write_cluster(buf: &mut [u8], cluster: &WriteCluster<'_>) -> Result<usize> {
        let payload_len = cluster.payload.len();
        let total = 8 + 8 + 1 + 1 + payload_len;
        if buf.len() < total {
            return Err(Error::Truncated { needed: total, available: buf.len() });
        }
        buf[0..4].copy_from_slice(CHUNK_CLUS);
        let chunk_size = (payload_len + 10) as u32;
        buf[4..8].copy_from_slice(&chunk_size.to_le_bytes());
        buf[8..16].copy_from_slice(&cluster.timestamp.to_le_bytes());
        buf[16] = cluster.flags.bits();
        buf[17] = cluster.track_id;
        buf[18..18 + payload_len].copy_from_slice(cluster.payload);
        Ok(total)
    }

    /// Append the file header to `buf`.
    pub fn write_header(buf: &mut [u8], tb_num: u32, tb_den: u32) -> Result<usize> {
        if buf.len() < FILE_HEADER_SIZE {
            return Err(Error::Truncated { needed: FILE_HEADER_SIZE, available: buf.len() });
        }
        buf[0..4].copy_from_slice(MAGIC);
        buf[4..6].copy_from_slice(&VERSION.to_le_bytes());
        buf[6..8].copy_from_slice(&(FILE_HEADER_SIZE as u16).to_le_bytes());
        buf[8..12].copy_from_slice(&tb_num.to_le_bytes());
        buf[12..16].copy_from_slice(&tb_den.to_le_bytes());
        Ok(FILE_HEADER_SIZE)
    }

    /// Append a TRAK chunk to `buf`.
    pub fn write_track(buf: &mut [u8], track: &TrackHeader) -> Result<usize> {
        let payload_len = 2 + track.private.len();
        let total = 8 + payload_len;
        if buf.len() < total {
            return Err(Error::Truncated { needed: total, available: buf.len() });
        }
        buf[0..4].copy_from_slice(CHUNK_TRAK);
        buf[4..8].copy_from_slice(&(payload_len as u32).to_le_bytes());
        buf[8] = track.track_id;
        buf[9] = track.codec_id;
        buf[10..10 + track.private.len()].copy_from_slice(&track.private);
        Ok(total)
    }
}

/// Cluster being written.
pub struct WriteCluster<'a> {
    pub timestamp: u64,
    pub flags: ClusterFlags,
    pub track_id: u8,
    pub payload: &'a [u8],
}

/// Iterator over [`Cluster`] chunks.
pub struct ClusterIter<'a> {
    raw: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for ClusterIter<'a> {
    type Item = Result<Cluster<'a>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.raw.len() { return None; }
        if self.pos + 18 > self.raw.len() {
            return Some(Err(Error::Truncated { needed: 18, available: self.raw.len() - self.pos }));
        }
        if &self.raw[self.pos..self.pos + 4] != CHUNK_CLUS {
            return Some(Err(Error::MalformedChunk { chunk: "CLUS", reason: "bad id" }));
        }
        let size = u32::from_le_bytes([
            self.raw[self.pos + 4],
            self.raw[self.pos + 5],
            self.raw[self.pos + 6],
            self.raw[self.pos + 7],
        ]) as usize;
        if self.pos + 8 + size > self.raw.len() {
            return Some(Err(Error::Truncated {
                needed: 8 + size,
                available: self.raw.len() - self.pos,
            }));
        }
        let timestamp = u64::from_le_bytes([
            self.raw[self.pos + 8],
            self.raw[self.pos + 9],
            self.raw[self.pos + 10],
            self.raw[self.pos + 11],
            self.raw[self.pos + 12],
            self.raw[self.pos + 13],
            self.raw[self.pos + 14],
            self.raw[self.pos + 15],
        ]);
        let flags = ClusterFlags::from_bits_truncate(self.raw[self.pos + 16]);
        let track_id = self.raw[self.pos + 17];
        let payload = &self.raw[self.pos + 18..self.pos + 8 + size];
        self.pos += 8 + size;
        Some(Ok(Cluster { timestamp, flags, track_id, payload }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_file() -> Vec<u8> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 256];
        let n = Container::write_header(&mut tmp, 1, 1000).unwrap();
        buf.extend_from_slice(&tmp[..n]);
        let trk = TrackHeader { track_id: 0, codec_id: 0x22, private: vec![1, 2, 3] };
        let n = Container::write_track(&mut tmp, &trk).unwrap();
        buf.extend_from_slice(&tmp[..n]);
        for i in 0..4u64 {
            let payload = vec![i as u8; 5];
            let flags = if i == 0 { ClusterFlags::IS_KEYFRAME } else { ClusterFlags::EMPTY };
            let wc = WriteCluster { timestamp: i * 1000, flags, track_id: 0, payload: &payload };
            let n = Container::write_cluster(&mut tmp, &wc).unwrap();
            buf.extend_from_slice(&tmp[..n]);
        }
        buf
    }

    #[test]
    fn header_round_trip() {
        let raw = sample_file();
        let c = Container::parse(&raw).unwrap();
        assert_eq!(c.timebase_num, 1);
        assert_eq!(c.timebase_den, 1000);
        assert_eq!(c.tracks.len(), 1);
        assert_eq!(c.tracks[0].codec_id, 0x22);
        assert_eq!(c.tracks[0].private, vec![1, 2, 3]);
    }

    #[test]
    fn cluster_iteration() {
        let raw = sample_file();
        let c = Container::parse(&raw).unwrap();
        let clusters: Vec<_> = c.clusters().map(|r| r.unwrap()).collect();
        assert_eq!(clusters.len(), 4);
        assert_eq!(clusters[0].timestamp, 0);
        assert!(clusters[0].flags.contains(ClusterFlags::IS_KEYFRAME));
        assert!(!clusters[1].flags.contains(ClusterFlags::IS_KEYFRAME));
    }

    #[test]
    fn seek_to_keyframe() {
        let raw = sample_file();
        let c = Container::parse(&raw).unwrap();
        let _off = c.seek_to_keyframe(0).unwrap();
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bad = vec![0u8; FILE_HEADER_SIZE];
        bad[0..4].copy_from_slice(b"NOPE");
        assert!(Container::parse(&bad).is_err());
    }

    #[test]
    fn rejects_bad_version() {
        let mut bad = vec![0u8; FILE_HEADER_SIZE];
        bad[0..4].copy_from_slice(MAGIC);
        bad[4..6].copy_from_slice(&99u16.to_le_bytes());
        bad[6..8].copy_from_slice(&(FILE_HEADER_SIZE as u16).to_le_bytes());
        assert!(matches!(Container::parse(&bad), Err(Error::UnsupportedVersion(99))));
    }
}