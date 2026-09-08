//! `TptDemuxer` — strict and streaming readers for the `.tpt` container.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use tpt_media_container::{Container, ContainerError, ContainerResult};
use tpt_media_core::{Packet, PacketFlags, Stream, Timestamp, Track};

use crate::crc32::crc32;
use crate::format::{
    chunk_id_is_reserved, decode_clus, decode_idx, decode_tmta, decode_trak, read_chunk_header,
    FileHeader, IndexFile, TptError, CHUNK_CLUS, CHUNK_CRC1, CHUNK_IDX, CHUNK_TMTA, CHUNK_TRAK,
    CLUS_KEYFRAME,
};

/// One cluster recorded by the scan (and subsequently the in-memory index).
#[derive(Copy, Clone, Debug)]
pub struct ChunkIndexEntry {
    /// Absolute byte offset of the `CLUS` chunk header.
    pub offset: usize,
    /// `true` if the cluster is a keyframe.
    pub keyframe: bool,
    /// Presentation timestamp (in the track timebase).
    pub pts: u64,
    /// Owning track ID.
    pub track_id: u8,
}

/// Demuxer for `.tpt` byte streams.
///
/// Construct with [`TptDemuxer::new`] for a complete file (a trailing partial
/// chunk is an error) or [`TptDemuxer::new_streaming`] + [`TptDemuxer::feed`]
/// for a file that is still being appended to (live streams, fragmented
/// writing).
pub struct TptDemuxer<'a> {
    bytes: &'a [u8],
    pos: usize,
    header: FileHeader,
    stream: Stream,
    track_indices: BTreeMap<u8, usize>,
    index: Vec<ChunkIndexEntry>,
    per_track: BTreeMap<u8, Vec<ChunkIndexEntry>>,
    disk_index: Option<IndexFile>,
    crc_footer: Option<(usize, u32)>,
    cursor: usize,
    seek_to: Option<u64>,
    strict: bool,
}

impl<'a> TptDemuxer<'a> {
    /// Parse a complete `.tpt` buffer. A truncated trailing chunk is an error.
    pub fn new(bytes: &'a [u8]) -> Result<Self, TptError> {
        Self::create(bytes, true)
    }

    /// Parse a `.tpt` buffer that may end in the middle of a chunk.
    ///
    /// Scanning stops at the first incomplete chunk; call [`feed`] with the
    /// extended buffer once more bytes are available.
    ///
    /// [`feed`]: TptDemuxer::feed
    pub fn new_streaming(bytes: &'a [u8]) -> Result<Self, TptError> {
        Self::create(bytes, false)
    }

    fn create(bytes: &'a [u8], strict: bool) -> Result<Self, TptError> {
        let header = FileHeader::from_bytes(bytes)?;
        if header.header_size as usize > bytes.len() {
            return if strict {
                Err(TptError::Truncated)
            } else {
                // Streaming: nothing can be parsed yet, but the buffer may
                // grow to hold the header.
                Ok(TptDemuxer::blank(header, bytes, strict))
            };
        }
        let mut d = TptDemuxer {
            bytes,
            pos: header.header_size as usize,
            header,
            stream: Stream::empty(header.timebase),
            track_indices: BTreeMap::new(),
            index: Vec::new(),
            per_track: BTreeMap::new(),
            disk_index: None,
            crc_footer: None,
            cursor: 0,
            seek_to: None,
            strict,
        };
        d.scan()?;
        Ok(d)
    }

    fn blank(header: FileHeader, bytes: &'a [u8], strict: bool) -> Self {
        TptDemuxer {
            bytes,
            pos: header.header_size as usize,
            header,
            stream: Stream::empty(header.timebase),
            track_indices: BTreeMap::new(),
            index: Vec::new(),
            per_track: BTreeMap::new(),
            disk_index: None,
            crc_footer: None,
            cursor: 0,
            seek_to: None,
            strict,
        }
    }

    /// Resume scanning with an extended buffer (live append).
    ///
    /// The new slice must extend the previous one: it must be at least as
    /// long and share the whole previous prefix. This restores *streaming
    /// playback* for fragmented writes and live streams.
    pub fn feed(&mut self, bytes: &'a [u8]) -> Result<(), TptError> {
        if bytes.len() < self.bytes.len() {
            return Err(TptError::Other("buffer shrank"));
        }
        if &bytes[..self.bytes.len()] != self.bytes {
            return Err(TptError::Other("buffer prefix changed"));
        }
        if bytes.len() == self.bytes.len() {
            return Ok(());
        }
        self.bytes = bytes;
        self.scan()
    }

