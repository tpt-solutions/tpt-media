//! Vault encode/decode benchmarks.
//!
//! Vault is an offline codec: the encoder's cost is dominated by the RDO
//! (every partition level prices an intra, an inter and a skip candidate,
//! each with a real transform) and by the multi-reference motion search.
//! Each group therefore measures, at one working point:
//!
//! - `encode_i_frame` - intra-only stream (`keyframe_interval = 0`);
//! - `encode_p_frame` - I then P frames, default `max_refs`;
//! - `encode_p_frame_refs1` - the same with a single reference, so the cost
//!   of the extra references is visible;
//! - `decode`.
//!
//! The clip is a smooth gradient with a panning textured block and mild
//! noise: content a block-partitioning codec is built for, with real motion
//! for the search. Rate and quality are printed (mean bytes per packet and
//! luma PSNR) so `docs/performance.md` quotes measured values only.

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_vault::{VaultConfig, VaultDecoder, VaultEncoder};

/// Frames per benchmark iteration.
const FRAMES: usize = 4;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// A synthetic clip: smooth gradient, a panning textured block, mild noise.
fn clip(width: u32, height: u32, format: PixelFormat, frames: usize) -> Vec<VideoFrame> {
    (0..frames)
        .map(|i| {
            let mut f = VideoFrame::alloc(width, height, format);
            let (w, h) = (width as usize, height as usize);
            let mut state = 0x5EED_A11A_u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
            let pan = i * 3;
            for y in 0..h {
                for x in 0..w {
                    let mut v = 40 + (x * 120 / w) as i32 + (y * 70 / h) as i32;
                    let bx = (x + w * 4 - pan) % w;
                    if bx > w / 2 && bx < w / 2 + w / 4 && y > h / 4 && y < h / 2 + h / 8 {
                        v = 90 + ((bx * 7 + y * 5) % 90) as i32;
                    }
                    v += (lcg(&mut state) >> 60) as i32 & 3;
                    f.pixels[y * w + x] = v.clamp(0, 255) as u8;
                }
            }
            if format == PixelFormat::Yuv420 {
                let (cw, ch) = (w / 2, h / 2);
                let base = w * h;
                for c in 0..2 {
                    for y in 0..ch {
                        for x in 0..cw {
                            f.pixels[base + c * cw * ch + y * cw + x] =
                                (110 + x * 30 / cw + c * 20) as u8;
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

fn luma_psnr(a: &VideoFrame, b: &VideoFrame, luma: usize) -> f64 {
    let mse = a.pixels[..luma]
        .iter()
        .zip(&b.pixels[..luma])
        .map(|(x, y)| {
            let d = f64::from(*x) - f64::from(*y);
            d * d
        })
        .sum::<f64>()
        / luma as f64;
    if mse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

fn bench_point(c: &mut Criterion, name: &str, cfg: VaultConfig, frames: usize) {
    let input = clip(cfg.width, cfg.height, cfg.format, frames);
    let mut encoder = VaultEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let mut decoder = VaultDecoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let mut packets = Vec::new();
    let mut psnr = 0.0;
    let luma = (cfg.width * cfg.height) as usize;
    for f in &input {
        let p = encoder.encode(f).unwrap().remove(0);
        let out = decoder.decode(&p).unwrap().remove(0);
        psnr += luma_psnr(f, &out, luma);
        packets.push(p);
    }
    let mean_bytes = packets.iter().map(|p| p.data.len()).sum::<usize>() / packets.len().max(1);
    let refs1 = VaultConfig {
        max_refs: 1,
        ..cfg.clone()
    };

    let mut group = c.benchmark_group(name);
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(5));

    group.bench_function("encode_i_frame", |b| {
        b.iter(|| {
            let mut enc =
                VaultEncoder::with_keyframe_interval(CodecConfig::default(), cfg.clone(), 0)
                    .unwrap();
            for frame in black_box(&input) {
                black_box(enc.encode(frame).unwrap());
            }
        })
    });

    group.bench_function("encode_p_frame", |b| {
        b.iter(|| {
            let mut enc = VaultEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
            for frame in black_box(&input) {
                black_box(enc.encode(frame).unwrap());
            }
        })
    });

    group.bench_function("encode_p_frame_refs1", |b| {
        b.iter(|| {
            let mut enc = VaultEncoder::new(CodecConfig::default(), refs1.clone()).unwrap();
            for frame in black_box(&input) {
                black_box(enc.encode(frame).unwrap());
            }
        })
    });

    group.bench_function("decode", |b| {
        b.iter(|| {
            let mut dec = VaultDecoder::new(CodecConfig::default(), cfg.clone()).unwrap();
            for pkt in black_box(&packets) {
                black_box(dec.decode(pkt).unwrap());
            }
        })
    });

    group.finish();
    eprintln!(
        "{name}: {mean_bytes} bytes/packet mean, {:.2} dB mean luma PSNR over {} frames",
        psnr / packets.len().max(1) as f64,
        packets.len()
    );
}

fn bench_vault(c: &mut Criterion) {
    bench_point(
        c,
        "vault/qcif_176x144_yuv420",
        VaultConfig {
            width: 176,
            height: 144,
            format: PixelFormat::Yuv420,
            ..VaultConfig::default()
        },
        FRAMES,
    );
    bench_point(
        c,
        "vault/cif_352x288_yuv420",
        VaultConfig {
            width: 352,
            height: 288,
            format: PixelFormat::Yuv420,
            ..VaultConfig::default()
        },
        FRAMES,
    );
}

criterion_group!(benches, bench_vault);
criterion_main!(benches);
