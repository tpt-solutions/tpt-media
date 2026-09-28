//! `tpt-media-sys` — the stable C ABI for the `tpt-media` codec
//! abstraction (todo.md §46).
//!
//! The FFI exposes the **stable media abstraction**, not internal
//! implementation details: a C caller sees opaque handles for packets,
//! audio frames, encoders, and decoders; an abstract
//! `tpt_media_config` (sample rate, channels, quality 0–100) rather
//! than per-codec parameter structs; and integer error codes rather
//! than Rust error types. Underneath, the same codec objects the CLI
//! drives do the work, so the FFI tracks the native codecs (Cinder,
//! Aura, Reed — the audio family; the video codecs gain the FFI with
//! their own abstraction slice).
//!
//! # ABI stability
//!
//! `tpt_abi_version()` reports the ABI revision of the loaded library
//! (`tpt_media_sys::ABI_VERSION`, currently 1). The rule, matching
//! §46: *adding* functions or error codes is compatible (the header
//! grows, the version holds); *changing* a signature, an error code's
//! meaning, a struct layout, or removing anything bumps the version.
//! The hand-written header `include/tpt_media.h` is kept in sync with
//! this file by `tests/round_trip.rs::header_matches_the_abi`.
//!
//! # Ownership
//!
//! Ownership is explicit and mirrored on the C side:
//!
//! - Every function that allocates a handle returns it through a
//!   `tpt_xxx**` out-parameter; the caller owns the handle and must
//!   release it with the matching `tpt_xxx_free`.
//! - `tpt_packet_new` **copies** the caller's payload bytes; the
//!   caller keeps ownership of its own buffer.
//! - `tpt_frame_samples` returns a pointer into the frame's owned
//!   buffer, valid until `tpt_frame_free`.
//! - Handles passed as `const` are only read; handles passed as
//!   mutable are used only within the call.
//! - A NULL handle is never dereferenced: every function validates
//!   its arguments and returns `TPT_ERR_NULL_ARGUMENT` instead.
//!
//! # Thread safety
//!
//! Every handle is exclusively owned, in the Rust `&mut` sense: two
//! threads must never call into the *same* handle concurrently, even
//! for nominally read-only accessors. Distinct handles may be used
//! concurrently from distinct threads. Handles are not guaranteed
//! `Send`: create and use a handle on one thread (the underlying
//! codec objects make no `Send` promise in v0.1).
//!
//! # Panics
//!
//! No Rust panic may unwind across this ABI (it is undefined behavior
//! in `extern "C"`). Every exported function body runs under
//! `catch_unwind`; a caught panic becomes `TPT_ERR_PANIC`. Argument
//! validation happens before any construction, so invalid input is an
//! error code, not a panic.
//!
//! # `unsafe` policy
//!
//! This crate is the one place in the workspace (besides delegated
//! SIMD in `tpt-dsp`) where `unsafe` is permitted — CONTRIBUTING §4.
//! Every `unsafe` block implements one of the documented caller
//! requirements above; the `// SAFETY:` comment on each states which.

use std::panic::{catch_unwind, AssertUnwindSafe};

use tpt_audio_aura::{
    AuraConfig, AuraDecoder, AuraDynDecoder, AuraDynEncoder, AuraEncoder, AuraMode,
};
use tpt_audio_cinder::{
    CinderDecoder, CinderDynDecoder, CinderDynEncoder, CinderEncoder, CinderEncoderConfig,
};
use tpt_audio_reed::{ReedConfig, ReedDecoder, ReedDynDecoder, ReedDynEncoder, ReedEncoder};
use tpt_media_codec::{CodecConfig, CodecError, DynDecoder, DynEncoder};
use tpt_media_core::{
    AudioFrame, ChannelLayout, CodecId, Frame, Packet, PacketFlags, SampleFormat, Timebase,
    Timestamp,
};

/// ABI revision of this library. See the crate docs' stability rule.
pub const ABI_VERSION: u32 = 1;

/// Crate version as a NUL-terminated string (diagnostics).
pub const VERSION_STRING: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

