//! Rust-side tests of the exported C ABI (§46).
//!
//! These call the same `#[no_mangle] extern "C"` functions a C caller
//! would, through the same signatures, so signature errors and
//! ownership mistakes surface here. The *header* is validated by
//! `header_matches_the_abi`, and a real C compiler drives the ABI end
//! to end in `tests/c_abi.rs` (when a toolchain exists).

use std::ptr;

use tpt_media_sys::{TptError, TptMediaConfig};

const AURA: u32 = (b'A' as u32) << 24 | (b'U' as u32) << 16 | (b'R' as u32) << 8 | b'A' as u32;
const CINDER: u32 = (b'C' as u32) << 24 | (b'I' as u32) << 16 | (b'N' as u32) << 8 | b'D' as u32;
const REED: u32 = (b'R' as u32) << 24 | (b'E' as u32) << 16 | (b'E' as u32) << 8 | b'D' as u32;

fn cfg(rate: u32) -> TptMediaConfig {
    TptMediaConfig {
        sample_rate: rate,
        channels: 1,
        quality: 80,
    }
}

/// A deterministic S16 sample, small enough that every codec's
/// quantizer keeps it finite.
fn sample(i: usize) -> i16 {
    (((i % 64) as i16) - 32) * 64
}

/// Encode `blocks` frames through the raw ABI and return the packets.
unsafe fn encode_all(codec: u32, rate: u32, blocks: usize) -> Vec<(Vec<u8>, i64)> {
    let mut enc: *mut tpt_media_sys::TptEncoder = ptr::null_mut();
    let rc = tpt_media_sys::tpt_encoder_new(codec, &cfg(rate), &mut enc);
    assert_eq!(rc, TptError::Ok.code(), "encoder_new");

    let block = tpt_media_sys::tpt_encoder_block_size(codec, rate);
    assert!(block > 0, "block_size");

    let mut out = Vec::new();
    for b in 0..blocks {
        let mut frame: *mut tpt_media_sys::TptFrame = ptr::null_mut();
        let rc = tpt_media_sys::tpt_frame_new(rate, 1, block, b as i64, 1, rate, &mut frame);
        assert_eq!(rc, TptError::Ok.code(), "frame_new");
        let samples = tpt_media_sys::tpt_frame_samples(frame);
        for i in 0..block as usize {
            let s = sample(i + b * block as usize).to_le_bytes();
            // SAFETY: samples points at block*2 writable bytes for the
            // lifetime of the frame we just allocated.
            unsafe {
                *samples.add(i * 2) = s[0];
                *samples.add(i * 2 + 1) = s[1];
            }
        }

        let mut pkt: *mut tpt_media_sys::TptPacket = ptr::null_mut();
        let rc = tpt_media_sys::tpt_encoder_encode(enc, frame, &mut pkt);
        assert_eq!(rc, TptError::Ok.code(), "encode block {b}");
        assert!(!pkt.is_null(), "encode must produce a packet");
        let len = tpt_media_sys::tpt_packet_len(pkt);
        let data = tpt_media_sys::tpt_packet_data(pkt);
        let mut bytes = Vec::with_capacity(len as usize);
        for i in 0..len as usize {
            // SAFETY: data points at len readable bytes per the ABI.
            bytes.push(unsafe { *data.add(i) });
        }
        out.push((bytes, tpt_media_sys::tpt_packet_pts(pkt)));

        tpt_media_sys::tpt_frame_free(frame);
        tpt_media_sys::tpt_packet_free(pkt);
    }

    let mut pkt: *mut tpt_media_sys::TptPacket = ptr::null_mut();
    let rc = tpt_media_sys::tpt_encoder_flush(enc, &mut pkt);
    assert_eq!(rc, TptError::Ok.code(), "flush");
    assert!(pkt.is_null(), "audio encoders buffer nothing");
    tpt_media_sys::tpt_encoder_free(enc);
    out
}

