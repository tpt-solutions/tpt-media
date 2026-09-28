//! Thrum encode/decode benchmarks.
//!
//! Two working points, because the codec's cost is dominated by the
//! exhaustive motion search and that only applies to P-frames:
//!
//! - `qcif` (176x144, 4:2:0) - a plausible real-time call resolution.
//! - `vga_gray` (640x480, mono) - a larger frame, and the worst case for
//!   the search, at the default `search_range`.
//!
//! Each group measures I-frames and P-frames separately, and the P-frame
//! figures are the ones that bound Thrum's one-way latency: the
//! encoder never holds a frame back, so encode + decode of a single
//! P-frame *is* the latency contribution. Numbers land in
//! `docs/performance.md`.

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_thrum::{ThrumConfig, ThrumDecoder, ThrumEncoder};

/// Frames per benchmark iteration.
const FRAMES: usize = 8;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// A synthetic clip: a panning diagonal ramp plus mild noise, so motion
/// estimation has genuine, non-trivial work to do every frame.
fn clip(width: u32, height: u32, format: PixelFormat, frames: usize) -> Vec<VideoFrame> {
    (0..frames)
        .map(|i| {
            let mut f = VideoFrame::alloc(width, height, format);
            let (w, h) = (width as usize, height as usize);
            let mut state = 0x5EED_A11A_u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
            for y in 0..h {
                for x in 0..w {
                    let phase = x as i32 + y as i32 + (i as i32) * 4;
                    let v = phase.rem_euclid(256) as u8;
                    f.pixels[y * w + x] = v ^ (lcg(&mut state) as u8 & 7);
                }
            }
            if format == PixelFormat::Yuv420 {
                let (cw, ch) = (w / 2, h / 2);
                let base = w * h;
                for c in 0..2 {
                    for y in 0..ch {
                        for x in 0..cw {
                            let v =
                                (64 + (x as i32 + y as i32 + (i as i32) * 2).rem_euclid(180)) as u8;
                            f.pixels[base + c * cw * ch + y * cw + x] = v;
                        }
                    }
                }
            }
            f.frame.pts = Timestamp {
                ticks: i as i64,
                timebase: Timebase::new(1, 30),
            };
            f
        })
        .collect()
}

fn bench_point(c: &mut Criterion, name: &str, cfg: ThrumConfig, frames: usize) {
    let input = clip(cfg.width, cfg.height, cfg.format, frames);
    let mut encoder = ThrumEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let packets: Vec<_> = input
        .iter()
        .flat_map(|f| encoder.encode(f).unwrap())
        .collect();
    let mean_bytes = packets.iter().map(|p| p.data.len()).sum::<usize>() / packets.len().max(1);

    let mut group = c.benchmark_group(name);
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("encode_i_frame", |b| {
        b.iter(|| {
            let mut enc =
                ThrumEncoder::with_keyframe_interval(CodecConfig::default(), cfg.clone(), 0)
                    .unwrap();
            for frame in black_box(&input) {
                black_box(enc.encode(frame).unwrap());
            }
        })
    });

    group.bench_function("encode_p_frame", |b| {
        b.iter(|| {
            let mut enc = ThrumEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
            for frame in black_box(&input) {
                black_box(enc.encode(frame).unwrap());
            }
        })
    });

    group.bench_function("decode", |b| {
        b.iter(|| {
            let mut dec = ThrumDecoder::new(CodecConfig::default(), cfg.clone()).unwrap();
            for pkt in black_box(&packets) {
                black_box(dec.decode(pkt).unwrap());
            }
        })
    });

    group.finish();
    eprintln!("{name}: {mean_bytes} bytes/packet mean");
}

fn bench_thrum(c: &mut Criterion) {
    bench_point(
        c,
        "thrum/qcif_176x144_yuv420",
        ThrumConfig {
            width: 176,
            height: 144,
            format: PixelFormat::Yuv420,
            ..ThrumConfig::default()
        },
        FRAMES,
    );
    bench_point(
        c,
        "thrum/vga_640x480_gray8",
        ThrumConfig {
            width: 640,
            height: 480,
            format: PixelFormat::Gray8,
            ..ThrumConfig::default()
        },
        FRAMES,
    );
}

criterion_group!(benches, bench_thrum);
criterion_main!(benches);
