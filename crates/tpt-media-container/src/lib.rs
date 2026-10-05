//! `tpt-media-container` — generic container abstraction.
//!
//! The [`Container`] trait abstracts over the wire format of a media
//! container so that codecs do not need to know whether they are
//! packaged inside `.tpt`, `.mp4`, or some future TPT-defined format.
//! The native implementation lives in the `tpt-container` crate.

#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::vec::Vec;

use tpt_media_core::{Packet, Stream, Track};

/// Container-format-specific error type.
#[derive(Debug)]
pub enum ContainerError {
    /// The input ended unexpectedly.
    Truncated,
    /// The container header is invalid or unsupported.
    InvalidHeader,
    /// A chunk was malformed.
    MalformedChunk,
    /// An unsupported feature was requested.
    Unsupported(&'static str),
    /// I/O error.
    Io(&'static str),
}

/// Result alias.
pub type ContainerResult<T> = Result<T, ContainerError>;

/// A chunk in a generic container.
///
/// The four-byte ID identifies the chunk type (e.g. "TRAK", "CLUS").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// Four-byte ASCII chunk ID.
    pub id: [u8; 4],
    /// Chunk body bytes.
    pub body: Vec<u8>,
}

/// Generic container reader/writer.
pub trait Container {
    /// Stream-level information for this container.
    fn stream(&self) -> &Stream;
    /// Read the next packet from the container.
    fn next_packet(&mut self) -> ContainerResult<Packet>;
    /// Write a packet to the container.
    fn write_packet(&mut self, packet: &Packet) -> ContainerResult<()>;
    /// Finalize the container (e.g. write index, trailer).
    fn finalize(&mut self) -> ContainerResult<()>;
    /// Skip to the next keyframe at or after `target_pts` ticks.
    fn seek_to_keyframe(&mut self, target_pts: i64) -> ContainerResult<()>;
    /// Iterate over all tracks.
    fn tracks(&self) -> &[Track];
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{format, string::String, vec, vec::Vec};
    use tpt_media_core::{PacketFlags, Timebase, Timestamp};

    /// A conforming `Container`, so the trait's own contract is exercised
    /// rather than merely declared. The point of the abstraction is that a
    /// second format can be plugged in without touching the codecs, and that
    /// claim is only real if an implementor can satisfy it.
    #[derive(Default)]
    struct Fake {
        stream: Stream,
        incoming: Vec<Packet>,
        read: usize,
        written: Vec<Packet>,
        finalized: bool,
    }

    impl Fake {
        fn with_packets(packets: Vec<Packet>) -> Self {
            Self {
                incoming: packets,
                ..Self::default()
            }
        }
    }

    impl Container for Fake {
        fn stream(&self) -> &Stream {
            &self.stream
        }
        fn next_packet(&mut self) -> ContainerResult<Packet> {
            // End of stream is `Truncated`, matching the documented contract
            // that a short read is an error and never a panic.
            let p = self
                .incoming
                .get(self.read)
                .cloned()
                .ok_or(ContainerError::Truncated)?;
            self.read += 1;
            Ok(p)
        }
        fn write_packet(&mut self, packet: &Packet) -> ContainerResult<()> {
            self.written.push(packet.clone());
            Ok(())
        }
        fn finalize(&mut self) -> ContainerResult<()> {
            self.finalized = true;
            Ok(())
        }
        fn seek_to_keyframe(&mut self, _target_pts: i64) -> ContainerResult<()> {
            Ok(())
        }
        fn tracks(&self) -> &[Track] {
            &self.stream.tracks
        }
    }

    fn packet(data: &[u8]) -> Packet {
        Packet {
            data: data.to_vec(),
            pts: Timestamp::zero(Timebase::ONE_PER_SECOND),
            dts: Timestamp::zero(Timebase::ONE_PER_SECOND),
            flags: PacketFlags::empty(),
            track_id: 0,
            duration: None,
        }
    }

    // --- Chunk: the derives and the empty body are the public surface. ----

    #[test]
    fn chunk_construction() {
        let c = Chunk {
            id: *b"CLUS",
            body: vec![1, 2, 3],
        };
        assert_eq!(c.id, *b"CLUS");
        assert_eq!(c.body.len(), 3);
    }

    #[test]
    fn chunk_with_empty_body_is_valid() {
        // §38 "empty input": a zero-length body is a legitimate chunk, not a
        // malformed one, and must not be confused with a missing body.
        let c = Chunk {
            id: *b"IDX ",
            body: Vec::new(),
        };
        assert!(c.body.is_empty());
        assert_eq!(c.id, *b"IDX ");
    }

    #[test]
    fn chunk_id_is_always_four_bytes() {
        // The type system enforces the width; this pins that the invariant is
        // real rather than assumed, including for a non-ASCII-looking ID.
        let c = Chunk {
            id: [0x00, 0x1F, 0xFF, 0x80],
            body: vec![],
        };
        assert_eq!(c.id.len(), 4);
    }

    #[test]
    fn chunk_clone_is_a_deep_copy() {
        // Cloning must not alias the body: callers hand `Chunk` to code that
        // will modify it, and a shared buffer would corrupt the original.
        let a = Chunk {
            id: *b"TRAK",
            body: vec![7, 8],
        };
        let mut b = a.clone();
        assert_eq!(a, b);
        b.body.push(9);
        assert_eq!(a.body, vec![7, 8]);
        assert_ne!(a, b);
    }

