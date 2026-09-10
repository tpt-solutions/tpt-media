//! `tpt-container` — the native `.tpt` container format.
//!
//! ## Binary layout (little-endian)
//!
//! ```text
//! FileHeader (20 bytes)
//!   magic        u8[4]    = "TPTM"
//!   version      u16      = 0x0001 (major = >> 8, minor = & 0xFF)
//!   header_size  u16      = 20
//!   timebase_num u32
//!   timebase_den u32
//!   reserved     u32      = 0 (ignored on read)
//!
//! Chunk (TLV)
//!   chunk_id     u8[4]
//!   chunk_size   u32
//!   body         u8[chunk_size]
//! ```
//!
//! Known chunk types:
//!
//! - `TRAK` — full track descriptor: track ID, media type, codec ID + version,
//!   codec-private data, timebase, sample rate, channel layout, dimensions,
//!   pixel format, optional name and language.
//! - `TMTA` — typed metadata key/value pairs for a track.
//! - `CLUS` — data cluster: `[pts:u64, dts:u64, flags:u8, track_id:u8,
//!   (duration:u32), payload, (crc:u32)]`. Flags: bit 0 = keyframe,
//!   bit 1 = end-of-frame, bit 2 = discontinuity, bit 3 = duration present,
//!   bit 4 = checksum present.
//! - `IDX ` — optional per-track index (written by `finalize`), with its own
//!   version byte.
//! - `CRC1` — optional container-level CRC-32 footer (written by `finalize`).
//!
//! See `docs/container-design.md` for the full specification.

extern crate alloc;

mod crc32;
mod demuxer;
mod format;
mod muxer;

pub use crc32::crc32;
pub use demuxer::{ChunkIndexEntry, TptDemuxer};
pub use format::{
    chunk_id_is_reserved, decode_clus, decode_idx, decode_tmta, decode_trak, encode_chunk,
    encode_clus, encode_idx, encode_tmta, encode_trak, read_chunk_header, DecodedCluster,
    FileHeader, IndexEntry, IndexFile, TptError, CHUNK_CLUS, CHUNK_CRC1, CHUNK_IDX, CHUNK_TMTA,
    CHUNK_TRAK, CLUS_DISCONTINUITY, CLUS_END_OF_FRAME, CLUS_HAS_CHECKSUM, CLUS_HAS_DURATION,
    CLUS_KEYFRAME, INDEX_VERSION, MAGIC, VERSION,
};
pub use muxer::{packet_to_clus_flags, TptMuxer};

use alloc::vec::Vec;
use tpt_media_container::Container;
use tpt_media_core::{Packet, Timebase, Track};

/// Convenience: build a complete `.tpt` byte buffer from tracks and packets.
///
/// The buffer is finalized (trailer appended) before it is returned.
pub fn build_tpt(
    timebase: Timebase,
    tracks: &[(u8, &[u8; 4], &[u8])],
    packets: &[Packet],
) -> Vec<u8> {
    let mut m = TptMuxer::new(timebase);
    for &(id, codec, priv_data) in tracks {
        m.add_track(id, codec, priv_data);
    }
    for p in packets {
        m.write_packet_raw(p).expect("registered track");
    }
    m.finish()
}

/// Parse a complete `.tpt` byte buffer into header, tracks and packets.
pub fn parse_tpt(bytes: &[u8]) -> Result<(FileHeader, Vec<Track>, Vec<Packet>), TptError> {
    let mut d = TptDemuxer::new(bytes)?;
    let header = *d.header();
    let tracks = d.stream().tracks.clone();
    let mut packets = Vec::new();
    while let Ok(p) = d.next_packet() {
        packets.push(p);
    }
    Ok((header, tracks, packets))
}

