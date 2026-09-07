//! `tpt-media-container` — generic container abstraction.
//!
//! The [`Container`] trait abstracts over the wire format of a media
//! container so that codecs do not need to know whether they are
//! packaged inside `.tpt`, `.mp4`, or some future TPT-defined format.
//! The native implementation lives in the `tpt-container` crate.

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

    #[test]
    fn chunk_construction() {
        let c = Chunk {
            id: *b"CLUS",
            body: vec![1, 2, 3],
        };
        assert_eq!(c.id, *b"CLUS");
        assert_eq!(c.body.len(), 3);
    }
}