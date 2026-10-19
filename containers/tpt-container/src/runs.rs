//! `PKTS` packet runs: several packets of one track in one chunk.
//!
//! A `CLUS` chunk carries one packet, and its framing costs about 43 bytes
//! per packet (the 8-byte chunk header, an 18-byte prefix that repeats the
//! timestamps, and a 17-byte index entry). Cinder emits one packet per 256
//! samples, so for audio that framing was most of the file. A `PKTS` run
//! carries up to [`RUN_MAX_PACKETS`] packets of one track, with timestamps
//! stored as deltas from the run's base, and costs about 9 bytes per packet.
//!
//! ```text
//! PKTS body
//!   track_id     u8
//!   flags        u8      bit 0: CRC-32 of the body up to this field present
//!   count        u16     number of records (1..=RUN_MAX_PACKETS)
//!   base_pts     u64     track timebase
//!   base_dts     u64     track timebase
//!   record × count:
//!     flags      u8      bit 0 keyframe, bit 1 end-of-frame,
//!                        bit 2 discontinuity, bit 3 duration present,
//!                        bit 4 dts present (dts differs from pts)
//!     pts_delta  i32     pts - base_pts
//!     dts_delta  i32     iff bit 4: dts - base_dts
//!     duration   u32     iff bit 3
//!     len        u32
//!     payload    u8[len]
//!   crc32        u32     iff flags bit 0; over every byte before it
//! ```
//!
//! The builder never emits a delta that does not fit in `i32`: a packet that
//! would need one starts a new run instead.

use alloc::vec::Vec;

use crate::crc32::crc32;
use crate::format::{CLUS_DISCONTINUITY, CLUS_END_OF_FRAME, CLUS_HAS_DURATION, CLUS_KEYFRAME};

/// Most records one `PKTS` run may carry.
pub const RUN_MAX_PACKETS: u16 = 32;

/// Most body bytes one `PKTS` run may hold, header included. A run that
/// would pass this is started afresh, unless it holds only one packet,
/// which may be larger. This bounds how much a streaming reader buffers.
pub const RUN_MAX_BYTES: usize = 64 * 1024;

/// `PKTS` header flag: a CRC-32 trailer follows the records.
pub const PKTS_HAS_CHECKSUM: u8 = 0x01;
/// Record flag: the dts is present and differs from the pts.
pub const REC_HAS_DTS: u8 = 0x10;

/// Fixed prefix of a `PKTS` body: track, flags, count, base pts, base dts.
pub const PKTS_HEADER: usize = 1 + 1 + 2 + 8 + 8;

/// Largest framing one record can carry: flags, pts and dts deltas,
/// duration and length. A record's payload is on top of this.
pub const RECORD_MAX_OVERHEAD: usize = 1 + 4 + 4 + 4 + 4;

/// One packet as stored in a run, with absolute timestamps.
#[derive(Clone, Copy, Debug)]
pub struct PacketRecord<'a> {
    /// Presentation timestamp, track timebase.
    pub pts: u64,
    /// Decode timestamp, track timebase (equal to `pts` when not stored).
    pub dts: u64,
    /// `CLUS_*` flags: keyframe, end-of-frame, discontinuity.
    pub flags: u8,
    /// Optional duration, track timebase.
    pub duration: Option<u32>,
    /// Compressed payload.
    pub payload: &'a [u8],
}

/// The header of a decoded `PKTS` body.
#[derive(Clone, Copy, Debug)]
pub struct RunHeader {
    /// Owning track ID.
    pub track_id: u8,
    /// Number of records in the run.
    pub count: u16,
    /// Base presentation timestamp.
    pub base_pts: u64,
    /// Base decode timestamp.
    pub base_dts: u64,
    /// `true` when there was no checksum or the embedded CRC-32 matched.
    pub crc_valid: bool,
}

/// Builds one `PKTS` run. Push records in order; check [`RunBuilder::accepts`]
/// first, and start a new run when it says no.
pub struct RunBuilder {
    track_id: u8,
    base_pts: u64,
    base_dts: u64,
    count: u16,
    keyframe: bool,
    records: Vec<u8>,
}

impl RunBuilder {
    /// Start a run whose first packet has the given timestamps.
    pub fn new(track_id: u8, pts: u64, dts: u64) -> Self {
        RunBuilder {
            track_id,
            base_pts: pts,
            base_dts: dts,
            count: 0,
            keyframe: false,
            records: Vec::new(),
        }
    }

    /// Whether a packet for `track_id`, with these timestamps and a payload
    /// of `payload_len` bytes, can join this run.
    pub fn accepts(&self, track_id: u8, pts: u64, dts: u64, payload_len: usize) -> bool {
        if track_id != self.track_id || self.count >= RUN_MAX_PACKETS {
            return false;
        }
        let worst_case = PKTS_HEADER + self.records.len() + RECORD_MAX_OVERHEAD + payload_len;
        if worst_case > RUN_MAX_BYTES {
            return false;
        }
        let fits = |delta: i64| i32::try_from(delta).is_ok();
        fits((pts as i64).wrapping_sub(self.base_pts as i64))
            && fits((dts as i64).wrapping_sub(self.base_dts as i64))
    }