    /// Scan (or resume scanning) every complete chunk.
    fn scan(&mut self) -> Result<(), TptError> {
        while self.pos + 8 <= self.bytes.len() {
            let (id, size, body_start) = match read_chunk_header(self.bytes, self.pos) {
                Some(x) => x,
                None => break,
            };
            let body_end = match body_start.checked_add(size) {
                Some(e) => e,
                None => return Err(TptError::BadChunk),
            };
            if body_end > self.bytes.len() {
                if self.strict {
                    return Err(TptError::Truncated);
                }
                // Streaming: wait for the rest of this chunk.
                break;
            }
            let body = &self.bytes[body_start..body_end];
            if id == CHUNK_TRAK {
                self.index_trak(body)?;
            } else if id == CHUNK_TMTA {
                self.index_tmta(body)?;
            } else if id == CHUNK_CLUS {
                if let Some(cluster) = decode_clus(body) {
                    self.index.push(ChunkIndexEntry {
                        offset: self.pos,
                        keyframe: (cluster.flags & CLUS_KEYFRAME) != 0,
                        pts: cluster.pts,
                        track_id: cluster.track_id,
                    });
                }
            } else if id == CHUNK_IDX {
                if let Some(file) = decode_idx(body) {
                    // Index versioning: only an understood version is used.
                    if file.version == crate::format::INDEX_VERSION {
                        self.disk_index = Some(file);
                    }
                }
            } else if id == CHUNK_CRC1 {
                if body.len() == 4 {
                    let value = u32::from_le_bytes([body[0], body[1], body[2], body[3]]);
                    self.crc_footer = Some((self.pos, value));
                }
            } else if chunk_id_is_reserved(&id) {
                // Reserved range: skip silently (compatibility rule).
            }
            self.pos = body_end;
        }
        Ok(())
    }

    fn index_trak(&mut self, body: &[u8]) -> Result<(), TptError> {
        let track = decode_trak(body, self.header.timebase).ok_or(TptError::BadChunk)?;
        if !self.track_indices.contains_key(&track.id) {
            let idx = self.stream.tracks.len();
            self.track_indices.insert(track.id, idx);
            self.stream.tracks.push(track);
        }
        Ok(())
    }

    fn index_tmta(&mut self, body: &[u8]) -> Result<(), TptError> {
        let (track_id, entries) = decode_tmta(body).ok_or(TptError::BadChunk)?;
        if let Some(&idx) = self.track_indices.get(&track_id) {
            self.stream.tracks[idx].metadata = entries;
        }
        Ok(())
    }

    /// Per-track scan index (rebuilt on [`rebuild_index`]).
    ///
    /// [`rebuild_index`]: TptDemuxer::rebuild_index
    pub fn track_index(&self, track_id: u8) -> Option<&[ChunkIndexEntry]> {
        self.per_track.get(&track_id).map(Vec::as_slice)
    }

    /// The on-disk index chunk, when present and of a supported version.
    pub fn disk_index(&self) -> Option<&IndexFile> {
        self.disk_index.as_ref()
    }

    /// The parsed file header.
    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    /// Rebuild the in-memory index from a full scan (recovery).
    ///
    /// Used when the on-disk `IDX ` chunk is missing or corrupt: packets and
    /// seeking continue to work from the scan index. The result is not
    /// written back to the buffer unless the caller does so.
    pub fn rebuild_index(&mut self) {
        let mut per_track: BTreeMap<u8, Vec<ChunkIndexEntry>> = BTreeMap::new();
        for e in &self.index {
            per_track.entry(e.track_id).or_default().push(*e);
        }
        self.per_track = per_track;
    }

    /// Build the trailer (`IDX ` + `CRC1`) for the current byte buffer.
    ///
    /// This supports *optional footer/index reconstruction*: a stream that
    /// ends without a trailer can have one reconstructed in memory, and the
    /// returned bytes can be appended to the file to turn it into a fully
    /// finalized container.
    pub fn reconstruct_trailer(&self) -> Vec<u8> {
        use crate::format::{encode_chunk, encode_idx, CHUNK_IDX, INDEX_VERSION};
        let mut tracks: BTreeMap<u8, Vec<crate::format::IndexEntry>> = BTreeMap::new();
        for e in &self.index {
            tracks
                .entry(e.track_id)
                .or_default()
                .push(crate::format::IndexEntry {
                    pts: e.pts,
                    offset: e.offset as u64,
                    keyframe: e.keyframe,
                });
        }
        let mut out = encode_chunk(&CHUNK_IDX, &encode_idx(INDEX_VERSION, &tracks));
        // The footer covers every byte up to the CRC chunk itself: the live
        // stream *plus* the index chunk just appended.
        let mut cover = alloc::vec::Vec::with_capacity(self.bytes.len() + out.len());
        cover.extend_from_slice(self.bytes);
        cover.extend_from_slice(&out);
        let crc = crc32(&cover);
        out.extend_from_slice(&crate::format::encode_chunk(
            &CHUNK_CRC1,
            &crc.to_le_bytes(),
        ));
        out
    }

