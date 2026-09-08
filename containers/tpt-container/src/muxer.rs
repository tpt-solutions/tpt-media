//! `TptMuxer` — incremental, append-only writer for the `.tpt` container.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::mem;

use tpt_media_container::{Container, ContainerError, ContainerResult};
use tpt_media_core::{
    CodecConfig, CodecId, MediaType, Packet, PacketFlags, Stream, Timebase, Track,
};

use crate::crc32::crc32;
use crate::format::{
    encode_chunk, encode_clus, encode_idx, encode_trak, FileHeader, IndexEntry, CHUNK_CLUS,
    CHUNK_CRC1, CHUNK_IDX, CHUNK_TRAK, CLUS_DISCONTINUITY, CLUS_END_OF_FRAME, CLUS_KEYFRAME,
};

/// In-memory, append-only muxer for `.tpt` byte streams.
///
/// Chunks are appended as they are produced (no seek-back required), so the
/// muxer doubles as a live-stream / fragment writer. `finalize()` — or, via
/// the [`Container`] trait, `finalize` — appends the per-track `IDX ` chunk
/// and the container-level `CRC1` footer as a trailer, which is the graceful
/// termination of the stream.
#[derive(Default)]
pub struct TptMuxer {
    bytes: Vec<u8>,
    stream: Stream,
    index: BTreeMap<u8, Vec<IndexEntry>>,
    finalized: bool,
    cluster_checksums: bool,
}

