//! `tpt-media-codec` — generic codec framework.
//!
//! Provides the [`Encoder`], [`Decoder`], [`CodecInfo`], [`CodecConfig`],
//! [`Capabilities`], [`ResourceLimits`], and the type-erased
//! [`Registry`] / [`DynDecoder`] / [`DynEncoder`] abstraction.
//!
//! See `ARCHITECTURE.md` §6 and `docs/codec-design.md`.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use tpt_media_core::Packet;

/// Result of a codec operation.
pub type CodecResult<T> = Result<T, CodecError>;

/// Errors produced by codecs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    /// The packet was malformed or truncated.
    MalformedPacket,
    /// The decoder requires more input before producing a frame.
    NeedMoreInput,
    /// The decoder was cancelled.
    Cancelled,
    /// A configured resource limit was hit.
    ResourceLimit(ResourceLimitKind),
    /// An unsupported feature was requested.
    Unsupported(&'static str),
    /// Configuration was rejected.
    InvalidConfig(&'static str),
    /// Internal invariant violated.
    Internal,
    /// Bitstream-level error.
    Bitstream(String),
    /// Downstream container or pipeline error.
    Downstream(String),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecError::MalformedPacket => f.write_str("malformed packet"),
            CodecError::NeedMoreInput => f.write_str("need more input"),
            CodecError::Cancelled => f.write_str("cancelled"),
            CodecError::ResourceLimit(k) => write!(f, "resource limit: {:?}", k),
            CodecError::Unsupported(s) => write!(f, "unsupported: {}", s),
            CodecError::InvalidConfig(s) => write!(f, "invalid config: {}", s),
            CodecError::Internal => f.write_str("internal error"),
            CodecError::Bitstream(s) => write!(f, "bitstream: {}", s),
            CodecError::Downstream(s) => write!(f, "downstream: {}", s),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for CodecError {}

/// Which resource limit was hit.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResourceLimitKind {
    /// Frame dimensions exceed the configured maximum.
    FrameDimensions,
    /// Packet size exceeds the configured maximum.
    PacketSize,
    /// Reference frame count exceeds the configured maximum.
    ReferenceFrames,
    /// Allocation exceeds the configured maximum.
    Allocation,
    /// Recursion depth exceeded.
    RecursionDepth,
    /// Decoder CPU-time budget exceeded (best-effort).
    CpuBudget,
    /// Metadata size exceeds the configured maximum.
    MetadataSize,
}

/// Resource limits enforced by a decoder.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Maximum frame width × height in pixels (or samples × channels for audio).
    pub max_frame_bytes: u32,
    /// Maximum packet size in bytes.
    pub max_packet_bytes: u32,
    /// Maximum reference frame count.
    pub max_reference_frames: u8,
    /// Maximum decoder-internal allocation in bytes.
    pub max_allocation_bytes: u32,
    /// Maximum recursion depth.
    pub max_recursion_depth: u8,
    /// Optional CPU-time budget per decode call, in microseconds (0 = disabled).
    pub max_decode_micros: u32,
    /// Maximum size in bytes of a single metadata chunk/record (e.g. a
    /// container's per-track `TMTA` block) before it is even parsed.
    pub max_metadata_bytes: u32,
}

impl ResourceLimits {
    /// Permissive limits suitable for trusted input.
    pub const fn permissive() -> Self {
        ResourceLimits {
            max_frame_bytes: 256 * 1024 * 1024,
            max_packet_bytes: 64 * 1024 * 1024,
            max_reference_frames: 32,
            max_allocation_bytes: 512 * 1024 * 1024,
            max_recursion_depth: 16,
            max_decode_micros: 0,
            max_metadata_bytes: 16 * 1024 * 1024,
        }
    }

    /// Conservative limits suitable for untrusted input.
    pub const fn strict() -> Self {
        ResourceLimits {
            max_frame_bytes: 16 * 1024 * 1024,
            max_packet_bytes: 4 * 1024 * 1024,
            max_reference_frames: 8,
            max_allocation_bytes: 32 * 1024 * 1024,
            max_recursion_depth: 6,
            max_decode_micros: 50_000,
            max_metadata_bytes: 64 * 1024,
        }
    }

    /// Check that a packet size is within the configured maximum.
    pub fn check_packet(&self, len: usize) -> CodecResult<()> {
        if len > self.max_packet_bytes as usize {
            return Err(CodecError::ResourceLimit(ResourceLimitKind::PacketSize));
        }
        Ok(())
    }

    /// Check that a frame byte count is within the configured maximum.
    pub fn check_frame_bytes(&self, bytes: usize) -> CodecResult<()> {
        if bytes > self.max_frame_bytes as usize {
            return Err(CodecError::ResourceLimit(
                ResourceLimitKind::FrameDimensions,
            ));
        }
        Ok(())
    }