    /// Number of records so far.
    pub fn len(&self) -> usize {
        self.count as usize
    }

    /// `true` when no record has been pushed.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Track this run belongs to.
    pub fn track_id(&self) -> u8 {
        self.track_id
    }

    /// Base presentation timestamp (the run's index key).
    pub fn base_pts(&self) -> u64 {
        self.base_pts
    }

    /// `true` if any record is a keyframe.
    pub fn has_keyframe(&self) -> bool {
        self.keyframe
    }

    /// Append one packet. The caller must have checked [`RunBuilder::accepts`].
    pub fn push(&mut self, flags: u8, pts: u64, dts: u64, duration: Option<u32>, payload: &[u8]) {
        debug_assert!(self.accepts(self.track_id, pts, dts, payload.len()));
        let has_dts = dts != pts;
        let mut rec_flags = flags & (CLUS_KEYFRAME | CLUS_END_OF_FRAME | CLUS_DISCONTINUITY);
        if duration.is_some() {
            rec_flags |= CLUS_HAS_DURATION;
        }
        if has_dts {
            rec_flags |= REC_HAS_DTS;
        }
        self.keyframe |= flags & CLUS_KEYFRAME != 0;
        self.records.push(rec_flags);
        let pts_delta = (pts as i64).wrapping_sub(self.base_pts as i64) as i32;
        self.records.extend_from_slice(&pts_delta.to_le_bytes());
        if has_dts {
            let dts_delta = (dts as i64).wrapping_sub(self.base_dts as i64) as i32;
            self.records.extend_from_slice(&dts_delta.to_le_bytes());
        }
        if let Some(d) = duration {
            self.records.extend_from_slice(&d.to_le_bytes());
        }
        self.records
            .extend_from_slice(&(payload.len() as u32).to_le_bytes());
        self.records.extend_from_slice(payload);
        self.count += 1;
    }

    /// The complete `PKTS` body.
    pub fn finish(self, checksum: bool) -> Vec<u8> {
        let mut out = Vec::with_capacity(PKTS_HEADER + self.records.len() + 4);
        out.push(self.track_id);
        out.push(if checksum { PKTS_HAS_CHECKSUM } else { 0 });
        out.extend_from_slice(&self.count.to_le_bytes());
        out.extend_from_slice(&self.base_pts.to_le_bytes());
        out.extend_from_slice(&self.base_dts.to_le_bytes());
        out.extend_from_slice(&self.records);
        if checksum {
            let crc = crc32(&out);
            out.extend_from_slice(&crc.to_le_bytes());
        }
        out
    }
}

/// Read one record at `pos`, returning it and the position after it.
fn read_record<'a>(
    body: &'a [u8],
    pos: usize,
    header: &RunHeader,
) -> Option<(PacketRecord<'a>, usize)> {
    let flags = *body.get(pos)?;
    let mut p = pos + 1;
    let pts_delta = i32::from_le_bytes(body.get(p..p + 4)?.try_into().ok()?);
    p += 4;
    let pts = (header.base_pts as i64).wrapping_add(pts_delta as i64) as u64;
    let dts = if flags & REC_HAS_DTS != 0 {
        let d = i32::from_le_bytes(body.get(p..p + 4)?.try_into().ok()?);
        p += 4;
        (header.base_dts as i64).wrapping_add(d as i64) as u64
    } else {
        pts
    };
    let duration = if flags & CLUS_HAS_DURATION != 0 {
        let d = u32::from_le_bytes(body.get(p..p + 4)?.try_into().ok()?);
        p += 4;
        Some(d)
    } else {
        None
    };
    let len = u32::from_le_bytes(body.get(p..p + 4)?.try_into().ok()?) as usize;
    p += 4;
    let end = p.checked_add(len)?;
    let payload = body.get(p..end)?;
    Some((
        PacketRecord {
            pts,
            dts,
            flags: flags & (CLUS_KEYFRAME | CLUS_END_OF_FRAME | CLUS_DISCONTINUITY),
            duration,
            payload,
        },
        end,
    ))
}

/// Where the records end: the body minus the CRC trailer, when present.
fn records_end(body: &[u8]) -> Option<usize> {
    let has_checksum = body.get(1)? & PKTS_HAS_CHECKSUM != 0;
    body.len().checked_sub(if has_checksum { 4 } else { 0 })
}