impl TptMuxer {
    /// Begin a new container with the given global timebase.
    pub fn new(timebase: Timebase) -> Self {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&FileHeader::new(timebase).to_bytes());
        TptMuxer {
            bytes,
            stream: Stream::empty(timebase),
            index: BTreeMap::new(),
            finalized: false,
            cluster_checksums: false,
        }
    }

    /// Enable optional per-cluster CRC-32 fields (bits in the `CLUS` layout).
    pub fn enable_cluster_checksums(&mut self) {
        self.cluster_checksums = true;
    }

    /// Append an `CLUS` chunk for `payload`.
    pub fn add_track(&mut self, track_id: u8, codec_id: &[u8; 4], codec_private: &[u8]) {
        let mut track = Track::new(
            track_id,
            MediaType::Audio,
            CodecId::new(*codec_id),
            self.stream.global_timebase,
        );
        track.codec_config = CodecConfig {
            codec_id: CodecId::new(*codec_id),
            version: 1,
            data: codec_private.to_vec(),
        };
        self.register_track(&track);
    }

    /// Append a complete track descriptor (`TRAK`).
    ///
    /// The track's `id`, `media_type`, codec version, timebase and
    /// `TrackMedia` fields (sample rate, channel layout, dimensions, pixel
    /// format) are all serialized. A second `TRAK` for the same ID is ignored.
    pub fn register_track(&mut self, track: &Track) {
        if track.codec_config.data.len() > u16::MAX as usize {
            return;
        }
        if self.stream.track(track.id).is_some() {
            return;
        }
        let body = encode_trak(track);
        self.bytes
            .extend_from_slice(&encode_chunk(&CHUNK_TRAK, &body));
        if !track.metadata.is_empty() {
            let meta = crate::format::encode_tmta(track.id, &track.metadata);
            self.bytes
                .extend_from_slice(&encode_chunk(&crate::format::CHUNK_TMTA, &meta));
        }
        self.stream.tracks.push(track.clone());
    }

    /// Append an `CLUS` chunk (simple convenience form, no duration).
    pub fn add_cluster(&mut self, timestamp: u64, flags: u8, track_id: u8, payload: &[u8]) {
        let body = encode_clus(timestamp, timestamp, flags, track_id, None, false, payload);
        self.append_cluster(body, timestamp, track_id, (flags & CLUS_KEYFRAME) != 0);
    }

    /// Append a packet as a `CLUS` chunk (write path of [`Container`]).
    pub fn write_packet_raw(&mut self, packet: &Packet) -> ContainerResult<()> {
        if self.finalized {
            return Err(ContainerError::Unsupported("container already finalized"));
        }
        if self.stream.track(packet.track_id).is_none() {
            return Err(ContainerError::Unsupported("track not registered"));
        }
        let flags = packet_to_clus_flags(packet);
        let duration = packet
            .duration
            .map(|d| (d.ticks.clamp(0, u32::MAX as i64)) as u32);
        let body = encode_clus(
            packet.pts.ticks as u64,
            packet.dts.ticks as u64,
            flags,
            packet.track_id,
            duration,
            self.cluster_checksums,
            &packet.data,
        );
        self.append_cluster(
            body,
            packet.pts.ticks as u64,
            packet.track_id,
            packet.is_keyframe(),
        );
        Ok(())
    }

    fn append_cluster(&mut self, body: Vec<u8>, pts: u64, track_id: u8, keyframe: bool) {
        debug_assert!(!self.finalized, "cannot write after finalize");
        let offset = self.bytes.len() as u64;
        self.bytes
            .extend_from_slice(&encode_chunk(&CHUNK_CLUS, &body));
        self.index.entry(track_id).or_default().push(IndexEntry {
            pts,
            offset,
            keyframe,
        });
    }

    /// Append the trailer: per-track `IDX ` index plus the `CRC1` footer.
    ///
    /// This is the graceful termination of a `.tpt` stream. Files written
    /// without a trailer (e.g. a live stream that ends abruptly) remain
    /// readable: the demuxer falls back to its scan index.
    pub fn finalize(&mut self) -> ContainerResult<()> {
        if self.finalized {
            return Ok(());
        }
        let idx_body = encode_idx(crate::format::INDEX_VERSION, &self.index);
        self.bytes
            .extend_from_slice(&encode_chunk(&CHUNK_IDX, &idx_body));
        let crc = crc32(&self.bytes);
        let crc_body = crc.to_le_bytes();
        self.bytes
            .extend_from_slice(&encode_chunk(&CHUNK_CRC1, &crc_body));
        self.finalized = true;
        Ok(())
    }

    /// Produce the final byte buffer, writing the trailer first.
    pub fn finish(mut self) -> Vec<u8> {
        let _ = self.finalize();
        self.finalized = true;
        mem::take(&mut self.bytes)
    }

    /// Raw bytes written so far, without the trailer.
    ///
    /// This is what a live-stream writer would flush to its sink: the
    /// canonical fragments of a still-in-progress container.
    pub fn raw_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Append raw bytes to the stream buffer (live append / reconstruction).
    ///
    /// This lets external writers splice pre-serialized chunks into a
    /// stream; for most callers `register_track` / `add_cluster` are the
    /// canonical way to extend a stream. Writing after `finalize` is a panic.
    pub fn append_raw(&mut self, bytes: &[u8]) {
        debug_assert!(!self.finalized);
        self.bytes.extend_from_slice(bytes);
    }
}

/// Map packet flags to `CLUS` wire flags.
pub fn packet_to_clus_flags(packet: &Packet) -> u8 {
    (if packet.is_keyframe() {
        CLUS_KEYFRAME
    } else {
        0
    }) | (if packet.flags.contains(PacketFlags::END_OF_FRAME) {
        CLUS_END_OF_FRAME
    } else {
        0
    }) | (if packet.is_discontinuity() {
        CLUS_DISCONTINUITY
    } else {
        0
    })
}

impl Container for TptMuxer {
    fn stream(&self) -> &Stream {
        &self.stream
    }

    fn next_packet(&mut self) -> ContainerResult<Packet> {
        Err(ContainerError::Unsupported("TptMuxer is write-only"))
    }

    fn write_packet(&mut self, packet: &Packet) -> ContainerResult<()> {
        self.write_packet_raw(packet)
    }

    fn finalize(&mut self) -> ContainerResult<()> {
        self.finalize()
    }

    fn seek_to_keyframe(&mut self, _target_pts: i64) -> ContainerResult<()> {
        Err(ContainerError::Unsupported("TptMuxer is write-only"))
    }

    fn tracks(&self) -> &[Track] {
        &self.stream.tracks
    }
}