    /// Check that a reference-frame count is within the configured maximum.
    pub fn check_reference_frames(&self, n: u8) -> CodecResult<()> {
        if n > self.max_reference_frames {
            return Err(CodecError::ResourceLimit(
                ResourceLimitKind::ReferenceFrames,
            ));
        }
        Ok(())
    }

    /// Check that an allocation is within the configured maximum.
    pub fn check_allocation(&self, bytes: usize) -> CodecResult<()> {
        if bytes > self.max_allocation_bytes as usize {
            return Err(CodecError::ResourceLimit(ResourceLimitKind::Allocation));
        }
        Ok(())
    }

    /// Check that a recursion depth is within the configured maximum.
    pub fn check_recursion_depth(&self, depth: u8) -> CodecResult<()> {
        if depth > self.max_recursion_depth {
            return Err(CodecError::ResourceLimit(ResourceLimitKind::RecursionDepth));
        }
        Ok(())
    }

    /// Check that a metadata block size is within the configured maximum.
    pub fn check_metadata(&self, bytes: usize) -> CodecResult<()> {
        if bytes > self.max_metadata_bytes as usize {
            return Err(CodecError::ResourceLimit(ResourceLimitKind::MetadataSize));
        }
        Ok(())
    }
}

impl Default for ResourceLimits {
    fn default() -> Self {
        ResourceLimits::strict()
    }
}

/// Capabilities advertised by a codec implementation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct Capabilities {
    /// Real-time-safe decoder.
    pub real_time_decoder: bool,
    /// Real-time-safe encoder.
    pub real_time_encoder: bool,
    /// Hardware/SIMD acceleration available.
    pub hardware_accel: bool,
    /// Lossless mode available.
    pub lossless: bool,
    /// Lossy mode available.
    pub lossy: bool,
    /// Supports decoder drain (graceful EOS with remaining buffered
    /// frames flushed out, distinct from a hard flush).
    pub supports_drain: bool,
    /// Supports encoder drain (graceful EOS with remaining buffered
    /// packets flushed out, distinct from a hard flush).
    pub supports_encoder_drain: bool,
}

/// A media format (sample format / channel layout / sample rate, or
/// pixel format / dimensions) that a codec implementation claims to
/// support.
///
/// `supported_formats` in `CodecInfo` is the list of formats a decoder
/// *can read* (or an encoder *can produce*) without further probing;
/// anything outside this list is reported as `CodecError::Unsupported`
/// rather than silently producing garbage. This is the contract that
/// lets a pipeline validate `Track → Codec` pairings up front instead
/// of discovering mismatches mid-decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupportedFormat {
    /// Audio sample/format/rate/layout that the codec handles.
    Audio {
        sample_format: tpt_media_core::SampleFormat,
        channel_layout: tpt_media_core::ChannelLayout,
        sample_rate: u32,
    },
    /// Video pixel format / dimensions that the codec handles.
    Video {
        pixel_format: tpt_media_core::PixelFormat,
        width: u32,
        height: u32,
    },
}

impl SupportedFormat {
    /// True if `other` is compatible with `self` — i.e., if the codec's
    /// declared support covers the format the caller is asking for.
    ///
    /// For audio: sample_format / channel_layout must match exactly,
    /// sample_rate must match exactly (no resampling is assumed at the
    /// codec level; resampling is a separate filter node).
    ///
    /// For video: pixel_format must match exactly, dimensions must
    /// match exactly (no implicit scaling).
    pub fn matches(&self, other: &SupportedFormat) -> bool {
        match (self, other) {
            (
                SupportedFormat::Audio {
                    sample_format: a,
                    channel_layout: b,
                    sample_rate: c,
                },
                SupportedFormat::Audio {
                    sample_format: d,
                    channel_layout: e,
                    sample_rate: f,
                },
            ) => a == d && b == e && c == f,
            (
                SupportedFormat::Video {
                    pixel_format: a,
                    width: b,
                    height: c,
                },
                SupportedFormat::Video {
                    pixel_format: d,
                    width: e,
                    height: f,
                },
            ) => a == d && b == e && c == f,
            _ => false,
        }
    }
}

/// Static information about a codec implementation.
#[derive(Clone, Debug)]
pub struct CodecInfo {
    /// Human name (e.g. "Cinder").
    pub name: &'static str,
    /// 4-byte codec identifier (e.g. "CIND").
    pub codec_id: [u8; 4],
    /// Bitstream version this implementation understands.
    pub version: u16,
    /// Capabilities.
    pub capabilities: Capabilities,
    /// Formats this codec handles (audio sample/format/rate/layout
    /// combinations, or video pixel/dimensions). Pipelines use this to
    /// reject track→codec pairings up front rather than discovering the
    /// mismatch mid-decode.
    pub supported_formats: Vec<SupportedFormat>,
}

