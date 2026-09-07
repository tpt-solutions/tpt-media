//! Compressed-bitstream packet.

use alloc::vec::Vec;

use crate::timebase::Timestamp;

bitflags::bitflags! {
    /// Flags attached to every [`Packet`].
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    pub struct PacketFlags: u8 {
        /// Frame can be decoded without reference to any other frame.
        ///
        /// This is the I-frame / IDR equivalent: the packet's compressed
        /// data is self-contained, so the decoder can throw away its
        /// reference-picture buffer and start fresh from this packet.
        const KEYFRAME      = 0b0000_0001;
        /// Packet ends a logical access unit.
        ///
        /// For codecs where one *frame* of decoded output is split
        /// across multiple compressed packets (e.g., a video frame
        /// distributed across several slices or fields), the last
        /// packet in the access unit carries this flag. For codecs
        /// where each compressed packet is exactly one access unit
        /// (most audio codecs, single-slice intra video), this flag
        /// should be set on every packet by convention.
        const END_OF_FRAME  = 0b0000_0010;
        /// Stream discontinuity at this packet.
        ///
        /// Indicates that the timestamps, decoder state, or content
        /// sequence preceding this packet in the same track is **not
        /// meaningfully connected** to what follows. A consumer that
        /// is mid-playback should treat the next packet as the start
        /// of a new stream segment: drop in-flight reference frames,
        /// reset sample-rate-conversion state, flush any speculative
        /// buffers, and re-anchor the clock domain. Containers and
        /// muxer-level splitters set this when splicing two unrelated
        /// recordings together, when a live stream was paused/resumed
        /// without PTS continuity, or when the bitstream was recovered
        /// from corruption and the surrounding timestamps can no longer
        /// be trusted.
        ///
        /// Discontinuities are *intra-track*; cross-track sync uses the
        /// [`crate::timebase`] synchronization model, not this flag.
        const DISCONTINUITY = 0b0000_0100;
    }
}

/// A unit of compressed bitstream, ready to be fed to a decoder.
///
/// `Packet` is the smallest piece of data that crosses the
/// decoder boundary. It owns its compressed bytes (`Vec<u8>`) and
/// carries presentation and decode timestamps, flags, and a track
/// identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    /// Compressed bitstream bytes. May be empty for "empty" packets.
    pub data: Vec<u8>,
    /// Presentation timestamp.
    pub pts: Timestamp,
    /// Decode timestamp (may equal `pts` for simple codecs).
    pub dts: Timestamp,
    /// Packet flags (keyframe, EOS, etc.).
    pub flags: PacketFlags,
    /// Index of the track this packet belongs to.
    pub track_id: u8,
    /// Optional duration of the resulting frame.
    pub duration: Option<crate::timebase::Duration>,
}

impl Packet {
    /// Empty packet (no data, zero PTS, all flags clear).
    pub fn empty(track_id: u8, pts: Timestamp) -> Self {
        Packet {
            data: Vec::new(),
            pts,
            dts: pts,
            flags: PacketFlags::empty(),
            track_id,
            duration: None,
        }
    }

    /// Returns `true` if this packet is a keyframe.
    pub fn is_keyframe(&self) -> bool {
        self.flags.contains(PacketFlags::KEYFRAME)
    }

    /// Returns `true` if this packet marks a stream discontinuity.
    ///
    /// See [`PacketFlags::DISCONTINUITY`] for the semantics: a consumer
    /// should drop in-flight state and treat the next packet as the
    /// start of a new stream segment.
    pub fn is_discontinuity(&self) -> bool {
        self.flags.contains(PacketFlags::DISCONTINUITY)
    }

    /// Returns `true` if this packet is the last packet of a logical
    /// access unit (a "frame" in the decoded sense).
    pub fn is_end_of_frame(&self) -> bool {
        self.flags.contains(PacketFlags::END_OF_FRAME)
    }

    /// Length of the compressed data in bytes.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// True if this packet carries no compressed data.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timebase::Timebase;

    #[test]
    fn empty_packet_construction() {
        let p = Packet::empty(0, Timestamp::zero(Timebase::MILLISECONDS));
        assert_eq!(p.track_id, 0);
        assert!(p.is_empty());
        assert!(!p.is_keyframe());
    }

    #[test]
    fn keyframe_flag() {
        let mut p = Packet::empty(0, Timestamp::zero(Timebase::MILLISECONDS));
        p.flags |= PacketFlags::KEYFRAME;
        assert!(p.is_keyframe());
    }

    #[test]
    fn packet_with_data() {
        let mut p = Packet::empty(1, Timestamp::zero(Timebase::MILLISECONDS));
        p.data.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(p.len(), 4);
        assert!(!p.is_empty());
    }

    #[test]
    fn discontinuity_flag_accessor() {
        let mut p = Packet::empty(0, Timestamp::zero(Timebase::MILLISECONDS));
        assert!(!p.is_discontinuity());
        p.flags |= PacketFlags::DISCONTINUITY;
        assert!(p.is_discontinuity());
    }

    #[test]
    fn end_of_frame_flag_accessor() {
        let mut p = Packet::empty(0, Timestamp::zero(Timebase::MILLISECONDS));
        assert!(!p.is_end_of_frame());
        p.flags |= PacketFlags::END_OF_FRAME;
        assert!(p.is_end_of_frame());
    }

    #[test]
    fn packet_flags_are_independent() {
        // KEYFRAME, END_OF_FRAME and DISCONTINUITY are independent bits
        // and must coexist on a single packet (a keyframe that ends a
        // frame and marks a discontinuity is a perfectly reasonable
        // combination, e.g. a stream segment that begins with an
        // IDR after a splice).
        let mut p = Packet::empty(0, Timestamp::zero(Timebase::MILLISECONDS));
        p.flags |= PacketFlags::KEYFRAME | PacketFlags::END_OF_FRAME | PacketFlags::DISCONTINUITY;
        assert!(p.is_keyframe());
        assert!(p.is_end_of_frame());
        assert!(p.is_discontinuity());
    }
}