/// Iterate over raw chunks.
///
/// The callback is invoked for every chunk — known, unknown and reserved —
/// giving the chunk ID and its body. Files with a truncated trailing chunk
/// yield a `Truncated` error.
pub fn for_each_chunk(bytes: &[u8], mut cb: impl FnMut(&[u8; 4], &[u8])) -> Result<(), TptError> {
    let header = FileHeader::from_bytes(bytes)?;
    let mut pos = header.header_size as usize;
    while pos + 8 <= bytes.len() {
        let (id, size, body_start) = read_chunk_header(bytes, pos).ok_or(TptError::BadChunk)?;
        let body_end = body_start.checked_add(size).ok_or(TptError::BadChunk)?;
        if body_end > bytes.len() {
            return Err(TptError::Truncated);
        }
        cb(&id, &bytes[body_start..body_end]);
        pos = body_end;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use tpt_media_core::{
        ChannelLayout, CodecConfig, CodecId, MediaType, MetadataValue, PacketFlags, PixelFormat,
        Timestamp,
    };

    fn packet(track_id: u8, pts: i64, data: &[u8], flags: PacketFlags) -> Packet {
        let mut p = Packet::empty(
            track_id,
            Timestamp {
                ticks: pts,
                timebase: Timebase::MILLISECONDS,
            },
        );
        p.data = data.to_vec();
        p.flags = flags;
        p
    }

    // ---- §20.1 file header -------------------------------------------------

    #[test]
    fn header_round_trip() {
        let h = FileHeader::new(Timebase::MILLISECONDS);
        let b = h.to_bytes();
        let h2 = FileHeader::from_bytes(&b).unwrap();
        assert_eq!(h, h2);
        assert_eq!(h.header_size, 20);
        assert_eq!(h.reserved, 0);
    }

    #[test]
    fn header_rejects_bad_magic() {
        let mut b = [0u8; 20];
        b[0..4].copy_from_slice(b"XXXX");
        assert!(matches!(
            FileHeader::from_bytes(&b),
            Err(TptError::BadHeader)
        ));
    }

    #[test]
    fn header_version_checking_rejects_unsupported() {
        let h = FileHeader::new(Timebase::MILLISECONDS);
        let mut b = h.to_bytes();
        let cur = u16::from_le_bytes([b[4], b[5]]);
        assert!(
            cur <= VERSION,
            "current version is {} (need <= {})",
            cur,
            VERSION
        );
        // Zero version is invalid.
        b[4] = 0;
        b[5] = 0;
        assert!(matches!(
            FileHeader::from_bytes(&b),
            Err(TptError::UnsupportedVersion)
        ));
        // Future major version in the high byte.
        b[4] = 0x01;
        b[5] = 0x02;
        assert!(matches!(
            FileHeader::from_bytes(&b),
            Err(TptError::UnsupportedVersion)
        ));
        // Future minor within major 0.
        b[4] = 0x02;
        b[5] = 0x00;
        assert!(matches!(
            FileHeader::from_bytes(&b),
            Err(TptError::UnsupportedVersion)
        ));
    }

    #[test]
    fn header_compat_accepts_16_byte_v01_and_reserved_bytes() {
        // A version 0.1 file shipped a 16-byte header: no reserved field.
        let mut legacy = [0u8; 16];
        legacy[0..4].copy_from_slice(&MAGIC);
        legacy[4] = 1; // version minor 1, major 0
        legacy[6] = 16; // header_size
        legacy[8..12].copy_from_slice(&1000u32.to_le_bytes());
        legacy[12..16].copy_from_slice(&1u32.to_le_bytes());
        let h = FileHeader::from_bytes(&legacy).unwrap();
        assert_eq!(h.header_size, 16);
        assert_eq!(h.reserved, 0);
        // Nonzero reserved bytes are ignored (forward compatibility).
        let mut fwd = h.to_bytes();
        fwd[16..20].copy_from_slice(&0xCAFE_BABEu32.to_le_bytes());
        let h2 = FileHeader::from_bytes(&fwd).unwrap();
        assert_eq!(h2.reserved, 0xCAFE_BABE);
        assert!(h2.version <= VERSION);
    }

    // ---- §21 track structure -----------------------------------------------

    #[test]
    fn trak_round_trips_full_media_metadata() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        let mut track = Track::new(
            0,
            MediaType::Video,
            CodecId::new(*b"LIVE"),
            Timebase::new(90, 1),
        );
        track.codec_config = CodecConfig {
            codec_id: CodecId::new(*b"LIVE"),
            version: 0x00_03,
            data: vec![0xAA, 0xBB],
        };
        track.media.sample_rate = Some(48_000);
        track.media.channel_layout = Some(ChannelLayout::Stereo);
        track.media.width = Some(1920);
        track.media.height = Some(1080);
        track.media.pixel_format = Some(PixelFormat::Yuv420);
        track.name = Some("main".into());
        track.language = Some("en".into());
        track.metadata = vec![
            ("encoder".to_string(), MetadataValue::Text("tpt v1".into())),
            ("bitrate".to_string(), MetadataValue::Int(128_000)),
            ("lossless".to_string(), MetadataValue::Bool(true)),
        ];
        m.register_track(&track);
        let bytes = m.finish();

        let (_h, tracks, _packets) = parse_tpt(&bytes).unwrap();
        assert_eq!(tracks.len(), 1);
        let t = &tracks[0];
        assert_eq!(t.media_type, MediaType::Video);
        assert_eq!(t.codec_config.codec_id, CodecId::new(*b"LIVE"));
        assert_eq!(t.codec_config.version, 0x00_03);
        assert_eq!(t.codec_config.data, vec![0xAA, 0xBB]);
        assert_eq!(t.timebase, Timebase::new(90, 1));
        assert_eq!(t.media.sample_rate, Some(48_000));
        assert_eq!(t.media.channel_layout, Some(ChannelLayout::Stereo));
        assert_eq!(t.media.width, Some(1920));
        assert_eq!(t.media.height, Some(1080));
        assert_eq!(t.media.pixel_format, Some(PixelFormat::Yuv420));
        assert_eq!(t.name.as_deref(), Some("main"));
        assert_eq!(t.language.as_deref(), Some("en"));
        assert_eq!(t.metadata.len(), 3);
    }

    #[test]
    fn trak_defaults_when_fields_absent() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.add_track(0, b"PULS", &[1, 2, 3]);
        let bytes = m.finish();
        let (_h, tracks, _p) = parse_tpt(&bytes).unwrap();
        let t = &tracks[0];
        assert_eq!(t.media_type, MediaType::Audio);
        assert_eq!(t.codec_config.data, vec![1, 2, 3]);
        assert_eq!(t.media.sample_rate, None);
        assert_eq!(t.media.channel_layout, None);
        assert_eq!(t.name, None);
        assert_eq!(t.language, None);
    }

    // ---- §22 cluster structure ---------------------------------------------

    #[test]
    fn encode_decode_clus_round_trip() {
        let body = encode_clus(
            12345,
            12300,
            CLUS_KEYFRAME | CLUS_DISCONTINUITY,
            2,
            Some(43),
            false,
            &[10, 20, 30, 40],
        );
        let c = decode_clus(&body).unwrap();
        assert_eq!(c.pts, 12345);
        assert_eq!(c.dts, 12300);
        assert_eq!(c.flags & CLUS_KEYFRAME, CLUS_KEYFRAME);
        assert_eq!(c.flags & CLUS_DISCONTINUITY, CLUS_DISCONTINUITY);
        assert_eq!(c.track_id, 2);
        assert_eq!(c.duration, Some(43));
        assert_eq!(c.payload, &[10, 20, 30, 40]);
        assert!(c.crc_valid);
    }

    #[test]
    fn clus_pts_dts_duration_round_trip_through_container() {
        let tracks: [(u8, &[u8; 4], &[u8]); 1] = [(0, b"PULS", &[])];
        let mut p0 = packet(0, 1000, &[1, 2], PacketFlags::KEYFRAME);
        p0.dts = Timestamp {
            ticks: 990,
            timebase: Timebase::MILLISECONDS,
        };
        p0.duration = Some(tpt_media_core::Duration {
            ticks: 40,
            timebase: Timebase::MILLISECONDS,
        });
        let bytes = build_tpt(Timebase::MILLISECONDS, &tracks, &[p0]);
        let (_h, _t, decoded) = parse_tpt(&bytes).unwrap();
        assert_eq!(decoded[0].pts.ticks, 1000);
        assert_eq!(decoded[0].dts.ticks, 990);
        assert_eq!(decoded[0].duration.map(|d| d.ticks), Some(40));
    }

    #[test]
    fn clus_optional_checksum_round_trip_and_signals_mismatch() {
        let body = encode_clus(1, 1, CLUS_KEYFRAME, 0, None, true, b"payload");
        let good = decode_clus(&body).unwrap();
        assert!(good.crc_valid);
        assert_eq!(good.payload, b"payload");
        // Flipping a payload bit must be caught by the embedded checksum.
        let mut corrupt = body;
        corrupt[20] ^= 0x01;
        let bad = decode_clus(&corrupt).unwrap();
        assert!(!bad.crc_valid);
    }

    #[test]
    fn discontinuity_flag_round_trips_through_container() {
        let tracks: [(u8, &[u8; 4], &[u8]); 1] = [(0, b"PULS", &[])];
        let packets = vec![
            packet(
                0,
                0,
                &[0xAB],
                PacketFlags::DISCONTINUITY | PacketFlags::KEYFRAME,
            ),
            packet(0, 100, &[0xCD], PacketFlags::END_OF_FRAME),
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

    // ---- §23 container index ------------------------------------------------

    fn two_track_file() -> Vec<u8> {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.register_track(&Track::new(
            0,
            MediaType::Audio,
            CodecId::new(*b"PULS"),
            Timebase::MILLISECONDS,
        ));
        m.register_track(&Track::new(
            1,
            MediaType::Video,
            CodecId::new(*b"LIVE"),
            Timebase::MILLISECONDS,
        ));
        m.add_cluster(10, 0x00, 1, &[1]); // video non-key
        m.add_cluster(20, 0x01, 0, &[2]); // audio key
        m.add_cluster(30, 0x01, 1, &[3]); // video key
        m.add_cluster(40, 0x00, 1, &[4]); // video non-key
        m.add_cluster(50, 0x01, 0, &[5]); // audio key
        m.finish()
    }

    #[test]
    fn per_track_indexes_and_offset_lookup() {
        let bytes = two_track_file();
        let mut d = TptDemuxer::new(&bytes).unwrap();
        d.rebuild_index();
        let video = d.track_index(1).unwrap();
        let pts: Vec<u64> = video.iter().map(|e| e.pts).collect();
        assert_eq!(pts, vec![10, 30, 40]);
        // Offset-based lookup: moving the cursor to 0-services the clusters
        // at or after that byte offset.
        let target = video[1].offset;
        d.seek_to_offset(target).unwrap();
        let p = d.next_packet().unwrap();
        assert_eq!(p.track_id, 1);
        assert_eq!(p.pts.ticks, 30);
        assert!(p.is_keyframe());
    }

    #[test]
    fn per_track_keyframe_seek() {
        let bytes = two_track_file();
        let mut d = TptDemuxer::new(&bytes).unwrap();
        d.seek_to_keyframe_on_track(1, 25).unwrap();
        let p = d.next_packet().unwrap();
        assert_eq!(p.track_id, 1);
        assert_eq!(p.pts.ticks, 30);
        let mut d = TptDemuxer::new(&bytes).unwrap();
        d.seek_to_keyframe_on_track(0, 60).unwrap_err();
    }

    #[test]
    fn index_chunk_versioning_and_recovery() {
        let bytes = two_track_file();
        // The on-disk index from a finalized file is parsed.
        let d = TptDemuxer::new(&bytes).unwrap();
        let disk = d.disk_index().unwrap();
        assert_eq!(disk.version, INDEX_VERSION);
        assert_eq!(disk.tracks.get(&1).unwrap().len(), 3);
        // A future index version is ignored; the scan index (recovery)
        // still serves packets.
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.add_track(0, b"PULS", &[]);
        m.add_cluster(5, 0x01, 0, &[1]);
        let mut bytes = m.raw_bytes().to_vec();
        let mut tracks: BTreeMap<u8, Vec<IndexEntry>> = BTreeMap::new();
        tracks.insert(
            0,
            vec![IndexEntry {
                pts: 5,
                offset: 25,
                keyframe: true,
            }],
        );
        bytes.extend_from_slice(&encode_chunk(&CHUNK_IDX, &encode_idx(99, &tracks)));
        bytes.extend_from_slice(&encode_chunk(b"CRC1", &[0, 0, 0, 0]));
        let d = TptDemuxer::new(&bytes).unwrap();
        assert!(d.disk_index().is_none());
        assert_eq!(d.stream().tracks.len(), 1);
    }

    // ---- §24 robustness -----------------------------------------------------

    #[test]
    fn incomplete_final_cluster_is_an_error_in_strict_mode() {
        let bytes = two_track_file();
        let trunc = &bytes[..bytes.len() - 3];
        assert!(matches!(TptDemuxer::new(trunc), Err(TptError::Truncated)));
    }

    #[test]
    fn forward_compat_unknown_and_reserved_chunk_types_are_skipped() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.add_track(0, b"PULS", &[]);
        m.append_raw(&encode_chunk(b"XYZ!", &[1, 2, 3, 4]));
        m.append_raw(&encode_chunk(&[0x05, 0x00, 0x00, 0x00], &[9, 9]));
        m.add_cluster(10, 0x01, 0, &[0xAA]);
        let bytes = m.finish();
        let (_h, _t, packets) = parse_tpt(&bytes).unwrap();
        assert_eq!(packets.len(), 1);
    }

    #[test]
    fn container_checksum_verifies_and_missing_footer_is_none() {
        // Finalized file: footer present and matching.
        let finalized = two_track_file();
        let d = TptDemuxer::new(&finalized).unwrap();
        assert_eq!(d.verify_checksum(), Some(true));
        // Live buffer without a trailer: no footer to verify.
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.add_track(0, b"PULS", &[]);
        m.add_cluster(0, 0x01, 0, &[1]);
        let raw = m.raw_bytes().to_vec();
        let d = TptDemuxer::new_streaming(&raw).unwrap();
        assert_eq!(d.verify_checksum(), None);
    }

    #[test]
    fn cluster_checksums_verify_when_enabled() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.enable_cluster_checksums();
        m.add_track(0, b"PULS", &[]);
        m.add_cluster(10, 0x01, 0, &[1, 2, 3]);
        let bytes = m.finish();
        let d = TptDemuxer::new(&bytes).unwrap();
        assert!(d.verify_cluster_checksums().is_ok());
    }

    // ---- §25 streaming ------------------------------------------------------

    #[test]
    fn fragmented_writing_then_incremental_parse() {
        let full = two_track_file();
        // Feed in 8-byte slices; the parser tolerates a partial trailing
        // chunk and never errors until the last byte has arrived.
        assert!(full.len() > 24);
        let mut d = TptDemuxer::new_streaming(&full[..16]).unwrap();
        for end in (24..=full.len()).step_by(8) {
            d.feed(&full[..end]).unwrap();
        }
        let mut packets = Vec::new();
        while let Ok(p) = d.next_packet() {
            packets.push(p);
        }
        assert_eq!(packets.len(), 5);
        assert_eq!(packets[0].track_id, 1);
        assert_eq!(packets[0].pts.ticks, 10);
    }

    #[test]
    fn live_append_resumes_mid_chunk() {
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.add_track(0, b"PULS", &[]);
        m.add_cluster(10, CLUS_KEYFRAME, 0, &[1]);
        m.add_cluster(20, 0x00, 0, &[2]);
        let mut stream = m.raw_bytes().to_vec(); // live buffer: no trailer yet

        // A strict parse of the incomplete live buffer fails:
        let incomplete = stream[..stream.len() - 3].to_vec();
        assert!(TptDemuxer::new(&incomplete).is_err());

        // ...but a streaming parse pauses without error, then resumes when
        // the trailer is appended ("live append / footer reconstruction").
        let mut d = TptDemuxer::new_streaming(&incomplete).unwrap();
        let tail = {
            let dd = TptDemuxer::new_streaming(&stream).unwrap();
            dd.reconstruct_trailer()
        };
        stream.extend_from_slice(&tail);
        d.feed(&stream).unwrap();

        let mut got = Vec::new();
        while let Ok(p) = d.next_packet() {
            got.push(p.pts.ticks);
        }
        assert_eq!(got, vec![10, 20]);
        // The reconstructed footer verifies over the extended buffer.
        let final_d = TptDemuxer::new(&stream).unwrap();
        assert_eq!(final_d.verify_checksum(), Some(true));
    }

    #[test]
    fn missing_final_index_streams_gracefully() {
        // A stream that never gets a trailer (no IDX, no CRC1) still
        // reproduces every packet; EOF is signalled via next_packet
        // returning Truncated after the last cluster.
        let mut m = TptMuxer::new(Timebase::MILLISECONDS);
        m.add_track(0, b"PULS", &[]);
        m.add_cluster(0, CLUS_KEYFRAME, 0, &[1]);
        m.add_cluster(5, 0x00, 0, &[2]);
        m.add_cluster(9, CLUS_KEYFRAME, 0, &[3]);
        let raw = m.raw_bytes().to_vec();
        let d = TptDemuxer::new_streaming(&raw).unwrap();
        assert!(d.disk_index().is_none());
        assert_eq!(d.verify_checksum(), None);
        let mut d = d;
        let mut p = Vec::new();
        while let Ok(x) = d.next_packet() {
            p.push(x.pts.ticks);
        }
        assert_eq!(p, vec![0, 5, 9]);
    }

    #[test]
    fn truncated_header_streaming_tolerated() {
        // A 16-byte header with a 20-byte size: streaming waits for the rest.
        let mut h = FileHeader::new(Timebase::MILLISECONDS).to_bytes().to_vec();
        h.truncate(16);
        let d = TptDemuxer::new_streaming(&h).unwrap();
        let _ = d;
        assert!(TptDemuxer::new(&h).is_err());
        // Once completed, it parses.
        let full = FileHeader::new(Timebase::MILLISECONDS).to_bytes();
        assert!(TptDemuxer::new(&full).is_ok());
    }

    // ---- fuzz-ish determinism -----------------------------------------------

    #[test]
    fn corruption_never_panics_and_truncation_never_panics() {
        let valid = two_track_file();
        assert!(valid.len() > 64, "fixture too small: {}", valid.len());
        // Truncation sweep: every prefix length must either parse cleanly or
        // return a TptError, never panic, never misbehave.
        for cut in 0..valid.len() {
            let bytes = &valid[..cut];
            let _ = TptDemuxer::new(bytes);
            let _ = TptDemuxer::new_streaming(bytes);
            let _ = FileHeader::from_bytes(bytes);
        }
        // Deterministic LCG (no external deps) over every byte position:
        // flip one bit; parsing must stay panic-free.
        let mut state: u64 = 0x243F_6A88_85A3_08D3;
        for _ in 0..512 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let pos = (state as usize) % valid.len();
            let bit = 1u8 << ((state >> 32) % 8);
            let mut corrupt = valid.clone();
            corrupt[pos] ^= bit;
            let _ = TptDemuxer::new(&corrupt);
            let _ = TptDemuxer::new_streaming(&corrupt);
        }
        // The pristine file still parses after the sweep.
        assert_eq!(parse_tpt(&valid).unwrap().2.len(), 5);
    }

    #[test]
    fn for_each_chunk_visits_trailer_chunks() {
        let bytes = two_track_file();
        let mut ids: Vec<[u8; 4]> = Vec::new();
        for_each_chunk(&bytes, |id, _| ids.push(*id)).unwrap();
        assert!(ids.iter().any(|i| i == &[0x49, 0x44, 0x58, 0x20])); // IDX
        assert!(ids.iter().any(|i| i == &[0x43, 0x52, 0x43, 0x31])); // CRC1
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
            packet(0, 0, &[1, 2, 3], PacketFlags::KEYFRAME),
            packet(0, 100, &[4, 5, 6], PacketFlags::empty()),
        ];
        let bytes = build_tpt(Timebase::MILLISECONDS, &tracks, &packets);
        let (_h, _t, decoded) = parse_tpt(&bytes).unwrap();
        assert_eq!(decoded.len(), 2);
    }
}
