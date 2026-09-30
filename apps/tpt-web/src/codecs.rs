//! The codec table: which decoders the player can build, keyed by the
//! track's codec ID. This is the player's **extension point**.
//!
//! Only the codecs that exist today are wired in: Cinder, Aura and Reed
//! (audio) and Thrum (video). Vault and Mote (todo.md sections 33 and 34) are
//! implemented separately and are deliberately *not* referenced here. To add
//! one, write a [`DecoderBuilder`] for it and either call
//! [`DecoderTable::register`] on a table you pass to
//! [`crate::Player::with_table`], or add one line to
//! [`DecoderTable::native`] once the codec crate is a dependency of this
//! crate. Nothing else in the player names a concrete codec: the parser is
//! codec-agnostic and the player only speaks [`DynDecoder`] /
//! [`DynFrame`].
//!
//! The builders mirror how `tpt-cli` reconstructs a decoder from a track
//! (sample rate from `TrackMedia`, per-codec parameters from the
//! codec-private bytes) rather than going through
//! [`tpt_media_codec::Registry`]: the audio codecs' registry factories build
//! decoders from `Default` configs and ignore the track, so a `.tpt` written
//! at 44.1 kHz or with non-default Cinder bit depth would decode wrongly
//! through them.

use crate::error::WebError;
use tpt_audio_aura::{AuraConfig, AuraDecoder, AuraDynDecoder};
use tpt_audio_cinder::{CinderDecoder, CinderDynDecoder, CinderEncoderConfig};
use tpt_audio_reed::{ReedConfig, ReedDecoder, ReedDynDecoder};
use tpt_media_codec::{
    CodecConfig as FrameworkConfig, CodecError, DynDecoder, DynDecoderFactory, ResourceLimits,
};
use tpt_media_core::{CodecId, MediaType, PixelFormat, Track};
use tpt_video_thrum::{ThrumConfig, ThrumDecoderFactory};

/// Build a decoder for `track`, enforcing `limits`.
///
/// A builder must validate anything it takes from the (untrusted) track
/// before allocating; the player additionally rejects video geometry that
/// exceeds `limits.max_frame_bytes` before calling it.
pub type DecoderBuilder = fn(&Track, &ResourceLimits) -> Result<Box<dyn DynDecoder>, WebError>;

struct Entry {
    codec: [u8; 4],
    media: MediaType,
    name: &'static str,
    build: DecoderBuilder,
}

/// A set of decoders keyed by `(codec ID, media type)`.
pub struct DecoderTable {
    entries: Vec<Entry>,
}

impl DecoderTable {
    /// An empty table (nothing is playable).
    pub fn empty() -> Self {
        DecoderTable {
            entries: Vec::new(),
        }
    }

    /// The decoders that exist in this workspace today: Cinder, Aura, Reed,
    /// Thrum. Vault and Mote are added here (or via
    /// [`DecoderTable::register`]) when they land.
    pub fn native() -> Self {
        let mut t = DecoderTable::empty();
        t.register(*b"CIND", MediaType::Audio, "cinder", build_cinder);
        t.register(*b"AURA", MediaType::Audio, "aura", build_aura);
        t.register(*b"REED", MediaType::Audio, "reed", build_reed);
        t.register(*b"THRM", MediaType::Video, "thrum", build_thrum);
        t
    }

    /// Register (or replace) the decoder for `codec` on tracks of type
    /// `media`. `name` is the human-readable label shown by
    /// [`crate::StreamInfo`].
    pub fn register(
        &mut self,
        codec: [u8; 4],
        media: MediaType,
        name: &'static str,
        build: DecoderBuilder,
    ) {
        self.entries
            .retain(|e| !(e.codec == codec && e.media == media));
        self.entries.push(Entry {
            codec,
            media,
            name,
            build,
        });
    }

    /// `true` if a decoder is registered for this codec ID and media type.
    pub fn supports(&self, codec: CodecId, media: MediaType) -> bool {
        self.find(codec, media).is_some()
    }

    /// Display name of the decoder for this codec ID and media type.
    pub fn name(&self, codec: CodecId, media: MediaType) -> Option<&'static str> {
        self.find(codec, media).map(|e| e.name)
    }

    /// Build the decoder for `track`, or `Ok(None)` if the codec is not
    /// registered for the track's media type.
    pub fn build(
        &self,
        track: &Track,
        limits: &ResourceLimits,
    ) -> Result<Option<Box<dyn DynDecoder>>, WebError> {
        match self.find(track.codec_config.codec_id, track.media_type) {
            Some(e) => (e.build)(track, limits).map(Some),
            None => Ok(None),
        }
    }

    fn find(&self, codec: CodecId, media: MediaType) -> Option<&Entry> {
        let id = codec.as_bytes();
        self.entries
            .iter()
            .find(|e| e.codec == id && e.media == media)
    }
}

impl Default for DecoderTable {
    fn default() -> Self {
        DecoderTable::native()
    }
}

fn codec_err(name: &str, e: CodecError) -> WebError {
    WebError::Codec(format!("{name}: {e}"))
}

fn framework(track: &Track, limits: &ResourceLimits) -> FrameworkConfig {
    FrameworkConfig {
        version: track.codec_config.version,
        extra: track.codec_config.data.clone(),
        limits: *limits,
    }
}

