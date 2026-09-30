//! Incremental `.tpt` parser (todo.md section 45, "`.tpt` parser"; section
//! 25, "incremental parsing").
//!
//! [`tpt_container::TptDemuxer`] borrows one contiguous `&[u8]` for its whole
//! life. A browser hands the player bytes in `fetch()` chunks, and a
//! long-lived wasm instance cannot keep the entire file (or a
//! self-referential buffer + demuxer pair) alive. So this module is a small
//! *push* parser: it owns a bounded input buffer, is fed with
//! [`StreamParser::push`], and yields [`ParseEvent`]s from
//! [`StreamParser::poll`]. Consumed bytes are dropped, so memory is bounded
//! by [`WebLimits::max_input_buffer_bytes`] however long the stream is.
//!
//! It reuses the container crate's own decoders for every field
//! ([`FileHeader::from_bytes`], [`decode_trak`], [`decode_tmta`],
//! [`decode_clus`], [`read_chunk_header`]) so the wire format has exactly one
//! implementation; what is new here is only the framing state machine and
//! the limits.
//!
//! Hostile-input behaviour:
//!
//! * A chunk that *claims* a size above the limit for its kind is rejected
//!   from its 8-byte header alone, before a single body byte is buffered.
//! * `IDX `, `CRC1`, reserved and unknown chunks are *skipped without
//!   buffering*, so a multi-gigabyte unknown chunk costs no memory.
//! * A structurally bad `CLUS` (or one for an unknown track) is skipped and
//!   counted; a bad `TRAK`/`TMTA` is fatal, because a wrong track table
//!   would mis-decode everything after it.
//! * A `CLUS` whose embedded CRC-32 does not match is dropped and counted
//!   rather than decoded.

use crate::error::{ContainerFault, WebError};
use crate::limits::{WebLimits, CLUS_OVERHEAD, MAX_TRAK_BYTES};
use tpt_container::{
    decode_clus, decode_tmta, decode_trak, read_chunk_header, FileHeader, CHUNK_CLUS, CHUNK_TMTA,
    CHUNK_TRAK, CLUS_DISCONTINUITY, CLUS_END_OF_FRAME, CLUS_KEYFRAME, MAGIC,
};
use tpt_media_core::{Duration, Packet, PacketFlags, Timestamp, Track};

/// Minimum bytes needed before the header can be validated (the container
/// crate's `MIN_HEADER_SIZE`).
const MIN_HEADER: usize = 16;

/// One thing the parser found in the byte stream.
#[derive(Clone, Debug)]
pub enum ParseEvent {
    /// A track was declared (`TRAK`), or its metadata was replaced (`TMTA`).
    /// Consumers upsert by [`Track::id`].
    Track(Track),
    /// A compressed packet (`CLUS`) for a declared track, with its
    /// timestamps in that track's timebase.
    Packet(Packet),
}

/// Counters describing what the parser has consumed and refused.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ParserStats {
    /// Total bytes accepted by [`StreamParser::push`].
    pub bytes_in: u64,
    /// `CLUS` chunks delivered as packets.
    pub packets: u64,
    /// `CLUS` chunks dropped: malformed, unknown track, or bad CRC-32.
    pub dropped_clusters: u64,
    /// Of `dropped_clusters`, those dropped for a CRC-32 mismatch.
    pub crc_failures: u64,
    /// Chunks skipped without being parsed (`IDX `, `CRC1`, unknown).
    pub skipped_chunks: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum State {
    /// Waiting for / validating the file header.
    Header,
    /// Between chunks.
    Chunks,
    /// Discarding the body of a skipped chunk; this many bytes remain.
    Skip(u64),
    /// A fatal error was returned; no further progress.
    Failed,
}

/// Push-based `.tpt` parser with a bounded buffer. See the
/// [module docs](self).
#[derive(Debug)]
pub struct StreamParser {
    limits: WebLimits,
    buf: Vec<u8>,
    /// Offset of the first unconsumed byte in `buf`.
    start: usize,
    state: State,
    header: Option<FileHeader>,
    tracks: Vec<Track>,
    stats: ParserStats,
    error: Option<WebError>,
}

impl StreamParser {
    /// A parser enforcing `limits` (which are clamped, so the buffer can
    /// always hold one legal chunk).
    pub fn new(limits: WebLimits) -> Self {
        StreamParser {
            limits: limits.clamped(),
            buf: Vec::new(),
            start: 0,
            state: State::Header,
            header: None,
            tracks: Vec::new(),
            stats: ParserStats::default(),
            error: None,
        }
    }

