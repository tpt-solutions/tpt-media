//! Track, stream, and media-type definitions.

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec_id::{CodecConfig, CodecId};
use crate::timebase::Timebase;

/// Type of media carried on a track.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MediaType {
    /// Audio track.
    Audio,
    /// Video track.
    Video,
    /// Subtitle / caption track.
    Subtitle,
    /// Metadata / side-data track.
    Metadata,
}

/// Description of a single track in a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// Track index within the stream.
    pub id: u8,
    /// Codec configuration.
    pub codec_config: CodecConfig,
    /// Media type.
    pub media_type: MediaType,
    /// Track-specific timebase.
    pub timebase: Timebase,
    /// Human-readable name (optional).
    pub name: Option<String>,
    /// Language tag (BCP-47), optional.
    pub language: Option<String>,
    /// Free-form metadata.
    pub metadata: Vec<(String, MetadataValue)>,
}

impl Track {
    /// Build a minimal track descriptor.
    pub fn new(id: u8, media_type: MediaType, codec_id: CodecId, timebase: Timebase) -> Self {
        Track {
            id,
            codec_config: CodecConfig::empty(codec_id),
            media_type,
            timebase,
            name: None,
            language: None,
            metadata: Vec::new(),
        }
    }
}

/// A media stream is a collection of tracks plus a global timebase.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Stream {
    /// All tracks in the stream.
    pub tracks: Vec<Track>,
    /// Global (file-level) timebase.
    pub global_timebase: Timebase,
}

impl Stream {
    /// Empty stream with the given global timebase.
    pub fn empty(global_timebase: Timebase) -> Self {
        Stream {
            tracks: Vec::new(),
            global_timebase,
        }
    }

    /// Add a track to the stream and return its new index.
    pub fn add_track(&mut self, track: Track) -> u8 {
        let id = self.tracks.len() as u8;
        let mut t = track;
        t.id = id;
        self.tracks.push(t);
        id
    }

    /// Number of tracks.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Find a track by id.
    pub fn track(&self, id: u8) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Find a track by id (mutable).
    pub fn track_mut(&mut self, id: u8) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }
}

// Local re-export so that `Track::metadata` can use `MetadataValue`
// without circular imports.
pub use crate::metadata::MetadataValue;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_track_assigns_id() {
        let mut s = Stream::empty(Timebase::MILLISECONDS);
        let t = Track::new(0, MediaType::Audio, CodecId::new(*b"PULS"), Timebase::MILLISECONDS);
        let id = s.add_track(t);
        assert_eq!(id, 0);
        assert_eq!(s.track_count(), 1);
    }

    #[test]
    fn find_track() {
        let mut s = Stream::empty(Timebase::MILLISECONDS);
        s.add_track(Track::new(0, MediaType::Audio, CodecId::new(*b"PULS"), Timebase::MILLISECONDS));
        assert!(s.track(0).is_some());
        assert!(s.track(7).is_none());
    }
}