fn audio_rate(track: &Track) -> u32 {
    track.media.sample_rate.unwrap_or(48_000)
}

fn build_cinder(track: &Track, limits: &ResourceLimits) -> Result<Box<dyn DynDecoder>, WebError> {
    let private = &track.codec_config.data;
    let bits = private
        .first()
        .copied()
        .filter(|b| (4..=12).contains(b))
        .unwrap_or(8);
    let cfg = CinderEncoderConfig {
        sample_rate: audio_rate(track),
        bits_per_coef: bits,
        ..CinderEncoderConfig::default()
    };
    let d =
        CinderDecoder::new(framework(track, limits), cfg).map_err(|e| codec_err("cinder", e))?;
    Ok(Box::new(CinderDynDecoder::new(d)))
}

fn build_aura(track: &Track, limits: &ResourceLimits) -> Result<Box<dyn DynDecoder>, WebError> {
    let private = &track.codec_config.data;
    let block_size = if private.len() >= 2 {
        u16::from_le_bytes([private[0], private[1]])
    } else {
        1024
    };
    let cfg = AuraConfig {
        sample_rate: audio_rate(track),
        block_size,
        ..AuraConfig::default()
    };
    let d = AuraDecoder::new(framework(track, limits), cfg).map_err(|e| codec_err("aura", e))?;
    Ok(Box::new(AuraDynDecoder::new(d)))
}

fn build_reed(track: &Track, limits: &ResourceLimits) -> Result<Box<dyn DynDecoder>, WebError> {
    let cfg = ReedConfig::from_codec_private(audio_rate(track), &track.codec_config.data)
        .map_err(|e| codec_err("reed", e))?;
    let d = ReedDecoder::new(framework(track, limits), cfg).map_err(|e| codec_err("reed", e))?;
    Ok(Box::new(ReedDynDecoder::new(d)))
}

fn build_thrum(track: &Track, limits: &ResourceLimits) -> Result<Box<dyn DynDecoder>, WebError> {
    let width = track
        .media
        .width
        .ok_or_else(|| WebError::Codec("thrum: video track is missing its width".into()))?;
    let height = track
        .media
        .height
        .ok_or_else(|| WebError::Codec("thrum: video track is missing its height".into()))?;
    let private = &track.codec_config.data;
    let cfg = if private.len() >= ThrumConfig::CODEC_PRIVATE_LEN {
        ThrumConfig::from_codec_private(width, height, private)
            .map_err(|e| codec_err("thrum", e))?
    } else {
        ThrumConfig {
            width,
            height,
            format: track.media.pixel_format.unwrap_or(PixelFormat::Yuv420),
            ..ThrumConfig::default()
        }
    };
    // Thrum's decoder factory carries the geometry (a video codec's
    // configuration *is* its geometry), so a per-track factory is the
    // supported way to obtain a `DynDecoder`.
    let factory = ThrumDecoderFactory::new(cfg).map_err(|e| codec_err("thrum", e))?;
    factory
        .build(&framework(track, limits))
        .map_err(|e| codec_err("thrum", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_media_core::Timebase;

    fn track(codec: &[u8; 4], media: MediaType) -> Track {
        Track::new(0, media, CodecId::new(*codec), Timebase::MILLISECONDS)
    }

    #[test]
    fn native_table_covers_exactly_the_four_implemented_codecs() {
        let t = DecoderTable::native();
        for (id, media, name) in [
            (b"CIND", MediaType::Audio, "cinder"),
            (b"AURA", MediaType::Audio, "aura"),
            (b"REED", MediaType::Audio, "reed"),
            (b"THRM", MediaType::Video, "thrum"),
        ] {
            assert_eq!(t.name(CodecId::new(*id), media), Some(name));
        }
        // Wrong media type for a known codec, and unknown codecs.
        assert!(!t.supports(CodecId::new(*b"CIND"), MediaType::Video));
        assert!(!t.supports(CodecId::new(*b"NOPE"), MediaType::Audio));
    }

    #[test]
    fn unregistered_codec_builds_to_none_not_an_error() {
        let t = DecoderTable::native();
        let r = t
            .build(&track(b"NOPE", MediaType::Audio), &ResourceLimits::strict())
            .unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn video_track_without_geometry_is_a_clean_error() {
        let t = DecoderTable::native();
        let r = t.build(&track(b"THRM", MediaType::Video), &ResourceLimits::strict());
        assert!(matches!(r, Err(WebError::Codec(_))));
    }

    #[test]
    fn register_replaces_an_existing_entry() {
        let mut t = DecoderTable::native();
        fn nope(_: &Track, _: &ResourceLimits) -> Result<Box<dyn DynDecoder>, WebError> {
            Err(WebError::Unsupported("stub".into()))
        }
        t.register(*b"CIND", MediaType::Audio, "stub", nope);
        assert_eq!(
            t.name(CodecId::new(*b"CIND"), MediaType::Audio),
            Some("stub")
        );
        assert!(t
            .build(&track(b"CIND", MediaType::Audio), &ResourceLimits::strict())
            .is_err());
    }
}