    /// The parsed file header, once enough bytes have arrived.
    pub fn header(&self) -> Option<&FileHeader> {
        self.header.as_ref()
    }

    /// Tracks declared so far.
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Counters.
    pub fn stats(&self) -> ParserStats {
        self.stats
    }

    /// Bytes received but not yet consumed by parsing.
    pub fn buffered(&self) -> usize {
        self.buf.len() - self.start
    }

    /// Heap bytes currently reserved by the input buffer.
    pub fn capacity_bytes(&self) -> usize {
        self.buf.capacity()
    }

    /// `true` if the parser is mid-chunk (or mid-header): ending the input
    /// now would cut something off.
    pub fn has_partial_tail(&self) -> bool {
        match self.state {
            State::Header => self.buffered() > 0,
            State::Chunks => self.buffered() > 0,
            State::Skip(n) => n > 0,
            State::Failed => false,
        }
    }

    /// Append `bytes` to the input buffer.
    ///
    /// Returns [`WebError::InputFull`] (consuming nothing) if the bytes would
    /// push the *unparsed* backlog past `max_input_buffer_bytes`: poll
    /// events, then push again. Bytes that belong to a chunk being skipped
    /// are discarded immediately and never count against the buffer.
    pub fn push(&mut self, mut bytes: &[u8]) -> Result<(), WebError> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        // Fast path: stream bytes that only feed a skipped chunk straight
        // through, without buffering them.
        if let State::Skip(remaining) = self.state {
            if self.buffered() == 0 {
                let take = (bytes.len() as u64).min(remaining);
                self.stats.bytes_in += take;
                bytes = &bytes[take as usize..];
                self.state = if remaining == take {
                    State::Chunks
                } else {
                    State::Skip(remaining - take)
                };
            }
        }
        if bytes.is_empty() {
            return Ok(());
        }
        self.compact();
        let limit = self.limits.max_input_buffer_bytes as usize;
        if self.buffered().saturating_add(bytes.len()) > limit {
            return Err(WebError::InputFull);
        }
        // `try_reserve` so an allocation failure is an error, not an abort.
        if self.buf.try_reserve(bytes.len()).is_err() {
            return Err(WebError::Limit("input buffer allocation"));
        }
        self.buf.extend_from_slice(bytes);
        self.stats.bytes_in += bytes.len() as u64;
        Ok(())
    }

    /// Drop consumed bytes from the front of the buffer.
    fn compact(&mut self) {
        if self.start == 0 {
            return;
        }
        if self.start == self.buf.len() {
            self.buf.clear();
        } else {
            self.buf.drain(..self.start);
        }
        self.start = 0;
    }

    fn fail(&mut self, e: WebError) -> WebError {
        self.state = State::Failed;
        // Release the buffer: nothing more will be parsed.
        self.buf = Vec::new();
        self.start = 0;
        self.error = Some(e.clone());
        e
    }

    /// Produce the next event, `Ok(None)` if more input is needed, or a
    /// fatal error.
    pub fn poll(&mut self) -> Result<Option<ParseEvent>, WebError> {
        loop {
            if let Some(e) = &self.error {
                return Err(e.clone());
            }
            match self.state {
                State::Failed => {
                    return Err(self
                        .error
                        .clone()
                        .unwrap_or(WebError::Container(ContainerFault::BadChunk)))
                }
                State::Header => {
                    if !self.parse_header()? {
                        return Ok(None);
                    }
                }
                State::Skip(remaining) => {
                    let avail = self.buffered() as u64;
                    let take = avail.min(remaining);
                    self.start += take as usize;
                    if take == remaining {
                        self.state = State::Chunks;
                    } else {
                        self.state = State::Skip(remaining - take);
                        return Ok(None);
                    }
                }
                State::Chunks => match self.next_chunk()? {
                    Step::NeedInput => return Ok(None),
                    Step::Continue => {}
                    Step::Event(ev) => return Ok(Some(ev)),
                },
            }
        }
    }

    /// Returns `Ok(true)` once the header is consumed.
    fn parse_header(&mut self) -> Result<bool, WebError> {
        let avail = &self.buf[self.start..];
        // Fail fast on the wrong magic: do not wait for more of a file that
        // is not a .tpt at all.
        let check = avail.len().min(4);
        if avail[..check] != MAGIC[..check] {
            return Err(self.fail(WebError::Container(ContainerFault::BadHeader)));
        }
        if avail.len() < MIN_HEADER {
            return Ok(false);
        }
        let header = match FileHeader::from_bytes(avail) {
            Ok(h) => h,
            Err(e) => return Err(self.fail(e.into())),
        };
        let size = header.header_size as usize;
        if avail.len() < size {
            return Ok(false);
        }
        self.start += size;
        self.header = Some(header);
        self.state = State::Chunks;
        Ok(true)
    }

    fn next_chunk(&mut self) -> Result<Step, WebError> {
        let (id, size, body_start) = match read_chunk_header(&self.buf, self.start) {
            Some(x) => x,
            None => return Ok(Step::NeedInput),
        };
        let cap = if id == CHUNK_CLUS {
            self.limits.codec.max_packet_bytes as usize + CLUS_OVERHEAD
        } else if id == CHUNK_TMTA {
            self.limits.codec.max_metadata_bytes as usize
        } else if id == CHUNK_TRAK {
            MAX_TRAK_BYTES
        } else {
            // Not a chunk this parser interprets: skip it without buffering,
            // whatever size it claims.
            self.start = body_start;
            self.stats.skipped_chunks += 1;
            self.state = State::Skip(size as u64);
            return Ok(Step::Continue);
        };
        if size > cap {
            return Err(self.fail(WebError::Limit(if id == CHUNK_CLUS {
                "packet size"
            } else if id == CHUNK_TMTA {
                "metadata size"
            } else {
                "track descriptor size"
            })));
        }
        let body_end = body_start + size; // cannot overflow: size <= cap
        if body_end > self.buf.len() {
            return Ok(Step::NeedInput);
        }
        let step = self.handle_chunk(id, body_start, body_end);
        self.start = body_end;
        step
    }

    fn handle_chunk(
        &mut self,
        id: [u8; 4],
        body_start: usize,
        body_end: usize,
    ) -> Result<Step, WebError> {
        let header_tb = match &self.header {
            Some(h) => h.timebase,
            None => return Err(self.fail(WebError::Container(ContainerFault::BadHeader))),
        };
        // Borrow the body out of `self.buf` without holding `self` borrowed:
        // every branch below finishes with the slice before touching `self`.
        if id == CHUNK_TRAK {
            let decoded = decode_trak(&self.buf[body_start..body_end], header_tb);
            let track = match decoded {
                Some(t) => t,
                None => return Err(self.fail(WebError::Container(ContainerFault::BadChunk))),
            };
            if self.tracks.iter().any(|t| t.id == track.id) {
                // First declaration wins, as in `TptDemuxer`.
                return Ok(Step::Continue);
            }
            if self.tracks.len() >= self.limits.max_tracks as usize {
                return Err(self.fail(WebError::Limit("track count")));
            }
            self.tracks.push(track.clone());
            Ok(Step::Event(ParseEvent::Track(track)))
        } else if id == CHUNK_TMTA {
            let decoded = decode_tmta(&self.buf[body_start..body_end]);
            let (track_id, entries) = match decoded {
                Some(x) => x,
                None => return Err(self.fail(WebError::Container(ContainerFault::BadChunk))),
            };
            match self.tracks.iter_mut().find(|t| t.id == track_id) {
                Some(t) => {
                    t.metadata = entries;
                    Ok(Step::Event(ParseEvent::Track(t.clone())))
                }
                // Metadata for a track never declared: ignore, like the
                // demuxer does.
                None => Ok(Step::Continue),
            }
        } else {
            // CHUNK_CLUS
            let cluster = match decode_clus(&self.buf[body_start..body_end]) {
                Some(c) => c,
                None => {
                    self.stats.dropped_clusters += 1;
                    return Ok(Step::Continue);
                }
            };
            if !cluster.crc_valid {
                self.stats.dropped_clusters += 1;
                self.stats.crc_failures += 1;
                return Ok(Step::Continue);
            }
            let timebase = match self.tracks.iter().find(|t| t.id == cluster.track_id) {
                Some(t) => t.timebase,
                None => {
                    self.stats.dropped_clusters += 1;
                    return Ok(Step::Continue);
                }
            };
            let ts = |ticks: u64| Timestamp {
                ticks: i64::try_from(ticks).unwrap_or(i64::MAX),
                timebase,
            };
            let mut pkt = Packet::empty(cluster.track_id, ts(cluster.pts));
            pkt.dts = ts(cluster.dts);
            pkt.data = cluster.payload.to_vec();
            if cluster.flags & CLUS_KEYFRAME != 0 {
                pkt.flags |= PacketFlags::KEYFRAME;
            }
            if cluster.flags & CLUS_END_OF_FRAME != 0 {
                pkt.flags |= PacketFlags::END_OF_FRAME;
            }
            if cluster.flags & CLUS_DISCONTINUITY != 0 {
                pkt.flags |= PacketFlags::DISCONTINUITY;
            }
            if let Some(d) = cluster.duration {
                pkt.duration = Some(Duration {
                    ticks: i64::from(d),
                    timebase,
                });
            }
            self.stats.packets += 1;
            Ok(Step::Event(ParseEvent::Packet(pkt)))
        }
    }
}