/// Decode `packets` (as raw bytes) through the raw ABI.
unsafe fn decode_all(codec: u32, rate: u32, packets: &[(Vec<u8>, i64)]) -> Vec<Vec<i16>> {
    let mut dec: *mut tpt_media_sys::TptDecoder = ptr::null_mut();
    let rc = tpt_media_sys::tpt_decoder_new(codec, &cfg(rate), &mut dec);
    assert_eq!(rc, TptError::Ok.code(), "decoder_new");

    let mut out = Vec::new();
    for (bytes, pts) in packets {
        let mut pkt: *mut tpt_media_sys::TptPacket = ptr::null_mut();
        let rc = tpt_media_sys::tpt_packet_new(
            bytes.as_ptr(),
            bytes.len() as u32,
            *pts,
            *pts,
            1,
            rate,
            1,
            &mut pkt,
        );
        assert_eq!(rc, TptError::Ok.code(), "packet_new");

        let mut frame: *mut tpt_media_sys::TptFrame = ptr::null_mut();
        let rc = tpt_media_sys::tpt_decoder_decode(dec, pkt, &mut frame);
        assert_eq!(rc, TptError::Ok.code(), "decode");
        assert!(!frame.is_null(), "decode must produce a frame");
        assert_eq!(tpt_media_sys::tpt_frame_pts(frame), *pts, "PTS carried");
        assert_eq!(tpt_media_sys::tpt_frame_sample_rate(frame), rate);

        let n = tpt_media_sys::tpt_frame_sample_count(frame) as usize;
        let data = tpt_media_sys::tpt_frame_samples_const(frame);
        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            // SAFETY: data points at n*2 readable bytes per the ABI.
            let lo = unsafe { *data.add(i * 2) };
            let hi = unsafe { *data.add(i * 2 + 1) };
            samples.push(i16::from_le_bytes([lo, hi]));
        }
        out.push(samples);

        tpt_media_sys::tpt_frame_free(frame);
        tpt_media_sys::tpt_packet_free(pkt);
    }

    let mut frame: *mut tpt_media_sys::TptFrame = ptr::null_mut();
    let rc = tpt_media_sys::tpt_decoder_flush(dec, &mut frame);
    assert_eq!(rc, TptError::Ok.code(), "decoder flush");
    assert!(frame.is_null(), "audio decoders buffer nothing");
    tpt_media_sys::tpt_decoder_free(dec);
    out
}

/// The full ABI round trip is bit-exact for the lossless codec.
#[test]
fn aura_lossless_round_trips_bit_exact_through_the_abi() {
    unsafe {
        let packets = encode_all(AURA, 48_000, 4);
        let decoded = decode_all(AURA, 48_000, &packets);
        assert_eq!(decoded.len(), 4);
        for (b, out) in decoded.iter().enumerate() {
            let expected: Vec<i16> = (0..1024usize).map(|i| sample(i + b * 1024)).collect();
            assert_eq!(*out, expected, "block {b} must round-trip exactly");
        }
    }
}

/// The lossy codecs also survive the ABI round trip with their
/// documented properties: right shape, non-silent, finite.
#[test]
fn lossy_codecs_round_trip_through_the_abi() {
    unsafe {
        for (codec, rate) in [(CINDER, 48_000), (REED, 16_000)] {
            let packets = encode_all(codec, rate, 2);
            let decoded = decode_all(codec, rate, &packets);
            assert_eq!(decoded.len(), 2, "{codec}");
            for out in &decoded {
                assert!(
                    out.iter().any(|&s| s != 0),
                    "{codec}: decoded silence from a loud input"
                );
            }
        }
    }
}

/// NULL arguments are error codes, never dereferences.
#[test]
fn null_arguments_are_rejected() {
    unsafe {
        assert_eq!(
            tpt_media_sys::tpt_encoder_new(AURA, ptr::null(), ptr::null_mut()),
            TptError::NullArgument.code()
        );
        assert_eq!(
            tpt_media_sys::tpt_decoder_decode(ptr::null_mut(), ptr::null(), ptr::null_mut()),
            TptError::NullArgument.code()
        );
        assert_eq!(
            tpt_media_sys::tpt_frame_new(48_000, 1, 1024, 0, 1, 48_000, ptr::null_mut()),
            TptError::NullArgument.code()
        );
        // Freeing NULL is a no-op, not a crash.
        tpt_media_sys::tpt_frame_free(ptr::null_mut());
        tpt_media_sys::tpt_packet_free(ptr::null_mut());
        tpt_media_sys::tpt_encoder_free(ptr::null_mut());
        tpt_media_sys::tpt_decoder_free(ptr::null_mut());
    }
}

