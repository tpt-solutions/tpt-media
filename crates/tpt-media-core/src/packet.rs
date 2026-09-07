//! Compressed-bitstream packet.

use alloc::vec::Vec;

use crate::timebase::Timestamp;

bitflags::bitflags! {
    /// Flags attached to every [`Packet`].
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    pub struct PacketFlags: u8 {
        /// Frame can be decoded without reference to any other frame.
        const KEYFRAME      = 0b0000_0001;
        /// Packet ends a logical access unit.
        const END_OF_FRAME  = 0b0000_0010;
        /// Stream discontinuity at this packet.
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
}