/// Error codes returned by every exported function.
///
/// Codes are ABI: their values and meanings never change within an
/// ABI version. Nonzero is always an error; `TPT_OK` is `0`.
#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TptError {
    /// Success.
    Ok = 0,
    /// A required pointer argument was NULL.
    NullArgument = -1,
    /// An argument was present but nonsensical (bad sample rate,
    /// channels other than 1, quality over 100, frame of the wrong
    /// block size, non-positive timebase component, ...).
    InvalidArgument = -2,
    /// The codec id (or packet bitstream version) is not supported by
    /// this build.
    Unsupported = -3,
    /// A packet was malformed or truncated.
    MalformedPacket = -4,
    /// The decoder requires more input before producing a frame.
    NeedMoreInput = -5,
    /// A configured resource limit was hit.
    ResourceLimit = -6,
    /// The configuration was rejected by the codec.
    InvalidConfig = -7,
    /// The operation was cancelled.
    Cancelled = -8,
    /// Internal invariant violated (a bug in the codec layer).
    Internal = -9,
    /// A Rust panic was caught at the ABI boundary. Process state is
    /// otherwise untouched, but the handle involved should be
    /// considered poisoned and freed.
    Panic = -10,
}

impl TptError {
    /// The ABI error code for this value (what the C side compares
    /// against the `enum tpt_error` constants).
    pub fn code(self) -> i32 {
        self as i32
    }
}

fn map_codec_err(e: &CodecError) -> TptError {
    match e {
        CodecError::MalformedPacket => TptError::MalformedPacket,
        CodecError::NeedMoreInput => TptError::NeedMoreInput,
        CodecError::Cancelled => TptError::Cancelled,
        CodecError::ResourceLimit(_) => TptError::ResourceLimit,
        CodecError::Unsupported(_) => TptError::Unsupported,
        CodecError::InvalidConfig(_) => TptError::InvalidConfig,
        // `Bitstream`/`Downstream` carry a `String` the ABI cannot
        // return in v0.1; they mean the codec layer hit an unexpected
        // state, which is what INTERNAL says.
        CodecError::Bitstream(_) | CodecError::Downstream(_) | CodecError::Internal => {
            TptError::Internal
        }
    }
}

/// Run `f` inside a panic guard: a caught panic becomes
/// [`TptError::Panic`] and never crosses the ABI.
fn guard<T>(f: impl FnOnce() -> Result<T, TptError>) -> Result<T, TptError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(Err(TptError::Panic))
}

/// Opaque decoder handle. Owns a boxed codec decoder.
pub struct TptDecoder {
    inner: Box<dyn DynDecoder>,
}

/// Opaque encoder handle. Owns a boxed codec encoder.
pub struct TptEncoder {
    inner: Box<dyn DynEncoder>,
    codec_id: CodecId,
}

/// Opaque audio frame handle. Owns an [`AudioFrame`] (S16 interleaved
/// samples — the native audio family's only coded sample format).
pub struct TptFrame {
    inner: AudioFrame,
}

/// Opaque packet handle. Owns a [`Packet`] (owned payload bytes).
pub struct TptPacket {
    inner: Packet,
}

/// The abstract per-codec configuration, mirroring the CLI's
/// `--quality` so a C caller never touches per-codec parameter
/// structs.
///
/// * `sample_rate` — Hz. 44.1/48 kHz for Cinder/Aura, 8/16 kHz for
///   Reed (each codec rejects others with `TPT_ERR_INVALID_CONFIG`).
/// * `channels` — must be 1: the whole native audio family is S16
///   mono today (`TPT_ERR_INVALID_ARGUMENT` otherwise).
/// * `quality` — 0..=100, mapped exactly as the CLI maps `--quality`:
///   Cinder per-coefficient bit depth (`4 + q·8/100`, i.e. 4–12
///   bits), Reed LSF bit depth (`4 + q·4/100`), ignored by Aura
///   (lossless).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TptMediaConfig {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count; must be 1 in this ABI version.
    pub channels: u16,
    /// Quality, 0..=100.
    pub quality: u8,
}

/// Which native audio codec a packed codec id names.
enum CodecKind {
    Cinder,
    Aura,
    Reed,
}

fn classify(id: CodecId) -> Option<CodecKind> {
    match id.as_bytes() {
        [b'C', b'I', b'N', b'D'] => Some(CodecKind::Cinder),
        [b'A', b'U', b'R', b'A'] => Some(CodecKind::Aura),
        [b'R', b'E', b'E', b'D'] => Some(CodecKind::Reed),
        _ => None,
    }
}