impl CodecInfo {
    /// True if this codec advertises support for `format`.
    pub fn supports_format(&self, format: &SupportedFormat) -> bool {
        self.supported_formats.iter().any(|f| f.matches(format))
    }
}

/// Codec configuration passed to encoders/decoders.
#[derive(Clone, Debug)]
pub struct CodecConfig {
    /// Bitstream version to produce/consume (`major << 8 | minor`).
    pub version: u16,
    /// Codec-private configuration bytes.
    pub extra: Vec<u8>,
    /// Resource limits to enforce.
    pub limits: ResourceLimits,
}

impl Default for CodecConfig {
    fn default() -> Self {
        CodecConfig {
            version: 1,
            extra: Vec::new(),
            limits: ResourceLimits::default(),
        }
    }
}

/// Decoder trait.
pub trait Decoder {
    type Frame;
    /// Static information about this codec.
    fn info(&self) -> &CodecInfo;
    /// Decode one packet and return zero or more frames.
    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<Self::Frame>>;
    /// Flush any buffered state and return any held-back frames.
    ///
    /// `flush` is a **hard** reset: it discards decoder-internal state
    /// (reference frames, sample-rate-conversion history, etc.) so the
    /// decoder can be reused on an unrelated stream. For a graceful
    /// end-of-stream that emits the remaining buffered frames without
    /// throwing state away, use [`Decoder::drain`].
    fn flush(&mut self) -> CodecResult<Vec<Self::Frame>> {
        Ok(Vec::new())
    }
    /// Drain the decoder at end-of-stream.
    ///
    /// Distinct from `flush`: a drain emits any frames the decoder is
    /// still holding back (e.g., a frame whose second field arrived in
    /// the last packet but whose first field won't arrive again),
    /// without discarding reference frames or otherwise preparing the
    /// decoder for a new stream. After `drain` returns, the codec has
    /// reached a stable end-of-stream state and may be reused only
    /// after a fresh `flush`.
    ///
    /// The default implementation forwards to `flush`, which is the
    /// right behaviour for codecs that have no look-ahead or buffered
    /// output (most audio codecs, simple video codecs). Codecs that
    /// hold back frames — MDCT overlap-add, B-frame reordering, etc.
    /// — should override this and advertise `supports_drain` in
    /// `Capabilities`.
    fn drain(&mut self) -> CodecResult<Vec<Self::Frame>> {
        self.flush()
    }
}

/// Encoder trait.
pub trait Encoder {
    type Frame;
    /// Static information about this codec.
    fn info(&self) -> &CodecInfo;
    /// Encode one frame and return zero or more packets.
    fn encode(&mut self, frame: &Self::Frame) -> CodecResult<Vec<Packet>>;
    /// Flush the encoder, returning any buffered packets.
    ///
    /// `flush` is a **hard** reset: it discards encoder-internal state
    /// (rate-control history, B-frame queue, etc.) so the encoder can
    /// be reused on an unrelated stream. For a graceful end-of-stream
    /// that emits the remaining buffered packets without throwing
    /// state away, use [`Encoder::drain`].
    fn flush(&mut self) -> CodecResult<Vec<Packet>> {
        Ok(Vec::new())
    }
    /// Drain the encoder at end-of-stream.
    ///
    /// Distinct from `flush`: a drain emits any packets the encoder is
    /// still holding back (e.g., delayed B-frames that depend on
    /// future P/I frames, an MDCT window that hasn't been quantised
    /// yet) without discarding rate-control history or otherwise
    /// preparing the encoder for a new stream. The default forwards
    /// to `flush`; codecs with buffered output should override this
    /// and advertise `supports_encoder_drain` in `Capabilities`.
    fn drain(&mut self) -> CodecResult<Vec<Packet>> {
        self.flush()
    }
}

/// Type-erased frame payload.
#[derive(Clone, Debug)]
pub enum DynFrame {
    /// Audio frame bytes (interleaved).
    Audio {
        sample_format: tpt_media_core::SampleFormat,
        channel_layout: tpt_media_core::ChannelLayout,
        sample_rate: u32,
        sample_count: u32,
        samples: Vec<u8>,
    },
    /// Video frame.
    Video {
        pixel_format: tpt_media_core::PixelFormat,
        width: u32,
        height: u32,
        stride: u32,
        pixels: Vec<u8>,
    },
}