enum Step {
    NeedInput,
    Continue,
    Event(ParseEvent),
}

#[cfg(all(test, feature = "synth"))]
mod tests {
    use super::*;
    use crate::synth;

    fn drain(p: &mut StreamParser) -> Vec<ParseEvent> {
        let mut out = Vec::new();
        while let Some(e) = p.poll().expect("no error") {
            out.push(e);
        }
        out
    }

    fn count(events: &[ParseEvent]) -> (usize, usize) {
        let tracks = events
            .iter()
            .filter(|e| matches!(e, ParseEvent::Track(_)))
            .count();
        (tracks, events.len() - tracks)
    }

    #[test]
    fn whole_file_and_byte_at_a_time_agree_with_the_container_demuxer() {
        let file = synth::audio_tpt(synth::AudioKind::Cinder, 48_000, 0.25);
        let (_h, _tracks, expected) = tpt_container::parse_tpt(&file).unwrap();

        let mut whole = StreamParser::new(WebLimits::default());
        whole.push(&file).unwrap();
        let ev_whole = drain(&mut whole);

        let mut drip = StreamParser::new(WebLimits::default());
        let mut ev_drip = Vec::new();
        for b in &file {
            drip.push(std::slice::from_ref(b)).unwrap();
            ev_drip.extend(drain(&mut drip));
        }

        let packets = |evs: &[ParseEvent]| -> Vec<Packet> {
            evs.iter()
                .filter_map(|e| match e {
                    ParseEvent::Packet(p) => Some(p.clone()),
                    _ => None,
                })
                .collect()
        };
        let a = packets(&ev_whole);
        let b = packets(&ev_drip);
        assert_eq!(a.len(), expected.len());
        assert_eq!(b.len(), expected.len());
        for ((x, y), z) in a.iter().zip(&b).zip(&expected) {
            assert_eq!(x.data, z.data);
            assert_eq!(y.data, z.data);
            assert_eq!(x.pts, z.pts);
            assert_eq!(y.pts, z.pts);
            assert_eq!(x.flags, z.flags);
        }
        assert_eq!(count(&ev_whole).0, count(&ev_drip).0);
        assert_eq!(whole.stats().packets as usize, expected.len());
    }