    /// Verify the container-level `CRC1` footer, if present.
    ///
    /// Returns `None` when the buffer has no footer. A footer that fails to
    /// match indicates the trailer was not regenerated after the file was
    /// modified (or the file is corrupt).
    pub fn verify_checksum(&self) -> Option<bool> {
        let (offset, value) = self.crc_footer?;
        Some(crc32(&self.bytes[..offset]) == value)
    }

    /// Verify the optional per-cluster checksums, if any.
    ///
    /// Clusters without a checksum field are skipped. Mismatches are counted
    /// and reported with the track/offset that failed so a strict consumer
    /// can decide what to do.
    pub fn verify_cluster_checksums(&self) -> Result<(), Vec<(u8, usize)>> {
        let mut bad = Vec::new();
        for e in &self.index {
            let (_id, size, body_start) = read_chunk_header(self.bytes, e.offset)
                .ok_or_else(|| vec![(e.track_id, e.offset)])?;
            if let Some(cluster) = decode_clus(&self.bytes[body_start..body_start + size]) {
                if !cluster.crc_valid {
                    bad.push((e.track_id, e.offset));
                }
            }
        }
        if bad.is_empty() {
            Ok(())
        } else {
            Err(bad)
        }
    }

    /// Move the read cursor to the first cluster at `offset` or later
    /// (offset-based lookup, §23).
    pub fn seek_to_offset(&mut self, offset: usize) -> ContainerResult<()> {
        self.cursor = self
            .index
            .iter()
            .position(|e| e.offset >= offset)
            .unwrap_or(self.index.len());
        Ok(())
    }

    /// Move the read cursor to the first keyframe at or after `target_pts`
    /// on a specific track (per-track seeking, §23).
    pub fn seek_to_keyframe_on_track(
        &mut self,
        track_id: u8,
        target_pts: i64,
    ) -> ContainerResult<()> {
        self.rebuild_index();
        let entries = self
            .per_track
            .get(&track_id)
            .ok_or(ContainerError::MalformedChunk)?;
        let pos = entries
            .iter()
            .position(|e| e.keyframe && e.pts as i64 >= target_pts)
            .unwrap_or(entries.len());
        let entry = entries.get(pos).ok_or(ContainerError::Truncated)?;
        self.cursor = self
            .index
            .iter()
            .position(|e| e.offset >= entry.offset)
            .ok_or(ContainerError::Truncated)?;
        Ok(())
    }
}

impl<'a> Container for TptDemuxer<'a> {
    fn stream(&self) -> &Stream {
        &self.stream
    }

    fn next_packet(&mut self) -> ContainerResult<Packet> {
        if let Some(target) = self.seek_to.take() {
            self.cursor = match self
                .index
                .iter()
                .position(|e| e.keyframe && e.pts >= target)
            {
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
            if let Some(cluster) = decode_clus(body) {
                let timebase = self
                    .stream
                    .track(cluster.track_id)
                    .map(|t| t.timebase)
                    .unwrap_or(self.header.timebase);
                let mut pkt = Packet::empty(
                    cluster.track_id,
                    Timestamp {
                        ticks: cluster.pts as i64,
                        timebase,
                    },
                );
                pkt.dts = Timestamp {
                    ticks: cluster.dts as i64,
                    timebase,
                };
                pkt.data = cluster.payload.to_vec();
                if (cluster.flags & CLUS_KEYFRAME) != 0 {
                    pkt.flags |= PacketFlags::KEYFRAME;
                }
                if cluster.flags & crate::format::CLUS_END_OF_FRAME != 0 {
                    pkt.flags |= PacketFlags::END_OF_FRAME;
                }
                if cluster.flags & crate::format::CLUS_DISCONTINUITY != 0 {
                    pkt.flags |= PacketFlags::DISCONTINUITY;
                }
                if let Some(d) = cluster.duration {
                    pkt.duration = Some(tpt_media_core::Duration {
                        ticks: d as i64,
                        timebase,
                    });
                }
                return Ok(pkt);
            }
        }
        Err(ContainerError::Truncated)
    }

    fn write_packet(&mut self, _packet: &Packet) -> ContainerResult<()> {
        Err(ContainerError::Unsupported("TptDemuxer is read-only"))
    }

    fn finalize(&mut self) -> ContainerResult<()> {
        Ok(())
    }

    fn seek_to_keyframe(&mut self, target_pts: i64) -> ContainerResult<()> {
        self.seek_to = Some(target_pts as u64);
        Ok(())
    }

    fn tracks(&self) -> &[Track] {
        &self.stream.tracks
    }
}
