//! Stream and media-format negotiation between adjacent pipeline stages.
//!
//! Negotiation answers "do the outputs of stage *A* feed the inputs of
//! stage *B*?" before a single item flows. Linear pipelines negotiate
//! edge-by-edge; an `Option<&[MediaFormat]>` of `None` means "accepts
//! anything / produces anything" and acts as a pass-through edge.

use tpt_media_core::{ChannelLayout, CodecId, MediaType, PixelFormat, SampleFormat, Timebase};

use crate::node::NodeError;

/// A concrete media format carried by items on an edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaFormat {
    /// Raw interleaved PCM audio.
    Audio {
        sample_format: SampleFormat,
        channel_layout: ChannelLayout,
        sample_rate: u32,
    },
    /// Raw video frames.
    Video {
        pixel_format: PixelFormat,
        width: u32,
        height: u32,
    },
    /// Compressed/container-level items (packets of a single codec).
    Compressed {
        media_type: MediaType,
        codec_id: CodecId,
    },
    /// An application-defined opaque edge.
    Opaque(&'static str),
}

impl MediaFormat {
    /// Two formats are compatible when they describe the same data layout.
    pub fn compatible(&self, other: &Self) -> bool {
        self == other
    }
}

/// A stage that can describe the formats it consumes and produces.
///
/// Both accessors return `None` for "no constraint / unknown" — such a
/// stage accepts anything (input) or produces anything (output). Returning
/// `Some(&[])` means "accepts/produces nothing", i.e. an immediate
/// mismatch.
pub trait FormatAware {
    /// Formats this stage can consume; `None` accepts any.
    fn input_formats(&self) -> Option<&[MediaFormat]> {
        None
    }

    /// Formats this stage produces; `None` produces any.
    fn output_formats(&self) -> Option<&[MediaFormat]> {
        None
    }
}

/// Resolve the agreed media format flowing from `left`'s outputs into
/// `right`'s inputs.
pub fn negotiate_formats<L: FormatAware, R: FormatAware>(
    left: &L,
    right: &R,
) -> Result<MediaFormat, NodeError> {
    let out = left.output_formats();
    let inp = right.input_formats();
    match (out, inp) {
        (Some(outs), Some(ins)) => outs
            .iter()
            .find(|a| ins.iter().any(|b| a.compatible(b)))
            .cloned()
            .ok_or(NodeError::FormatMismatch),
        (Some(outs), None) => outs.first().cloned().ok_or(NodeError::FormatMismatch),
        (None, Some(ins)) => ins.first().cloned().ok_or(NodeError::FormatMismatch),
        (None, None) => Err(NodeError::FormatMismatch),
    }
}

/// A negotiated stream-level contract (timebase + contained media).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSpec {
    /// Shared media timebase, if both sides agree on one.
    pub timebase: Option<Timebase>,
    /// The edge carries audio.
    pub has_audio: bool,
    /// The edge carries video.
    pub has_video: bool,
}

/// Merge two stream descriptions, failing when both declare an explicit
/// timebase but disagree.
pub fn negotiate_stream(a: &StreamSpec, b: &StreamSpec) -> Result<StreamSpec, NodeError> {
    let timebase = match (a.timebase, b.timebase) {
        (Some(x), Some(y)) if x != y => return Err(NodeError::FormatMismatch),
        (x, y) => x.or(y),
    };
    Ok(StreamSpec {
        timebase,
        has_audio: a.has_audio || b.has_audio,
        has_video: a.has_video || b.has_video,
    })
}