fn quality_to_bits(q: u8) -> u8 {
    4 + (u16::from(q) * 8 / 100) as u8
}

fn quality_to_lsf_bits(q: u8) -> u8 {
    4 + (u16::from(q) * 4 / 100) as u8
}

/// Samples per encode block for `codec` at `rate`, or `None` if the
/// codec is unknown.
///
/// The native audio encoders consume exactly one block per call
/// (Cinder's 2×256-sample MDCT window, Aura's 1024-sample block,
/// Reed's 20 ms frame); frames of any other length are rejected with
/// `TPT_ERR_INVALID_ARGUMENT`, and this function is how a caller
/// sizes them.
pub fn block_size(id: CodecId, rate: u32) -> Option<usize> {
    match classify(id)? {
        CodecKind::Cinder => Some(2 * 256),
        CodecKind::Aura => Some(usize::from(AuraConfig::default().block_size)),
        CodecKind::Reed => Some(rate as usize / 50),
    }
}

fn build_encoder(id: CodecId, cfg: &TptMediaConfig) -> Result<Box<dyn DynEncoder>, TptError> {
    let fw = CodecConfig::default();
    match classify(id).ok_or(TptError::Unsupported)? {
        CodecKind::Cinder => {
            let c = CinderEncoderConfig {
                sample_rate: cfg.sample_rate,
                bits_per_coef: quality_to_bits(cfg.quality),
                ..CinderEncoderConfig::default()
            };
            let enc = CinderEncoder::new(fw, c).map_err(|e| map_codec_err(&e))?;
            Ok(Box::new(CinderDynEncoder::new(enc)))
        }
        CodecKind::Aura => {
            let c = AuraConfig {
                mode: AuraMode::Lossless,
                sample_rate: cfg.sample_rate,
                ..AuraConfig::default()
            };
            let enc = AuraEncoder::new(fw, c).map_err(|e| map_codec_err(&e))?;
            Ok(Box::new(AuraDynEncoder::new(enc)))
        }
        CodecKind::Reed => {
            let c = ReedConfig {
                sample_rate: cfg.sample_rate,
                lsf_bits: quality_to_lsf_bits(cfg.quality),
                ..ReedConfig::default()
            };
            let enc = ReedEncoder::new(fw, c).map_err(|e| map_codec_err(&e))?;
            Ok(Box::new(ReedDynEncoder::new(enc)))
        }
    }
}

fn build_decoder(id: CodecId, cfg: &TptMediaConfig) -> Result<Box<dyn DynDecoder>, TptError> {
    // The decoder sees the same quality→parameter mapping as the
    // encoder, so a pair constructed from one config round-trips.
    let fw = CodecConfig::default();
    match classify(id).ok_or(TptError::Unsupported)? {
        CodecKind::Cinder => {
            let c = CinderEncoderConfig {
                sample_rate: cfg.sample_rate,
                bits_per_coef: quality_to_bits(cfg.quality),
                ..CinderEncoderConfig::default()
            };
            let dec = CinderDecoder::new(fw, c).map_err(|e| map_codec_err(&e))?;
            Ok(Box::new(CinderDynDecoder::new(dec)))
        }
        CodecKind::Aura => {
            let c = AuraConfig {
                sample_rate: cfg.sample_rate,
                ..AuraConfig::default()
            };
            let dec = AuraDecoder::new(fw, c).map_err(|e| map_codec_err(&e))?;
            Ok(Box::new(AuraDynDecoder::new(dec)))
        }
        CodecKind::Reed => {
            let c = ReedConfig {
                sample_rate: cfg.sample_rate,
                lsf_bits: quality_to_lsf_bits(cfg.quality),
                ..ReedConfig::default()
            };
            let dec = ReedDecoder::new(fw, c).map_err(|e| map_codec_err(&e))?;
            Ok(Box::new(ReedDynDecoder::new(dec)))
        }
    }
}

fn validate_config(cfg: &TptMediaConfig) -> Result<(), TptError> {
    if cfg.sample_rate == 0 || cfg.channels != 1 || cfg.quality > 100 {
        return Err(TptError::InvalidArgument);
    }
    Ok(())
}

