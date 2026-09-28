//! `tpt-media-core` — shared media data types for the `tpt-media` stack.
//!
//! This crate defines the vocabulary used by every other crate: timestamps,
//! durations, timebases, packets, frames, tracks, streams, codec identifiers,
//! pixel/sample formats, channel layouts, media types, and metadata.
//!
//! Design goals (see `ARCHITECTURE.md` §4):
//!
//! - **No `f64` timestamps.** Time is represented as rationals.
//! - **Caller-owned buffers.** Frame data is owned by the caller; the
//!   library never holds references to user data across calls.
//! - **`no_std`-friendly.** All types are `core`-only; the `std` feature
//!   pulls in a few convenience impls.
//!
//! Every public type implements `Debug`, `Clone` where allocation is
//! possible, and `PartialEq` where it makes sense.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

mod channel_layout;
mod codec_id;
mod frame;
mod metadata;
mod packet;
mod pixel_format;
mod sample_format;
mod sync;
mod timebase;
mod track;

pub use channel_layout::{ChannelLayout, ChannelMask};
pub use codec_id::{CodecConfig, CodecId};
pub use frame::{AudioFrame, Frame, VideoFrame};
pub use metadata::{Metadata, MetadataKey, MetadataValue};
pub use packet::{Packet, PacketFlags};
pub use pixel_format::PixelFormat;
pub use sample_format::SampleFormat;
pub use sync::{ClockDomain, SyncError, SyncMap, SyncPoint};
pub use timebase::{Duration, Timebase, Timestamp};
pub use track::{MediaType, Stream, Track, TrackMedia};

/// Convenience re-exports of common items.
pub mod prelude {
    pub use crate::{
        AudioFrame, ChannelLayout, ClockDomain, CodecConfig, CodecId, Duration, Frame, MediaType,
        Metadata, Packet, PacketFlags, PixelFormat, SampleFormat, Stream, SyncError, SyncMap,
        SyncPoint, Timebase, Timestamp, Track, TrackMedia, VideoFrame,
    };
}