    #[test]
    fn chunk_eq_compares_id_and_body() {
        let base = Chunk {
            id: *b"CLUS",
            body: vec![1],
        };
        assert_ne!(
            base,
            Chunk {
                id: *b"TMTA",
                body: vec![1]
            }
        );
        assert_ne!(
            base,
            Chunk {
                id: *b"CLUS",
                body: vec![2]
            }
        );
        assert_ne!(
            base,
            Chunk {
                id: *b"CLUS",
                body: vec![]
            }
        );
        assert_eq!(
            base,
            Chunk {
                id: *b"CLUS",
                body: vec![1]
            }
        );
    }

    #[test]
    fn chunk_debug_handles_a_large_body() {
        // §38 "maximum values": Debug is derived and gets called on whatever
        // a caller is holding; a big body must not blow up.
        let big = Chunk {
            id: *b"CLUS",
            body: vec![0xABu8; 4096],
        };
        let s = format!("{big:?}");
        assert!(s.starts_with("Chunk"));
    }

    // --- Container: an implementor's obligations, exercised end to end. ---

    #[test]
    fn write_then_read_round_trips_through_the_trait() {
        let mut c = Fake::default();
        let a = packet(&[1, 2, 3]);
        let b = packet(&[4, 5, 6]);
        c.write_packet(&a).unwrap();
        c.write_packet(&b).unwrap();
        c.finalize().unwrap();
        assert!(c.finalized);
        assert_eq!(c.written.len(), 2);

        // The write and read sides are independent: what was written is not
        // automatically readable, so feed the packets back explicitly.
        let mut r = Fake::with_packets(c.written.clone());
        assert_eq!(r.next_packet().unwrap(), a);
        assert_eq!(r.next_packet().unwrap(), b);
    }

    #[test]
    fn next_packet_past_the_end_is_truncated_not_a_panic() {
        // The contract the README states: a short read is an error. If this
        // becomes a panic or a silent `Ok`, every container built on the
        // trait inherits the bug. `ContainerError` has no `PartialEq`, so the
        // variant is matched rather than compared.
        let mut c = Fake::with_packets(vec![packet(&[1])]);
        assert!(c.next_packet().is_ok());
        assert!(matches!(c.next_packet(), Err(ContainerError::Truncated)));
    }

    #[test]
    fn next_packet_on_an_empty_container_is_truncated() {
        // §38 "empty input": no packets at all must be an error, not a hang
        // and not a fabricated zero-length packet.
        let mut c = Fake::default();
        assert!(matches!(c.next_packet(), Err(ContainerError::Truncated)));
    }

    #[test]
    fn a_zero_length_payload_is_a_valid_packet() {
        // An empty payload is legal at the codec level (silence, or a
        // header-only packet); the container must pass it through rather than
        // treating it as truncation.
        let mut c = Fake::with_packets(vec![packet(&[])]);
        assert!(c.next_packet().unwrap().data.is_empty());
    }

    #[test]
    fn next_packet_advances_and_does_not_repeat() {
        let mut c = Fake::with_packets(vec![packet(&[1]), packet(&[2])]);
        let first = c.next_packet().unwrap();
        let second = c.next_packet().unwrap();
        assert_eq!(first.data, vec![1]);
        assert_eq!(second.data, vec![2]);
    }

    #[test]
    fn seek_to_keyframe_leaves_the_reader_usable() {
        // A seek landing at or past the end must not corrupt the reader.
        let mut c = Fake::with_packets(vec![packet(&[9])]);
        c.seek_to_keyframe(i64::MAX).unwrap();
        // This fake does not move, so the packet is still there: proof the
        // call did not disturb the read position.
        assert_eq!(c.next_packet().unwrap().data, vec![9]);
    }

    #[test]
    fn seek_accepts_extreme_timestamps_without_panicking() {
        // §38 "maximum values"/"overflow": a hostile target must not become an
        // arithmetic trap inside an implementor.
        let mut c = Fake::with_packets(vec![]);
        for t in [i64::MIN, -1, 0, i64::MAX] {
            c.seek_to_keyframe(t).unwrap();
        }
    }

    #[test]
    fn stream_and_tracks_are_reachable_through_the_trait() {
        let c = Fake::default();
        // Both accessors hand out references, so they must work on a shared
        // borrow — a `&mut self` here would make the trait unusable by a
        // reader that only wants metadata.
        let _: &Stream = c.stream();
        assert!(c.tracks().is_empty());
    }

    // --- ContainerError: the crate's only error type. --------------------

    #[test]
    fn every_container_error_variant_is_nameable_and_keeps_its_payload() {
        // `ContainerError` derives Debug; callers match on the variant in code
        // and read the string in logs, and `Unsupported`/`Io` carry a message
        // that must survive rather than being dropped.
        let names: Vec<String> = [
            ContainerError::Truncated,
            ContainerError::InvalidHeader,
            ContainerError::MalformedChunk,
            ContainerError::Unsupported("nope"),
            ContainerError::Io("disk"),
        ]
        .iter()
        .map(|e| format!("{e:?}"))
        .collect();
        for want in ["Truncated", "InvalidHeader", "MalformedChunk"] {
            assert!(
                names.iter().any(|n| n.starts_with(want)),
                "no variant rendered as {want}: {names:?}"
            );
        }
        assert!(names.iter().any(|n| n.contains("nope")));
        assert!(names.iter().any(|n| n.contains("disk")));
    }

    #[test]
    fn container_result_alias_composes_both_ways() {
        // `ContainerResult<T>` is public; check it both as Ok and as Err.
        // `ContainerError` derives no `PartialEq`, so the Err side is matched
        // on the variant instead of compared.
        let ok: ContainerResult<u8> = Ok(1);
        let err: ContainerResult<u8> = Err(ContainerError::Truncated);
        assert!(ok.is_ok());
        assert!(matches!(err, Err(ContainerError::Truncated)));
    }
}