/// Type-erased decoder factory.
pub trait DynDecoderFactory {
    fn codec_id(&self) -> &[u8; 4];
    fn info(&self) -> CodecInfo;
    fn build(&self, cfg: &CodecConfig) -> CodecResult<Box<dyn DynDecoder>>;
}

/// Type-erased decoder.
pub trait DynDecoder {
    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<DynFrame>>;
    fn flush(&mut self) -> CodecResult<Vec<DynFrame>>;
    /// Drain the decoder at end-of-stream (graceful EOS — emit any
    /// remaining buffered frames, do not discard reference state).
    fn drain(&mut self) -> CodecResult<Vec<DynFrame>>;
    fn info(&self) -> CodecInfo;
}

/// Type-erased encoder factory.
pub trait DynEncoderFactory {
    fn codec_id(&self) -> &[u8; 4];
    fn info(&self) -> CodecInfo;
    fn build(&self, cfg: &CodecConfig) -> CodecResult<Box<dyn DynEncoder>>;
}

/// Type-erased encoder.
pub trait DynEncoder {
    fn encode(&mut self, frame: &DynFrame) -> CodecResult<Vec<Packet>>;
    fn flush(&mut self) -> CodecResult<Vec<Packet>>;
    /// Drain the encoder at end-of-stream (graceful EOS — emit any
    /// remaining buffered packets, do not discard rate-control state).
    fn drain(&mut self) -> CodecResult<Vec<Packet>>;
    fn info(&self) -> CodecInfo;
}

/// Codec discovery — a registry of available codec implementations.
#[derive(Default)]
pub struct Registry {
    decoders: Vec<Box<dyn DynDecoderFactory>>,
    encoders: Vec<Box<dyn DynEncoderFactory>>,
}

impl Registry {
    pub fn new() -> Self {
        Registry::default()
    }
    pub fn register_decoder<F: 'static + DynDecoderFactory>(&mut self, f: F) {
        self.decoders.push(Box::new(f));
    }
    pub fn register_encoder<F: 'static + DynEncoderFactory>(&mut self, f: F) {
        self.encoders.push(Box::new(f));
    }
    pub fn find_decoder(&self, codec_id: &[u8; 4]) -> Option<&dyn DynDecoderFactory> {
        self.decoders
            .iter()
            .find(|f| f.codec_id() == codec_id)
            .map(|f| f.as_ref())
    }
    pub fn find_encoder(&self, codec_id: &[u8; 4]) -> Option<&dyn DynEncoderFactory> {
        self.encoders
            .iter()
            .find(|f| f.codec_id() == codec_id)
            .map(|f| f.as_ref())
    }
    pub fn decoder_ids<'a>(&'a self) -> impl Iterator<Item = &'a [u8; 4]> + 'a {
        self.decoders.iter().map(|f| f.codec_id())
    }
    pub fn encoder_ids<'a>(&'a self) -> impl Iterator<Item = &'a [u8; 4]> + 'a {
        self.encoders.iter().map(|f| f.codec_id())
    }
}

impl core::fmt::Debug for Registry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Registry")
            .field("decoders", &self.decoders.len())
            .field("encoders", &self.encoders.len())
            .finish()
    }
}

/// Helper that enforces decoder-side resource limits before each
/// `decode` call.
pub struct PacketGuard<'a> {
    limits: &'a ResourceLimits,
}