/// Decode a `PKTS` header and check every record, without keeping them.
///
/// Returns `None` for a malformed run: a short header, a record that runs
/// past the body, a record count that does not match the records, or a zero
/// or oversized count. A run that fails here is dropped whole.
pub fn decode_run(body: &[u8]) -> Option<RunHeader> {
    if body.len() < PKTS_HEADER {
        return None;
    }
    let flags = body[1];
    let count = u16::from_le_bytes([body[2], body[3]]);
    if count == 0 || count > RUN_MAX_PACKETS {
        return None;
    }
    let base_pts = u64::from_le_bytes(body[4..12].try_into().ok()?);
    let base_dts = u64::from_le_bytes(body[12..20].try_into().ok()?);
    let records_end = records_end(body)?;
    let crc_valid = flags & PKTS_HAS_CHECKSUM == 0
        || u32::from_le_bytes(body[records_end..].try_into().ok()?) == crc32(&body[..records_end]);
    let header = RunHeader {
        track_id: body[0],
        count,
        base_pts,
        base_dts,
        crc_valid,
    };
    let mut pos = PKTS_HEADER;
    for _ in 0..count {
        let (_, next) = read_record(&body[..records_end], pos, &header)?;
        pos = next;
    }
    if pos != records_end {
        return None;
    }
    Some(header)
}

/// Every record of a `PKTS` body, in order, or `None` if the run is malformed.
pub fn run_records(body: &[u8]) -> Option<(RunHeader, Vec<PacketRecord<'_>>)> {
    let header = decode_run(body)?;
    let end = records_end(body)?;
    let mut out = Vec::with_capacity(header.count as usize);
    let mut pos = PKTS_HEADER;
    for _ in 0..header.count {
        let (rec, next) = read_record(&body[..end], pos, &header)?;
        out.push(rec);
        pos = next;
    }
    Some((header, out))
}

/// The record at `slot` in a `PKTS` body.
pub fn run_record(body: &[u8], slot: usize) -> Option<PacketRecord<'_>> {
    run_records(body).and_then(|(_, records)| records.get(slot).copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_run(n: u32) -> Vec<u8> {
        let mut run = RunBuilder::new(0, 100, 100);
        for i in 0..n {
            run.push(
                CLUS_KEYFRAME,
                100 + i as u64,
                100 + i as u64,
                None,
                &[i as u8; 3],
            );
        }
        run.finish(false)
    }

    #[test]
    fn records_round_trip_with_timestamps_and_payloads() {
        let mut run = RunBuilder::new(2, 1_000, 990);
        run.push(CLUS_KEYFRAME, 1_000, 990, Some(20), b"abc");
        run.push(0, 1_020, 1_005, None, b"");
        run.push(
            CLUS_END_OF_FRAME | CLUS_DISCONTINUITY,
            1_010,
            1_010,
            None,
            b"zz",
        );
        let body = run.finish(true);
        let (header, records) = run_records(&body).expect("valid run");
        assert_eq!(header.track_id, 2);
        assert_eq!(header.count, 3);
        assert!(header.crc_valid);
        assert_eq!(records[0].pts, 1_000);
        assert_eq!(records[0].dts, 990);
        assert_eq!(records[0].duration, Some(20));
        assert_eq!(records[0].payload, b"abc");
        assert_eq!(records[1].dts, 1_005);
        assert_eq!(records[2].pts, 1_010);
        assert_eq!(records[2].dts, 1_010, "dts equal to pts is not stored");
        assert_eq!(records[2].flags, CLUS_END_OF_FRAME | CLUS_DISCONTINUITY);
    }

    #[test]
    fn audio_records_cost_nine_bytes_of_framing() {
        // 1 flag + 4 pts delta + 4 length; no dts, no duration.
        let body = audio_run(4);
        assert_eq!(body.len(), PKTS_HEADER + 4 * (9 + 3));
    }

    #[test]
    fn builder_refuses_what_it_cannot_encode() {
        let run = RunBuilder::new(0, 0, 0);
        assert!(run.accepts(0, 1, 1, 4));
        assert!(!run.accepts(1, 1, 1, 4), "other track");
        assert!(!run.accepts(0, 1 << 40, 0, 4), "pts delta beyond i32");
        assert!(
            !run.accepts(0, 1, 1, RUN_MAX_BYTES),
            "would exceed the byte bound"
        );
        let mut full = RunBuilder::new(0, 0, 0);
        for i in 0..RUN_MAX_PACKETS {
            full.push(0, i as u64, i as u64, None, b"");
        }
        assert!(!full.accepts(0, 99, 99, 0), "run is full");
    }

    #[test]
    fn corrupt_runs_are_refused_whole() {
        let body = audio_run(3);
        assert!(
            decode_run(&body[..body.len() - 1]).is_none(),
            "truncated record"
        );
        let mut bad_count = body.clone();
        bad_count[2] = 0;
        bad_count[3] = 0;
        assert!(decode_run(&bad_count).is_none(), "zero count");
        let mut extra = body.clone();
        extra.push(0);
        assert!(decode_run(&extra).is_none(), "trailing bytes");
    }

    #[test]
    fn checksum_detects_a_flipped_payload_byte() {
        let mut run = RunBuilder::new(0, 0, 0);
        run.push(0, 0, 0, None, b"payload");
        let mut body = run.finish(true);
        let last_payload = body.len() - 4 - 1;
        body[last_payload] ^= 0xFF;
        let header = decode_run(&body).expect("structure is intact");
        assert!(!header.crc_valid);
    }
}
