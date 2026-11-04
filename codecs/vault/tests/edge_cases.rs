//! Edge-case sweep for Vault's public surface: wrong geometry and format,
//! short pixel buffers, empty and truncated packets, invalid configuration,
//! and extreme pixel content. Complements the unit tests and the random
//! sweeps in `property.rs`.

use tpt_media_codec::{CodecConfig, CodecError, Decoder, Encoder};
use tpt_media_core::{Packet, PacketFlags, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_vault::{VaultConfig, VaultDecoder, VaultEncoder};

const W: u32 = 64;
const H: u32 = 64;

fn cfg(format: PixelFormat) -> VaultConfig {
    VaultConfig {
        width: W,
        height: H,
        format,
        ..VaultConfig::default()
    }
}

fn frame(w: u32, h: u32, format: PixelFormat, fill: impl Fn(usize) -> u8) -> VideoFrame {
    let mut f = VideoFrame::alloc(w, h, format);
    for (i, p) in f.pixels.iter_mut().enumerate() {
        *p = fill(i);
    }
    f
}

fn packet(data: Vec<u8>) -> Packet {
    Packet {
        data,
        pts: Timestamp::zero(Timebase::new(1, 30)),
        dts: Timestamp::zero(Timebase::new(1, 30)),
        flags: PacketFlags::KEYFRAME,
        track_id: 0,
        duration: None,
    }
}

#[test]
fn wrong_geometry_or_format_is_refused() {
    let mut enc = VaultEncoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    let wrong_w = frame(W + 16, H, PixelFormat::Gray8, |_| 128);
    assert!(matches!(
        enc.encode(&wrong_w),
        Err(CodecError::InvalidConfig(_))
    ));
    let wrong_h = frame(W, H + 16, PixelFormat::Gray8, |_| 128);
    assert!(matches!(
        enc.encode(&wrong_h),
        Err(CodecError::InvalidConfig(_))
    ));
    let wrong_fmt = frame(W, H, PixelFormat::Yuv420, |_| 128);
    assert!(matches!(
        enc.encode(&wrong_fmt),
        Err(CodecError::Unsupported(_))
    ));
}

#[test]
fn short_pixel_buffer_is_refused_not_a_panic() {
    let mut enc = VaultEncoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    let mut short = frame(W, H, PixelFormat::Gray8, |_| 128);
    short.pixels.truncate(short.pixels.len() - 1);
    assert!(
        enc.encode(&short).is_err(),
        "a short buffer must be refused"
    );
    let mut empty = frame(W, H, PixelFormat::Gray8, |_| 128);
    empty.pixels.clear();
    assert!(
        enc.encode(&empty).is_err(),
        "an empty buffer must be refused"
    );
}

#[test]
fn zero_length_and_truncated_packets_fail_cleanly() {
    let mut enc = VaultEncoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    let f = frame(W, H, PixelFormat::Gray8, |i| (i * 3) as u8);
    let valid = enc
        .encode(&f)
        .unwrap()
        .into_iter()
        .next()
        .expect("one packet");
    let mut dec = VaultDecoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    assert!(
        dec.decode(&packet(Vec::new())).is_err(),
        "empty packet must fail"
    );
    for len in 0..valid.data.len() {
        let mut d = VaultDecoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
        assert!(
            d.decode(&packet(valid.data[..len].to_vec())).is_err(),
            "prefix of length {len} of {} decoded as complete",
            valid.data.len()
        );
    }
    let mut d = VaultDecoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    assert!(d.decode(&valid).is_ok(), "the intact packet decodes");
}

#[test]
fn invalid_configs_are_refused_at_construction() {
    let base = cfg(PixelFormat::Gray8);
    let bad = [
        VaultConfig {
            width: 0,
            ..base.clone()
        },
        VaultConfig {
            height: 0,
            ..base.clone()
        },
        VaultConfig {
            qp: 0,
            ..base.clone()
        },
        VaultConfig {
            qp: 52,
            ..base.clone()
        },
        VaultConfig {
            search_range: 33,
            ..base.clone()
        },
        VaultConfig {
            format: PixelFormat::Rgb24,
            ..base.clone()
        },
    ];
    for c in bad {
        let desc = format!("{c:?}");
        assert!(
            VaultEncoder::new(CodecConfig::default(), c).is_err(),
            "encoder accepted {desc}"
        );
    }
}

#[test]
fn extreme_pixel_content_keeps_exact_geometry() {
    for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
        for fill in [0u8, 255] {
            let mut enc = VaultEncoder::new(CodecConfig::default(), cfg(format)).unwrap();
            let mut dec = VaultDecoder::new(CodecConfig::default(), cfg(format)).unwrap();
            let f = frame(W, H, format, |_| fill);
            let mut packets = enc.encode(&f).unwrap();
            packets.extend(enc.drain().unwrap());
            let mut out = Vec::new();
            for p in &packets {
                out.extend(dec.decode(p).unwrap());
            }
            out.extend(dec.flush().unwrap());
            assert_eq!(out.len(), 1, "one frame in, one frame out");
            assert_eq!((out[0].width, out[0].height, out[0].format), (W, H, format));
            assert_eq!(
                out[0].pixels.len(),
                VideoFrame::alloc(W, H, format).pixels.len()
            );
        }
    }
}

#[test]
fn overflowing_geometry_is_refused_not_a_panic() {
    let huge = 1u32 << 30;
    let c = VaultConfig {
        width: huge,
        height: huge,
        format: PixelFormat::Gray8,
        ..VaultConfig::default()
    };
    let refused = match VaultEncoder::new(CodecConfig::default(), c.clone()) {
        Err(_) => true,
        Ok(mut e) => e
            .encode(&VideoFrame::alloc(huge, huge, PixelFormat::Gray8))
            .is_err(),
    };
    assert!(refused, "a 2^30 x 2^30 frame must be refused");
}