    #[test]
    fn consumed_bytes_are_released() {
        let file = synth::audio_tpt(synth::AudioKind::Cinder, 48_000, 1.0);
        let mut p = StreamParser::new(WebLimits::default());
        let mut peak = 0;
        for chunk in file.chunks(512) {
            p.push(chunk).unwrap();
            drain(&mut p);
            peak = peak.max(p.buffered());
        }
        // Never holds much more than one chunk plus one push.
        assert!(peak < 4096, "peak backlog {peak}");
        assert!(file.len() > 10 * peak);
    }

    #[test]
    fn wrong_magic_fails_immediately() {
        let mut p = StreamParser::new(WebLimits::default());
        p.push(b"RIFF").unwrap();
        assert!(matches!(
            p.poll(),
            Err(WebError::Container(ContainerFault::BadHeader))
        ));
        // Stays failed, and push reports the same error.
        assert!(p.push(b"x").is_err());
        assert!(p.poll().is_err());
    }

    fn header_bytes() -> Vec<u8> {
        tpt_container::FileHeader::new(tpt_media_core::Timebase::MILLISECONDS)
            .to_bytes()
            .to_vec()
    }

    #[test]
    fn oversized_claims_are_rejected_from_the_chunk_header_alone() {
        for id in [b"CLUS", b"TMTA", b"TRAK"] {
            let mut f = header_bytes();
            f.extend_from_slice(id);
            f.extend_from_slice(&u32::MAX.to_le_bytes());
            let mut p = StreamParser::new(WebLimits::default());
            p.push(&f).unwrap();
            assert!(
                matches!(p.poll(), Err(WebError::Limit(_))),
                "{}",
                String::from_utf8_lossy(id)
            );
            // The failed parser dropped its buffer.
            assert_eq!(p.capacity_bytes(), 0);
        }
    }