impl<'a> PacketGuard<'a> {
    pub fn new(limits: &'a ResourceLimits) -> Self {
        PacketGuard { limits }
    }
    pub fn check<F, R>(&self, packet: &Packet, f: F) -> CodecResult<R>
    where
        F: FnOnce() -> CodecResult<R>,
    {
        self.limits.check_packet(packet.data.len())?;
        f()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_is_stricter_than_permissive() {
        assert!(
            ResourceLimits::strict().max_packet_bytes
                < ResourceLimits::permissive().max_packet_bytes
        );
    }

    #[test]
    fn packet_guard_rejects_oversized() {
        let limits = ResourceLimits::strict();
        let guard = PacketGuard::new(&limits);
        let mut p = Packet::empty(
            0,
            tpt_media_core::Timestamp::zero(tpt_media_core::Timebase::MILLISECONDS),
        );
        p.data = alloc::vec![0u8; (limits.max_packet_bytes as usize) + 1];
        let result: CodecResult<()> = guard.check(&p, || Ok(()));
        assert!(matches!(
            result,
            Err(CodecError::ResourceLimit(ResourceLimitKind::PacketSize))
        ));
    }

    #[test]
    fn registry_register_and_find() {
        struct DummyFactory;
        impl DynDecoderFactory for DummyFactory {
            fn codec_id(&self) -> &[u8; 4] {
                b"DUM1"
            }
            fn info(&self) -> CodecInfo {
                CodecInfo {
                    name: "Dummy",
                    codec_id: *b"DUM1",
                    version: 1,
                    capabilities: Capabilities::default(),
                    supported_formats: Vec::new(),
                }
            }
            fn build(&self, _cfg: &CodecConfig) -> CodecResult<Box<dyn DynDecoder>> {
                unimplemented!()
            }
        }
        let mut reg = Registry::new();
        reg.register_decoder(DummyFactory);
        assert!(reg.find_decoder(b"DUM1").is_some());
        assert!(reg.find_decoder(b"NOPE").is_none());
        assert_eq!(reg.decoder_ids().count(), 1);
    }

    #[test]
    fn allocation_check() {
        let l = ResourceLimits::strict();
        assert!(l.check_allocation(1024).is_ok());
        assert!(l.check_allocation(usize::MAX).is_err());
    }

    #[test]
    fn recursion_check() {
        let l = ResourceLimits::strict();
        assert!(l.check_recursion_depth(3).is_ok());
        assert!(l.check_recursion_depth(20).is_err());
    }

    #[test]
    fn reference_frame_check() {
        let l = ResourceLimits::strict();
        assert!(l.check_reference_frames(4).is_ok());
        assert!(l.check_reference_frames(32).is_err());
    }

    // --- §38 unit-test gate: the public items with no coverage ------------

    #[test]
    fn default_limits_are_the_strict_ones() {
        // `Default` is what every `CodecConfig::default()` carries, so what
        // it resolves to is the policy every codec starts from.
        assert_eq!(ResourceLimits::default(), ResourceLimits::strict());
        assert_ne!(ResourceLimits::default(), ResourceLimits::permissive());
    }

    #[test]
    fn strict_is_stricter_on_every_field_not_just_packets() {
        // The original test compared one field, which would have passed with
        // a typo in any of the other six.
        let s = ResourceLimits::strict();
        let p = ResourceLimits::permissive();
        assert!(s.max_frame_bytes < p.max_frame_bytes);
        assert!(s.max_packet_bytes < p.max_packet_bytes);
        assert!(s.max_reference_frames < p.max_reference_frames);
        assert!(s.max_allocation_bytes < p.max_allocation_bytes);
        assert!(s.max_recursion_depth < p.max_recursion_depth);
        assert!(s.max_metadata_bytes < p.max_metadata_bytes);
        // The CPU budget is the one field where "stricter" means *set*:
        // permissive leaves it disabled (0).
        assert_eq!(p.max_decode_micros, 0);
        assert!(s.max_decode_micros > 0);
    }

    #[test]
    fn every_limit_is_inclusive_at_its_maximum() {
        // "Maximum X" means X is allowed and X+1 is not. Pinned per limit,
        // because an off-by-one here is silently a security hole in one
        // direction and a spurious rejection in the other.
        let l = ResourceLimits::strict();
        let m = l.max_packet_bytes as usize;
        assert!(l.check_packet(m).is_ok());
        assert!(l.check_packet(m + 1).is_err());

        let m = l.max_frame_bytes as usize;
        assert!(l.check_frame_bytes(m).is_ok());
        assert!(l.check_frame_bytes(m + 1).is_err());

        let m = l.max_allocation_bytes as usize;
        assert!(l.check_allocation(m).is_ok());
        assert!(l.check_allocation(m + 1).is_err());

        let m = l.max_metadata_bytes as usize;
        assert!(l.check_metadata(m).is_ok());
        assert!(l.check_metadata(m + 1).is_err());

        assert!(l.check_reference_frames(l.max_reference_frames).is_ok());
        assert!(l.check_reference_frames(l.max_reference_frames + 1).is_err());

        assert!(l.check_recursion_depth(l.max_recursion_depth).is_ok());
        assert!(l
            .check_recursion_depth(l.max_recursion_depth + 1)
            .is_err());
    }

    #[test]
    fn zero_is_always_within_every_limit() {
        // The empty case: a zero-length packet, frame, allocation or
        // metadata block must not be rejected by a size limit.
        let l = ResourceLimits::strict();
        assert!(l.check_packet(0).is_ok());
        assert!(l.check_frame_bytes(0).is_ok());
        assert!(l.check_allocation(0).is_ok());
        assert!(l.check_metadata(0).is_ok());
        assert!(l.check_reference_frames(0).is_ok());
        assert!(l.check_recursion_depth(0).is_ok());
    }

    #[test]
    fn usize_max_is_rejected_by_every_byte_limit() {
        // The maximum value a caller can pass, on a 64-bit target. A limit
        // compared through a narrower cast would let this through.
        let l = ResourceLimits::permissive();
        for err in [
            l.check_packet(usize::MAX),
            l.check_frame_bytes(usize::MAX),
            l.check_allocation(usize::MAX),
            l.check_metadata(usize::MAX),
        ] {
            assert!(err.is_err(), "usize::MAX must not pass a byte limit");
        }
    }

    #[test]
    fn each_limit_reports_its_own_kind() {
        // A caller deciding "retry with a smaller packet?" cannot tell which
        // limit fired if every size check reports the same variant.
        let l = ResourceLimits::strict();
        let pairs = [
            (l.check_packet(usize::MAX), ResourceLimitKind::PacketSize),
            (
                l.check_frame_bytes(usize::MAX),
                ResourceLimitKind::FrameDimensions,
            ),
            (
                l.check_reference_frames(u8::MAX),
                ResourceLimitKind::ReferenceFrames,
            ),
            (
                l.check_allocation(usize::MAX),
                ResourceLimitKind::Allocation,
            ),
            (
                l.check_recursion_depth(u8::MAX),
                ResourceLimitKind::RecursionDepth,
            ),
            (
                l.check_metadata(usize::MAX),
                ResourceLimitKind::MetadataSize,
            ),
        ];
        let mut got: Vec<ResourceLimitKind> = pairs
            .iter()
            .map(|(r, _)| match r {
                Err(CodecError::ResourceLimit(k)) => *k,
                other => panic!("expected a ResourceLimit error, got {other:?}"),
            })
            .collect();
        let mut want: Vec<ResourceLimitKind> = pairs.iter().map(|(_, k)| *k).collect();
        // `ResourceLimitKind` has no `Ord`, so compare as a sorted-by-debug
        // pair list: a set comparison that also proves all six are distinct.
        got.sort_by_key(|k| alloc::format!("{k:?}"));
        want.sort_by_key(|k| alloc::format!("{k:?}"));
        assert_eq!(got, want);
        assert_eq!(got.len(), 6);
    }

    #[test]
    fn cpu_budget_is_advertised_but_has_no_checker_yet() {
        // `max_decode_micros` is a knob with no enforcement function — the
        // same honest gap todo.md section 19 records. Asserted here so that
        // adding the checker is a deliberate change to a pinned state
        // rather than an accident.
        assert!(ResourceLimits::strict().max_decode_micros > 0);
        assert_eq!(ResourceLimits::permissive().max_decode_micros, 0);
    }

    #[test]
    fn packet_guard_passes_the_closure_result_through() {
        let limits = ResourceLimits::strict();
        let guard = PacketGuard::new(&limits);
        let mut p = Packet::empty(
            0,
            tpt_media_core::Timestamp::zero(tpt_media_core::Timebase::MILLISECONDS),
        );
        p.data = alloc::vec![7u8; 16];
        let out: CodecResult<u32> = guard.check(&p, || Ok(42));
        assert_eq!(out.unwrap(), 42);
    }

    #[test]
    fn packet_guard_accepts_an_empty_packet() {
        // Zero bytes is within every limit, so the closure must still run.
        let limits = ResourceLimits::strict();
        let guard = PacketGuard::new(&limits);
        let p = Packet::empty(
            0,
            tpt_media_core::Timestamp::zero(tpt_media_core::Timebase::MILLISECONDS),
        );
        let out: CodecResult<u32> = guard.check(&p, || Ok(1));
        assert_eq!(out.unwrap(), 1);
    }

    #[test]
    fn packet_guard_does_not_call_the_closure_when_it_rejects() {
        // The guard exists to *prevent* the work. If the closure ran before
        // the check, the limit would be advisory and a hostile packet would
        // already have been parsed.
        use core::cell::Cell;
        let limits = ResourceLimits::strict();
        let guard = PacketGuard::new(&limits);
        let mut p = Packet::empty(
            0,

    fn audio(rate: u32) -> SupportedFormat {
        SupportedFormat::Audio {
            sample_format: tpt_media_core::SampleFormat::S16,
            channel_layout: tpt_media_core::ChannelLayout::Mono,
            sample_rate: rate,
        }
    }

    fn video(w: u32, h: u32) -> SupportedFormat {
        SupportedFormat::Video {
            pixel_format: tpt_media_core::PixelFormat::Yuv420,
            width: w,
            height: h,
        }
    }

    #[test]
    fn supported_format_matching_is_exact_within_a_variant() {
        assert!(audio(48_000).matches(&audio(48_000)));
        assert!(video(320, 240).matches(&video(320, 240)));

        // A different sample rate is not "close enough": no resampling is
        // assumed at the codec level.
        assert!(!audio(48_000).matches(&audio(44_100)));
        // Nor is a different sample format or channel layout.
        assert!(!audio(48_000).matches(&SupportedFormat::Audio {
            sample_format: tpt_media_core::SampleFormat::F32,
            channel_layout: tpt_media_core::ChannelLayout::Mono,
            sample_rate: 48_000,
        }));
        // Nor a different pixel format or dimension.
        assert!(!video(320, 240).matches(&video(320, 241)));
        assert!(!video(320, 240).matches(&video(640, 240)));
    }

    #[test]
    fn an_audio_format_never_matches_a_video_format() {
        // Cross-variant is false in both directions: a video track offered
        // to an audio-only codec must be rejected, not reinterpreted.
        assert!(!audio(48_000).matches(&video(320, 240)));
        assert!(!video(320, 240).matches(&audio(48_000)));
    }

    #[test]
    fn supports_format_searches_the_declared_list() {
        let info = CodecInfo {
            name: "Test",
            codec_id: *b"TST1",
            version: 1,
            capabilities: Capabilities::default(),
            supported_formats: alloc::vec![audio(48_000), video(320, 240)],
        };
        assert!(info.supports_format(&audio(48_000)));
        assert!(info.supports_format(&video(320, 240)));
        assert!(!info.supports_format(&audio(44_100)));
        assert!(!info.supports_format(&video(640, 480)));
    }

    #[test]
    fn a_codec_declaring_no_formats_supports_none() {
        // An empty list is the honest answer for a codec that has not been
        // characterised yet. It must not be read as "supports anything",
        // which is the failure mode an `is_empty() || ...` shortcut causes.
        let info = CodecInfo {
            name: "Uncharacterised",
            codec_id: *b"UNKN",
            version: 1,
            capabilities: Capabilities::default(),
            supported_formats: Vec::new(),
        };
        assert!(!info.supports_format(&audio(48_000)));
        assert!(!info.supports_format(&video(320, 240)));
    }

    #[test]
    fn capabilities_default_to_nothing_claimed() {
        let c = Capabilities::default();
        assert!(!c.real_time_decoder);
        assert!(!c.real_time_encoder);
        assert!(!c.hardware_accel);
        assert!(!c.lossless);
        assert!(!c.lossy);
        assert!(!c.supports_drain);
        assert!(!c.supports_encoder_drain);
    }

    #[test]
    fn codec_config_default_is_version_one_with_strict_limits() {
        // What every codec's `CodecConfig::default()` starts from.
        let cfg = CodecConfig::default();
        assert_eq!(cfg.version, 1);
        assert!(cfg.extra.is_empty());
        assert_eq!(cfg.limits, ResourceLimits::strict());
    }

    #[test]
    fn codec_error_displays_every_variant() {
        // `Display` is what the CLI and the FFI turn these into messages, so
        // each variant is pinned rather than left to a wildcard.
        use core::fmt::Write as _;
        let mut out = String::new();
        let cases: [(CodecError, &str); 9] = [
            (CodecError::MalformedPacket, "malformed packet"),
            (CodecError::NeedMoreInput, "need more input"),
            (CodecError::Cancelled, "cancelled"),
            (
                CodecError::ResourceLimit(ResourceLimitKind::PacketSize),
                "resource limit: PacketSize",
            ),

    struct StubFactory {
        id: [u8; 4],
        name: &'static str,
    }

    impl StubFactory {
        fn new(id: &[u8; 4], name: &'static str) -> Self {
            StubFactory { id: *id, name }
        }
        fn info(&self) -> CodecInfo {
            CodecInfo {
                name: self.name,
                codec_id: self.id,
                version: 1,
                capabilities: Capabilities::default(),
                supported_formats: Vec::new(),
            }
        }
    }

    impl DynDecoderFactory for StubFactory {
        fn codec_id(&self) -> &[u8; 4] {
            &self.id
        }
        fn info(&self) -> CodecInfo {
            self.info()
        }
        fn build(&self, _cfg: &CodecConfig) -> CodecResult<Box<dyn DynDecoder>> {
            unimplemented!("not exercised by the registry lookup tests")
        }
    }

    impl DynEncoderFactory for StubFactory {
        fn codec_id(&self) -> &[u8; 4] {
            &self.id
        }
        fn info(&self) -> CodecInfo {
            self.info()
        }
        fn build(&self, _cfg: &CodecConfig) -> CodecResult<Box<dyn DynEncoder>> {
            unimplemented!("not exercised by the registry lookup tests")
        }
    }

    #[test]
    fn registry_tracks_decoders_and_encoders_separately() {
        let mut reg = Registry::new();
        reg.register_decoder(StubFactory::new(b"DCOD", "Decoder only"));
        reg.register_encoder(StubFactory::new(b"ENCD", "Encoder only"));

        assert_eq!(reg.find_decoder(b"DCOD").map(|f| f.info().name), Some("Decoder only"));
        assert_eq!(reg.find_encoder(b"ENCD").map(|f| f.info().name), Some("Encoder only"));
        // The decoder side does not answer for the encoder's ID, and vice
        // versa: a decode-only build must not appear to offer an encoder.
        assert!(reg.find_decoder(b"ENCD").is_none());
        assert!(reg.find_encoder(b"DCOD").is_none());
        assert_eq!(reg.decoder_ids().count(), 1);
        assert_eq!(reg.encoder_ids().count(), 1);
    }

    #[test]
    fn registry_ids_list_every_registration_in_order() {
        let mut reg = Registry::new();
        reg.register_decoder(StubFactory::new(b"AAAA", "A"));
        reg.register_decoder(StubFactory::new(b"BBBB", "B"));
        let ids: Vec<[u8; 4]> = reg.decoder_ids().map(|i| *i).collect();
        assert_eq!(ids, alloc::vec![*b"AAAA", *b"BBBB"]);
    }

    #[test]
    fn a_second_registration_for_one_id_is_shadowed_not_rejected() {
        // `register_*` returns `()`, so a duplicate cannot be reported. The
        // lookup takes the first registration, which means registration
        // order decides — pinned so the behaviour is a choice, not an
        // accident of `Vec::iter().find`.
        let mut reg = Registry::new();
        reg.register_decoder(StubFactory::new(b"DUP1", "First"));
        reg.register_decoder(StubFactory::new(b"DUP1", "Second"));
        assert_eq!(reg.find_decoder(b"DUP1").map(|f| f.info().name), Some("First"));
        // Both are still listed, so `decoder_ids().count()` is not a
        // distinct-codec count.
        assert_eq!(reg.decoder_ids().count(), 2);
    }

    #[test]
    fn an_empty_registry_finds_nothing() {
        let reg = Registry::new();
        assert!(reg.find_decoder(b"ANY!").is_none());
        assert!(reg.find_encoder(b"ANY!").is_none());
        assert_eq!(reg.decoder_ids().count(), 0);
        assert_eq!(reg.encoder_ids().count(), 0);
    }

    #[test]
    fn registry_debug_reports_counts_not_contents() {
        // The `Debug` impl is hand-written precisely so it does not dump
        // every factory; pinned so a derived `Debug` cannot leak them in.
        let mut reg = Registry::new();
        reg.register_decoder(StubFactory::new(b"AAAA", "A"));
        reg.register_encoder(StubFactory::new(b"BBBB", "B"));
        let text = alloc::format!("{reg:?}");
        assert!(text.contains("decoders"), "{text}");
        assert!(text.contains("encoders"), "{text}");
        assert!(!text.contains("AAAA"), "{text}");
    }

            (CodecError::Unsupported("8x8 DCT"), "unsupported: 8x8 DCT"),
            (CodecError::InvalidConfig("qp"), "invalid config: qp"),
            (CodecError::Internal, "internal error"),
            (CodecError::Bitstream("bad huffman"), "bitstream: bad huffman"),
            (
                CodecError::Downstream("disk full"),
                "downstream: disk full",
            ),
        ];
        for (err, want) in cases {
            out.clear();
            write!(out, "{err}").expect("write to String");
            assert_eq!(out, want);
        }
    }

    #[cfg(feature = "std")]
    #[test]
    fn codec_error_is_a_std_error() {
        fn as_std(e: &dyn std::error::Error) -> String {
            e.to_string()
        }
        let e = CodecError::MalformedPacket;
        assert_eq!(as_std(&e), "malformed packet");
        // `source()` is the error-chain hook; none of these carry a cause.
        assert!(std::error::Error::source(&e).is_none());
    }

            tpt_media_core::Timestamp::zero(tpt_media_core::Timebase::MILLISECONDS),
        );
        p.data = alloc::vec![0u8; (limits.max_packet_bytes as usize) + 1];
        let called = Cell::new(false);
        let out: CodecResult<()> = guard.check(&p, || {
            called.set(true);
            Ok(())
        });
        assert!(out.is_err());
        assert!(!called.get(), "closure ran despite the limit firing");
    }

    #[test]
    fn packet_guard_propagates_a_closure_error_unchanged() {
        // The guard must not rewrite what the decode itself reported.
        let limits = ResourceLimits::strict();
        let guard = PacketGuard::new(&limits);
        let p = Packet::empty(
            0,
            tpt_media_core::Timestamp::zero(tpt_media_core::Timebase::MILLISECONDS),
        );
        let out: CodecResult<u32> = guard.check(&p, || Err(CodecError::MalformedPacket));
        assert_eq!(out.unwrap_err(), CodecError::MalformedPacket);
    }

}