fn frame_from_parts(
    sample_rate: u32,
    channels: u16,
    sample_count: u32,
    pts_ticks: i64,
    tb_num: u32,
    tb_den: u32,
) -> Result<TptFrame, TptError> {
    if sample_rate == 0 || sample_count == 0 || channels != 1 || tb_num == 0 || tb_den == 0 {
        return Err(TptError::InvalidArgument);
    }
    Ok(TptFrame {
        inner: AudioFrame {
            frame: Frame {
                pts: Timestamp {
                    ticks: pts_ticks,
                    timebase: Timebase::new(tb_num, tb_den),
                },
                duration: None,
                side_data: Vec::new(),
            },
            // S16 mono: 2 bytes per sample, zero-filled.
            samples: vec![0u8; sample_count as usize * 2],
            sample_count,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate,
        },
    })
}

fn packet_from_parts(
    data: &[u8],
    pts_ticks: i64,
    dts_ticks: i64,
    tb_num: u32,
    tb_den: u32,
    keyframe: i32,
) -> Result<TptPacket, TptError> {
    if tb_num == 0 || tb_den == 0 {
        return Err(TptError::InvalidArgument);
    }
    let tb = Timebase::new(tb_num, tb_den);
    Ok(TptPacket {
        inner: Packet {
            data: data.to_vec(),
            pts: Timestamp {
                ticks: pts_ticks,
                timebase: tb,
            },
            dts: Timestamp {
                ticks: dts_ticks,
                timebase: tb,
            },
            flags: if keyframe != 0 {
                PacketFlags::KEYFRAME
            } else {
                PacketFlags::empty()
            },
            track_id: 0,
            duration: None,
        },
    })
}

/// Pull one audio frame out of the type-erased output of a decode or
/// flush call, attaching `pts` (the `DynFrame` carries no timestamps —
/// the CLI associates the packet's PTS with its output the same way).
///
/// `Ok(None)` means the call legitimately produced nothing.
fn one_audio_frame(
    mut frames: Vec<tpt_media_codec::DynFrame>,
    pts: Timestamp,
) -> Result<Option<Box<TptFrame>>, TptError> {
    match frames.len() {
        0 => Ok(None),
        1 => match frames.remove(0) {
            tpt_media_codec::DynFrame::Audio {
                sample_format,
                channel_layout,
                sample_rate,
                sample_count,
                samples,
            } => Ok(Some(Box::new(TptFrame {
                inner: AudioFrame {
                    frame: Frame {
                        pts,
                        duration: None,
                        side_data: Vec::new(),
                    },
                    samples,
                    sample_count,
                    format: sample_format,
                    channel_layout,
                    sample_rate,
                },
            }))),
            tpt_media_codec::DynFrame::Video { .. } => Err(TptError::Unsupported),
        },
        _ => Err(TptError::Internal),
    }
}

// ---------------------------------------------------------------------------
// Exported ABI
// ---------------------------------------------------------------------------

/// The ABI revision of this library.
///
/// # Safety
///
/// Always safe: takes no arguments, touches no handles.
#[no_mangle]
pub extern "C" fn tpt_abi_version() -> u32 {
    ABI_VERSION
}

/// The crate version as a NUL-terminated static string (diagnostics).
///
/// # Safety
///
/// Always safe: the returned pointer is static and NUL-terminated.
#[no_mangle]
pub extern "C" fn tpt_version_string() -> *const std::os::raw::c_char {
    VERSION_STRING.as_ptr() as *const std::os::raw::c_char
}

/// Pack four ASCII bytes into the codec id every constructor takes.
///
/// # Safety
///
/// Always safe: pure arithmetic.
#[no_mangle]
pub extern "C" fn tpt_codec_id(a: u8, b: u8, c: u8, d: u8) -> u32 {
    u32::from_be_bytes([a, b, c, d])
}

/// Allocate a new encoder for `codec_id` (see `tpt_codec_id`).
///
/// `out` receives the handle on success; the caller owns it and must
/// `tpt_encoder_free` it.
///
/// # Safety
///
/// `config` must point to a valid `tpt_media_config`; `out` must point
/// to writable memory. Neither is dereferenced if NULL (the call
/// returns `TPT_ERR_NULL_ARGUMENT`).
#[no_mangle]
pub unsafe extern "C" fn tpt_encoder_new(
    codec_id: u32,
    config: *const TptMediaConfig,
    out: *mut *mut TptEncoder,
) -> i32 {
    guard(|| {
        if config.is_null() || out.is_null() {
            return Err(TptError::NullArgument);
        }
        let cfg = *config;
        validate_config(&cfg)?;
        let id = CodecId::new(codec_id.to_be_bytes());
        let inner = build_encoder(id, &cfg)?;
        // SAFETY: `out` was checked non-NULL above; writing through it
        // is this function's contract with the caller.
        unsafe {
            *out = Box::into_raw(Box::new(TptEncoder {
                inner,
                codec_id: id,
            }))
        };
        Ok(())
    })
    .map_or_else(|e| e.code(), |_| TptError::Ok.code())
}