    #[test]
    fn unknown_giant_chunks_are_skipped_without_buffering() {
        let mut f = header_bytes();
        f.extend_from_slice(b"JUNK");
        let claimed: u32 = 200 * 1024 * 1024;
        f.extend_from_slice(&claimed.to_le_bytes());
        let mut p = StreamParser::new(WebLimits::default());
        p.push(&f).unwrap();
        assert!(p.poll().unwrap().is_none());
        // Feed 200 MiB of body in 1 MiB slices: none of it is retained.
        let slice = vec![0xAAu8; 1024 * 1024];
        for _ in 0..200 {
            p.push(&slice).unwrap();
            assert!(p.poll().unwrap().is_none());
            assert!(p.buffered() == 0);
        }
        assert!(p.capacity_bytes() < 1024);
        // A real chunk after the junk still parses.
        let tail = synth::audio_tpt(synth::AudioKind::Cinder, 48_000, 0.05);
        p.push(&tail[20..]).unwrap();
        let ev = drain(&mut p);
        assert!(count(&ev).1 > 0);
        assert!(p.stats().skipped_chunks >= 1);
    }

    #[test]
    fn input_full_is_retryable_and_consumes_nothing() {
        let limits = WebLimits {
            max_input_buffer_bytes: 1, // clamped up to the one-chunk floor
            ..WebLimits::default()
        };
        let cap = limits.clamped().max_input_buffer_bytes as usize;
        let mut p = StreamParser::new(limits);
        let junk = vec![b'T'; cap + 1]; // no header yet: stays buffered
        assert_eq!(p.push(&junk[..cap]).ok(), Some(()));
        assert_eq!(p.push(&junk[..1]), Err(WebError::InputFull));
        assert_eq!(p.buffered(), cap);
    }

    #[test]
    fn corrupted_cluster_crc_is_dropped_not_decoded() {
        use tpt_container::{encode_chunk, encode_clus, encode_trak};
        use tpt_media_core::{CodecId, MediaType, Timebase};
        let tb = Timebase::MILLISECONDS;
        let track = Track::new(0, MediaType::Audio, CodecId::new(*b"CIND"), tb);
        let mut f = header_bytes();
        f.extend(encode_chunk(b"TRAK", &encode_trak(&track)));
        let mut body = encode_clus(
            0,
            0,
            tpt_container::CLUS_KEYFRAME,
            0,
            None,
            true,
            &[1, 2, 3, 4],
        );
        let good = encode_chunk(b"CLUS", &body);
        let n = body.len();
        body[n - 6] ^= 0xFF; // flip a payload byte under the checksum
        let bad = encode_chunk(b"CLUS", &body);
        f.extend(&bad);
        f.extend(&good);
        let mut p = StreamParser::new(WebLimits::default());
        p.push(&f).unwrap();
        let ev = drain(&mut p);
        assert_eq!(count(&ev), (1, 1));
        assert_eq!(p.stats().crc_failures, 1);
        assert_eq!(p.stats().dropped_clusters, 1);
    }

    /// Deterministic xorshift so failures reproduce.
    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn random_and_mutated_input_never_panics_and_always_terminates() {
        let base = synth::audio_tpt(synth::AudioKind::Cinder, 48_000, 0.1);
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        for round in 0..400 {
            let mut data = base.clone();
            match round % 4 {
                0 => {
                    // flip a few random bytes
                    for _ in 0..1 + rng(&mut seed) % 8 {
                        let i = (rng(&mut seed) as usize) % data.len();
                        data[i] ^= (rng(&mut seed) & 0xFF) as u8 | 1;
                    }
                }
                1 => data.truncate((rng(&mut seed) as usize) % data.len()),
                2 => {
                    // pure noise after a valid header
                    for b in data.iter_mut().skip(20) {
                        *b = rng(&mut seed) as u8;
                    }
                }
                _ => {
                    // random chunk-size fields
                    let i = 20 + (rng(&mut seed) as usize) % (data.len() - 24);
                    data[i..i + 4].copy_from_slice(&(rng(&mut seed) as u32).to_le_bytes());
                }
            }
            let mut p = StreamParser::new(WebLimits::default());
            let step = 1 + (rng(&mut seed) as usize) % 700;
            'feed: for c in data.chunks(step) {
                if p.push(c).is_err() {
                    break;
                }
                loop {
                    match p.poll() {
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(_) => break 'feed,
                    }
                }
            }
            assert!(p.buffered() <= WebLimits::default().max_input_buffer_bytes as usize);
        }
    }
}
