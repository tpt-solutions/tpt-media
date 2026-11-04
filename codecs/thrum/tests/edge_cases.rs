//! Edge-case sweep for Thrum's public surface: wrong geometry and format,
//! short pixel buffers, empty and truncated packets, invalid configuration,
//! and extreme pixel content. Complements the unit tests in `src/lib.rs` and
//! the random sweeps in `property.rs`.

use tpt_media_codec::{CodecConfig, CodecError, Decoder, Encoder};
use tpt_media_core::{Packet, PacketFlags, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_thrum::{ThrumConfig, ThrumDecoder, ThrumEncoder};

const W: u32 = 32;
const H: u32 = 32;

fn cfg(format: PixelFormat) -> ThrumConfig {
    ThrumConfig {
        width: W,
        height: H,
        format,
        ..ThrumConfig::default()
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
    let mut enc = ThrumEncoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
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
    let mut enc = ThrumEncoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
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
    let mut enc = ThrumEncoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    let f = frame(W, H, PixelFormat::Gray8, |i| (i * 3) as u8);
    let valid = enc
        .encode(&f)
        .unwrap()
        .into_iter()
        .next()
        .expect("one packet");
    let mut dec = ThrumDecoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    assert!(
        dec.decode(&packet(Vec::new())).is_err(),
        "empty packet must fail"
    );
    for len in 0..valid.data.len() {
        let mut d = ThrumDecoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
        assert!(
            d.decode(&packet(valid.data[..len].to_vec())).is_err(),
            "prefix of length {len} of {} decoded as complete",
            valid.data.len()
        );
    }
    let mut d = ThrumDecoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    assert_eq!(
        d.decode(&valid).unwrap().len(),
        1,
        "the intact packet decodes"
    );
}

#[test]
fn invalid_configs_are_refused_at_construction() {
    let base = cfg(PixelFormat::Gray8);
    let bad = [
        ThrumConfig {
            width: 0,
            ..base.clone()
        },
        ThrumConfig {
            height: 0,
            ..base.clone()
        },
        ThrumConfig {
            width: 17,
            ..base.clone()
        }, // not a macroblock multiple
        ThrumConfig {
            qp: 0,
            ..base.clone()
        },
        ThrumConfig {
            qp: 52,
            ..base.clone()
        },
        ThrumConfig {
            search_range: 17,
            ..base.clone()
        },
        ThrumConfig {
            intra_refresh_percent: 101,
            ..base.clone()
        },
        ThrumConfig {
            format: PixelFormat::Rgb24,
            ..base.clone()
        },
    ];
    for c in bad {
        let desc = format!("{c:?}");
        assert!(
            ThrumEncoder::new(CodecConfig::default(), c).is_err(),
            "encoder accepted {desc}"
        );
    }
}

#[test]
fn extreme_pixel_content_keeps_exact_geometry() {
    for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
        for fill in [0u8, 255] {
            let mut enc = ThrumEncoder::new(CodecConfig::default(), cfg(format)).unwrap();
            let mut dec = ThrumDecoder::new(CodecConfig::default(), cfg(format)).unwrap();
            let f = frame(W, H, format, |_| fill);
            let packets = enc.encode(&f).unwrap();
            assert_eq!(packets.len(), 1);
            let out = dec.decode(&packets[0]).unwrap();
            assert_eq!(out.len(), 1);
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
    // Dimensions whose byte count overflows 32-bit arithmetic must be refused
    // at construction or encode, never wrap around or panic.
    let huge = 1u32 << 30;
    let c = ThrumConfig {
        width: huge,
        height: huge,
        format: PixelFormat::Gray8,
        ..ThrumConfig::default()
    };
    let refused = match ThrumEncoder::new(CodecConfig::default(), c.clone()) {
        Err(_) => true,
        Ok(mut e) => e
            .encode(&VideoFrame::alloc(huge, huge, PixelFormat::Gray8))
            .is_err(),
    };
    assert!(refused, "a 2^30 x 2^30 frame must be refused");
}