/// An unknown codec id is UNSUPPORTED, not a panic; a nonsense config
/// is INVALID_ARGUMENT, checked before the codec is ever built.
#[test]
fn bad_inputs_are_error_codes() {
    unsafe {
        let bogus = tpt_media_sys::tpt_codec_id(b'B', b'O', b'G', b'U');
        let mut handle: *mut tpt_media_sys::TptEncoder = ptr::null_mut();
        assert_eq!(
            tpt_media_sys::tpt_encoder_new(bogus, &cfg(48_000), &mut handle),
            TptError::Unsupported.code()
        );
        let bad = TptMediaConfig {
            sample_rate: 48_000,
            channels: 2,
            quality: 50,
        };
        assert_eq!(
            tpt_media_sys::tpt_encoder_new(AURA, &bad, &mut handle),
            TptError::InvalidArgument.code()
        );
        let bad_q = TptMediaConfig {
            sample_rate: 48_000,
            channels: 1,
            quality: 101,
        };
        assert_eq!(
            tpt_media_sys::tpt_encoder_new(AURA, &bad_q, &mut handle),
            TptError::InvalidArgument.code()
        );
        assert_eq!(
            tpt_media_sys::tpt_encoder_block_size(bogus, 48_000),
            0,
            "unknown codec has no block size"
        );
    }
}

/// A frame of the wrong block size is rejected with INVALID_ARGUMENT
/// rather than mis-encoded.
#[test]
fn wrong_block_size_is_invalid_argument() {
    unsafe {
        let mut enc: *mut tpt_media_sys::TptEncoder = ptr::null_mut();
        assert_eq!(
            tpt_media_sys::tpt_encoder_new(AURA, &cfg(48_000), &mut enc),
            TptError::Ok.code()
        );
        let mut frame: *mut tpt_media_sys::TptFrame = ptr::null_mut();
        assert_eq!(
            tpt_media_sys::tpt_frame_new(48_000, 1, 100, 0, 1, 48_000, &mut frame),
            TptError::Ok.code()
        );
        let mut pkt: *mut tpt_media_sys::TptPacket = ptr::null_mut();
        assert_eq!(
            tpt_media_sys::tpt_encoder_encode(enc, frame, &mut pkt),
            TptError::InvalidArgument.code()
        );
        tpt_media_sys::tpt_frame_free(frame);
        tpt_media_sys::tpt_encoder_free(enc);
    }
}

/// The hand-written header and the Rust ABI are the same surface: every
/// `#[no_mangle] extern "C"` function appears in `include/tpt_media.h`,
/// and the header declares nothing the library does not export.
#[test]
fn header_matches_the_abi() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let lib = std::fs::read_to_string(format!("{manifest}/src/lib.rs")).expect("lib.rs");
    let header =
        std::fs::read_to_string(format!("{manifest}/include/tpt_media.h")).expect("header");

    let mut rust_fns: Vec<String> = Vec::new();
    for line in lib.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("pub extern \"C\" fn ") {
            rust_fns.push(rest.split(['(', ':']).next().unwrap().to_string());
        } else if let Some(rest) = line.strip_prefix("pub unsafe extern \"C\" fn ") {
            rust_fns.push(rest.split(['(', ':']).next().unwrap().to_string());
        }
    }
    assert!(
        rust_fns.len() >= 20,
        "sanity: the ABI has grown to {} exports; if intentional, update this test",
        rust_fns.len()
    );

    for name in &rust_fns {
        assert!(
            header.contains(name),
            "header is missing `{name}` — update include/tpt_media.h in the same change"
        );
    }

    // The reverse direction: every `tpt_*` symbol the header declares
    // must exist in the Rust source (catches deleting a function
    // without updating the header).
    for line in header.lines() {
        let line = line.trim_start();
        for token in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if token.starts_with("tpt_")
                && token.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                && token != "tpt_error"
                && token != "tpt_media_config"
                // `#include "tpt_media.h"` splits into this token; it
                // is a file name, not a symbol.
                && token != "tpt_media"
                && !token.ends_with("_t")
            {
                assert!(
                    lib.contains(&format!("fn {token}")),
                    "header declares `{token}` but lib.rs exports no such function"
                );
            }
        }
    }
}