/// Encode one audio frame into at most one packet.
///
/// `out` receives a newly allocated packet handle (caller frees with
/// `tpt_packet_free`), or NULL when the encoder produced nothing. The
/// frame must carry exactly this codec's block size — see
/// `tpt_encoder_block_size` — and is only read, never stored.
///
/// # Safety
///
/// `enc` must be a valid handle with exclusive access (no concurrent
/// use from another thread); `frame` must be a valid handle; `out`
/// must be writable. NULL arguments return `TPT_ERR_NULL_ARGUMENT`.
#[no_mangle]
pub unsafe extern "C" fn tpt_encoder_encode(
    enc: *mut TptEncoder,
    frame: *const TptFrame,
    out: *mut *mut TptPacket,
) -> i32 {
    guard(|| {
        if enc.is_null() || frame.is_null() || out.is_null() {
            return Err(TptError::NullArgument);
        }
        // SAFETY: `enc`/`frame` are caller-owned handles checked
        // non-NULL above; the caller's thread-safety obligation
        // (crate docs) guarantees exclusive access for the call.
        let (enc_ref, frame_ref) = unsafe { (&mut *enc, &*frame) };
        let f = &frame_ref.inner;
        let expected = block_size(enc_ref.codec_id, f.sample_rate).ok_or(TptError::Unsupported)?;
        if f.samples.len() != expected * 2 {
            return Err(TptError::InvalidArgument);
        }
        let dyn_frame = tpt_media_codec::DynFrame::Audio {
            sample_format: f.format,
            channel_layout: f.channel_layout,
            sample_rate: f.sample_rate,
            sample_count: f.sample_count,
            samples: f.samples.clone(),
        };
        let mut packets = enc_ref
            .inner
            .encode(&dyn_frame)
            .map_err(|e| map_codec_err(&e))?;
        let pkt = match packets.len() {
            0 => None,
            // The native audio encoders emit exactly one packet per
            // frame; anything else is a bug, not an ABI state.
            1 => {
                let mut p = packets.remove(0);
                // The type-erased `DynFrame` carries no timestamps (a
                // codec-layer gap, not an ABI one), so the boundary
                // owns PTS association: the packet carries the frame's
                // PTS, the same way `tpt_decoder_decode` hands the
                // packet's PTS to its output frame.
                p.pts = f.frame.pts;
                p.dts = f.frame.pts;
                Some(Box::new(TptPacket { inner: p }))
            }
            _ => return Err(TptError::Internal),
        };
        // SAFETY: `out` checked non-NULL above.
        unsafe {
            *out = pkt.map_or(std::ptr::null_mut(), Box::into_raw);
        }
        Ok(())
    })
    .map_or_else(|e| e.code(), |_| TptError::Ok.code())
}

/// The number of samples per channel one encode call consumes for
/// this codec and sample rate (Cinder 512, Aura 1024, Reed rate/50),
/// or 0 for an unknown codec id.
///
/// # Safety
///
/// Always safe: takes no handles.
#[no_mangle]
pub extern "C" fn tpt_encoder_block_size(codec_id: u32, sample_rate: u32) -> u32 {
    block_size(CodecId::new(codec_id.to_be_bytes()), sample_rate)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

/// Finish the stream: emit any packet the encoder still buffers.
///
/// `out` receives NULL when nothing remains. After this call the
/// encoder should be freed; feeding it further frames is not
/// supported in this ABI version.
///
/// # Safety
///
/// As `tpt_encoder_encode`: valid, exclusively-owned `enc`; writable
/// `out`.
#[no_mangle]
pub unsafe extern "C" fn tpt_encoder_flush(enc: *mut TptEncoder, out: *mut *mut TptPacket) -> i32 {
    guard(|| {
        if enc.is_null() || out.is_null() {
            return Err(TptError::NullArgument);
        }
        // SAFETY: non-NULL, exclusively-owned handle per the docs.
        let enc_ref = unsafe { &mut *enc };
        let mut packets = enc_ref.inner.flush().map_err(|e| map_codec_err(&e))?;
        let pkt = match packets.len() {
            0 => None,
            1 => Some(Box::new(TptPacket {
                inner: packets.remove(0),
            })),
            _ => return Err(TptError::Internal),
        };
        // SAFETY: `out` checked non-NULL above.
        unsafe {
            *out = pkt.map_or(std::ptr::null_mut(), Box::into_raw);
        }
        Ok(())
    })
    .map_or_else(|e| e.code(), |_| TptError::Ok.code())
}

/// Release an encoder handle. NULL is allowed and does nothing.
///
/// # Safety
///
/// `enc` must be NULL or a handle handed out by `tpt_encoder_new` that
/// has not been freed yet and is not concurrently in use.
#[no_mangle]
pub unsafe extern "C" fn tpt_encoder_free(enc: *mut TptEncoder) {
    if !enc.is_null() {
        drop(unsafe { Box::from_raw(enc) });
    }
}

/// Allocate a new decoder for `codec_id`.
///
/// The decoder sees the same quality→parameter mapping as
/// `tpt_encoder_new`, so handles constructed from one config
/// round-trip.
///
/// # Safety
///
/// As `tpt_encoder_new`.
#[no_mangle]
pub unsafe extern "C" fn tpt_decoder_new(
    codec_id: u32,
    config: *const TptMediaConfig,
    out: *mut *mut TptDecoder,
) -> i32 {
    guard(|| {
        if config.is_null() || out.is_null() {
            return Err(TptError::NullArgument);
        }
        let cfg = *config;
        validate_config(&cfg)?;
        let id = CodecId::new(codec_id.to_be_bytes());
        let inner = build_decoder(id, &cfg)?;
        // SAFETY: `out` checked non-NULL above.
        unsafe { *out = Box::into_raw(Box::new(TptDecoder { inner })) };
        Ok(())
    })
    .map_or_else(|e| e.code(), |_| TptError::Ok.code())
}

/// Decode one packet into at most one audio frame.
///
/// `out` receives a newly allocated frame handle (caller frees with
/// `tpt_frame_free`), or NULL when this packet produced no frame. The
/// returned frame carries the packet's PTS.
///
/// # Safety
///
/// As `tpt_encoder_encode`: valid, exclusively-owned handles; writable
/// `out`.
#[no_mangle]
pub unsafe extern "C" fn tpt_decoder_decode(
    dec: *mut TptDecoder,
    pkt: *const TptPacket,
    out: *mut *mut TptFrame,
) -> i32 {
    guard(|| {
        if dec.is_null() || pkt.is_null() || out.is_null() {
            return Err(TptError::NullArgument);
        }
        // SAFETY: non-NULL, exclusively-owned handles.
        let (dec_ref, pkt_ref) = unsafe { (&mut *dec, &*pkt) };
        let pts = pkt_ref.inner.pts;
        let frames = dec_ref
            .inner
            .decode(&pkt_ref.inner)
            .map_err(|e| map_codec_err(&e))?;
        let frame = one_audio_frame(frames, pts)?;
        // SAFETY: `out` checked non-NULL above.
        unsafe {
            *out = frame.map_or(std::ptr::null_mut(), Box::into_raw);
        }
        Ok(())
    })
    .map_or_else(|e| e.code(), |_| TptError::Ok.code())
}

/// Finish the stream: emit any frame the decoder still buffers.
///
/// A flushed frame carries PTS 0 on its own sample-rate timebase —
/// there is no packet to associate it with.
///
/// # Safety
///
/// As `tpt_decoder_decode`.
#[no_mangle]
pub unsafe extern "C" fn tpt_decoder_flush(dec: *mut TptDecoder, out: *mut *mut TptFrame) -> i32 {
    guard(|| {
        if dec.is_null() || out.is_null() {
            return Err(TptError::NullArgument);
        }
        // SAFETY: non-NULL, exclusively-owned handle.
        let dec_ref = unsafe { &mut *dec };
        let frames = dec_ref.inner.flush().map_err(|e| map_codec_err(&e))?;
        let pts = Timestamp {
            ticks: 0,
            timebase: Timebase::new(1, 1),
        };
        let frame = one_audio_frame(frames, pts)?;
        // SAFETY: `out` checked non-NULL above.
        unsafe {
            *out = frame.map_or(std::ptr::null_mut(), Box::into_raw);
        }
        Ok(())
    })
    .map_or_else(|e| e.code(), |_| TptError::Ok.code())
}

/// Release a decoder handle. NULL is allowed and does nothing.
///
/// # Safety
///
/// As `tpt_encoder_free`.
#[no_mangle]
pub unsafe extern "C" fn tpt_decoder_free(dec: *mut TptDecoder) {
    if !dec.is_null() {
        drop(unsafe { Box::from_raw(dec) });
    }
}

/// Allocate an S16 mono audio frame of `sample_count` samples,
/// zero-filled, at `pts_ticks` on the `tb_num/tb_den` timebase.
///
/// Fill the buffer through `tpt_frame_samples`.
///
/// # Safety
///
/// `out` must be writable; validated arguments are otherwise safe.
#[no_mangle]
pub unsafe extern "C" fn tpt_frame_new(
    sample_rate: u32,
    channels: u16,
    sample_count: u32,
    pts_ticks: i64,
    tb_num: u32,
    tb_den: u32,
    out: *mut *mut TptFrame,
) -> i32 {
    guard(|| {
        if out.is_null() {
            return Err(TptError::NullArgument);
        }
        let f = frame_from_parts(
            sample_rate,
            channels,
            sample_count,
            pts_ticks,
            tb_num,
            tb_den,
        )?;
        // SAFETY: `out` checked non-NULL above.
        unsafe { *out = Box::into_raw(Box::new(f)) };
        Ok(())
    })
    .map_or_else(|e| e.code(), |_| TptError::Ok.code())
}

/// Writable pointer to the frame's S16 interleaved sample bytes
/// (`sample_count * channels * 2` bytes, valid until free).
///
/// # Safety
///
/// `frame` must be a valid handle; the returned pointer must not
/// outlive it and must not be written from two threads at once.
#[no_mangle]
pub unsafe extern "C" fn tpt_frame_samples(frame: *mut TptFrame) -> *mut u8 {
    if frame.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: valid, exclusively-owned handle; the returned pointer
    // aliases the frame's own buffer, which the caller may not touch
    // concurrently (crate docs).
    unsafe { (*frame).inner.samples.as_mut_ptr() }
}

/// Read-only pointer to the frame's sample bytes.
///
/// # Safety
///
/// `frame` must be a valid handle; the pointer is valid until free.
#[no_mangle]
pub unsafe extern "C" fn tpt_frame_samples_const(frame: *const TptFrame) -> *const u8 {
    if frame.is_null() {
        return std::ptr::null();
    }
    // SAFETY: read-only projection of a valid handle.
    unsafe { (*frame).inner.samples.as_ptr() }
}

/// Samples per channel in this frame.
///
/// # Safety
///
/// `frame` must be a valid handle or NULL (NULL yields 0).
#[no_mangle]
pub unsafe extern "C" fn tpt_frame_sample_count(frame: *const TptFrame) -> u32 {
    if frame.is_null() {
        return 0;
    }
    // SAFETY: read-only projection of a valid handle.
    unsafe { (*frame).inner.sample_count }
}

/// The frame's sample rate in Hz.
///
/// # Safety
///
/// `frame` must be a valid handle or NULL (NULL yields 0).
#[no_mangle]
pub unsafe extern "C" fn tpt_frame_sample_rate(frame: *const TptFrame) -> u32 {
    if frame.is_null() {
        return 0;
    }
    // SAFETY: read-only projection of a valid handle.
    unsafe { (*frame).inner.sample_rate }
}

/// The frame's PTS in timebase ticks.
///
/// # Safety
///
/// `frame` must be a valid handle or NULL (NULL yields 0).
#[no_mangle]
pub unsafe extern "C" fn tpt_frame_pts(frame: *const TptFrame) -> i64 {
    if frame.is_null() {
        return 0;
    }
    // SAFETY: read-only projection of a valid handle.
    unsafe { (*frame).inner.frame.pts.ticks }
}

/// Release a frame handle. NULL is allowed and does nothing.
///
/// # Safety
///
/// `frame` must be NULL or a handle handed out by this ABI that has
/// not been freed and is not concurrently in use.
#[no_mangle]
pub unsafe extern "C" fn tpt_frame_free(frame: *mut TptFrame) {
    if !frame.is_null() {
        drop(unsafe { Box::from_raw(frame) });
    }
}

/// Allocate a packet by **copying** `len` bytes from `data` (the
/// caller keeps its buffer), with `pts`/`dts` on `tb_num/tb_den`.
///
/// # Safety
///
/// `data` must point to `len` readable bytes (or be NULL only when
/// `len` is 0); `out` must be writable.
#[no_mangle]
pub unsafe extern "C" fn tpt_packet_new(
    data: *const u8,
    len: u32,
    pts_ticks: i64,
    dts_ticks: i64,
    tb_num: u32,
    tb_den: u32,
    keyframe: i32,
    out: *mut *mut TptPacket,
) -> i32 {
    guard(|| {
        if out.is_null() || (data.is_null() && len != 0) {
            return Err(TptError::NullArgument);
        }
        // SAFETY: `data` points to `len` readable bytes per the
        // caller contract checked above.
        let bytes = unsafe { std::slice::from_raw_parts(data, len as usize) };
        let p = packet_from_parts(bytes, pts_ticks, dts_ticks, tb_num, tb_den, keyframe)?;
        // SAFETY: `out` checked non-NULL above.
        unsafe { *out = Box::into_raw(Box::new(p)) };
        Ok(())
    })
    .map_or_else(|e| e.code(), |_| TptError::Ok.code())
}

/// Read-only pointer to the packet payload (`tpt_packet_len` bytes,
/// valid until free).
///
/// # Safety
///
/// `pkt` must be a valid handle or NULL (NULL yields NULL).
#[no_mangle]
pub unsafe extern "C" fn tpt_packet_data(pkt: *const TptPacket) -> *const u8 {
    if pkt.is_null() {
        return std::ptr::null();
    }
    // SAFETY: read-only projection of a valid handle.
    unsafe { (*pkt).inner.data.as_ptr() }
}

/// Payload length in bytes.
///
/// # Safety
///
/// `pkt` must be a valid handle or NULL (NULL yields 0).
#[no_mangle]
pub unsafe extern "C" fn tpt_packet_len(pkt: *const TptPacket) -> u32 {
    if pkt.is_null() {
        return 0;
    }
    // SAFETY: read-only projection of a valid handle.
    unsafe { (*pkt).inner.data.len() as u32 }
}

/// PTS in timebase ticks.
///
/// # Safety
///
/// `pkt` must be a valid handle or NULL (NULL yields 0).
#[no_mangle]
pub unsafe extern "C" fn tpt_packet_pts(pkt: *const TptPacket) -> i64 {
    if pkt.is_null() {
        return 0;
    }
    // SAFETY: read-only projection of a valid handle.
    unsafe { (*pkt).inner.pts.ticks }
}

/// DTS in timebase ticks.
///
/// # Safety
///
/// `pkt` must be a valid handle or NULL (NULL yields 0).
#[no_mangle]
pub unsafe extern "C" fn tpt_packet_dts(pkt: *const TptPacket) -> i64 {
    if pkt.is_null() {
        return 0;
    }
    // SAFETY: read-only projection of a valid handle.
    unsafe { (*pkt).inner.dts.ticks }
}

/// 1 when the packet is a keyframe, 0 otherwise.
///
/// # Safety
///
/// `pkt` must be a valid handle or NULL (NULL yields 0).
#[no_mangle]
pub unsafe extern "C" fn tpt_packet_is_keyframe(pkt: *const TptPacket) -> i32 {
    if pkt.is_null() {
        return 0;
    }
    // SAFETY: read-only projection of a valid handle.
    unsafe { (*pkt).inner.is_keyframe() as i32 }
}

/// Release a packet handle. NULL is allowed and does nothing.
///
/// # Safety
///
/// `pkt` must be NULL or a handle handed out by this ABI that has not
/// been freed and is not concurrently in use.
#[no_mangle]
pub unsafe extern "C" fn tpt_packet_free(pkt: *mut TptPacket) {
    if !pkt.is_null() {
        drop(unsafe { Box::from_raw(pkt) });
    }
}
