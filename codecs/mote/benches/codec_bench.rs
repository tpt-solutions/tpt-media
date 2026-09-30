//! Mote benchmarks: CPU (encode/decode), bandwidth and quality (printed),
//! and training cost.
//!
//! The bandwidth "target" is **defined by this benchmark**, not promised
//! for arbitrary content (todo.md 34). The scene is a fixed camera:
//! static textured background, per-frame sensor noise of +/-1 grey level,
//! and a 32x32 object crossing the frame. Everything is seeded, so the
//! printed figures reproduce exactly on the same toolchain.
//!
//! Working points:
//!
//! - `qvga_320x240_gray8_d4096`: the initial target - a 4096-entry
//!   dictionary (12-bit index) trained on the deployment scene.
//! - `qvga_320x240_gray8_d256`: an 8-bit-index dictionary, to show what
//!   the dictionary size buys.
//! - `qvga_320x240_yuv420_d4096`: colour.
//! - `vga_640x480_gray8_d4096`: the 480p ceiling.
//!
//! Numbers land in `docs/performance.md`.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{Packet, PacketFlags, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_mote::{
    compare, Dictionary, MoteConfig, MoteDecoder, MoteEncoder, TrainConfig, CRC_TRAILER_BYTES,
};

/// Frames per timed pass: one key frame then `FRAMES - 1` delta frames.
const FRAMES: usize = 8;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 33
}

/// Fixed-camera scene. `scene_seed` picks the (static) background
/// texture, so two seeds are two different "sites".
fn camera(w: u32, h: u32, format: PixelFormat, scene_seed: u64, i: usize) -> VideoFrame {
    let mut f = VideoFrame::alloc(w, h, format);
    let (wu, hu) = (w as usize, h as usize);
    let mut noise = 0x5EED_0000_0000u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
    let ox = (i * 5) % (wu - 32);
    for y in 0..hu {
        for x in 0..wu {
            // Static background: gradient + coarse tone per 16x16 cell +
            // fixed fine texture (hash of position and site).
            let mut h = scene_seed ^ ((x as u64) << 20) ^ (y as u64);
            let tex = (lcg(&mut h) % 13) as i32 - 6;
            let cell = ((x / 16 * 7 + y / 16 * 13) % 5) as i32 * 14;
            let mut v = 50 + ((x + y) * 90 / (wu + hu)) as i32 + cell + tex;
            v += (lcg(&mut noise) % 3) as i32 - 1;
            if (ox..ox + 32).contains(&x) && (hu / 3..hu / 3 + 32).contains(&y) {
                v = 235 - ((x - ox) as i32 / 4);
            }
            f.pixels[y * wu + x] = v.clamp(0, 255) as u8;
        }
    }
    if format == PixelFormat::Yuv420 {
        let (cw, ch) = (wu / 2, hu / 2);
        for c in 0..2 {
            for y in 0..ch {
                for x in 0..cw {
                    let mut h = scene_seed ^ 0xC0 ^ ((x as u64) << 20) ^ (y as u64) ^ (c as u64);
                    let tex = (lcg(&mut h) % 5) as i32 - 2;
                    let base = 110 + (x + y * (c + 1)) * 40 / (cw + ch) + ((x / 8 + y / 8) % 3) * 6;
                    f.pixels[wu * hu + c * cw * ch + y * cw + x] =
                        (base as i32 + tex).clamp(0, 255) as u8;
                }
            }
        }
    }
    f.frame.pts = Timestamp {
        ticks: i as i64,
        timebase: Timebase::new(1, 30),
    };
    f
}

struct Point {
    name: &'static str,
    cfg: MoteConfig,
    entries: usize,
    train_frames: usize,
}

struct Prepared {
    dict: Dictionary,
    frames: Vec<VideoFrame>,
    packets: Vec<Packet>,
}

fn prepare(p: &Point) -> Prepared {
    let train: Vec<_> = (0..p.train_frames)
        .map(|i| camera(p.cfg.width, p.cfg.height, p.cfg.format, 1, i * 3))
        .collect();
    let t0 = Instant::now();
    let dict = Dictionary::train_from_frames(
        &train,
        &TrainConfig {
            entries: p.entries,
            iterations: 4,
            ..TrainConfig::default()
        },
    )
    .unwrap();
    let train_s = t0.elapsed().as_secs_f64();

    // Timed stream: frames the dictionary was *not* trained on (same
    // site, later in time).
    let frames: Vec<_> = (0..FRAMES)
        .map(|i| camera(p.cfg.width, p.cfg.height, p.cfg.format, 1, 100 + i))
        .collect();
    let mut enc =
        MoteEncoder::with_dictionary(CodecConfig::default(), p.cfg.clone(), dict.clone(), false)
            .unwrap();
    let packets: Vec<_> = frames.iter().flat_map(|f| enc.encode(f).unwrap()).collect();

    // Quality + bandwidth accounting.
    let mut dec =
        MoteDecoder::with_dictionary(CodecConfig::default(), p.cfg.clone(), dict.clone()).unwrap();
    let (mut psnr_sum, mut psnr_min) = (0.0f64, f64::MAX);
    let (mut key_bytes, mut delta_sum, mut delta_n) = (0usize, 0usize, 0usize);
    for (pkt, f) in packets.iter().zip(frames.iter()) {
        if pkt.flags.contains(PacketFlags::KEYFRAME) {
            key_bytes = pkt.data.len();
        } else {
            delta_sum += pkt.data.len();
            delta_n += 1;
        }
        let out = dec.decode(pkt).unwrap().remove(0);
        let q = compare(f, &out).unwrap();
        psnr_sum += q.luma_psnr_db();
        psnr_min = psnr_min.min(q.luma_psnr_db());
    }
    let mean_delta = delta_sum as f64 / delta_n.max(1) as f64;

    // Held-out site: the same dictionary on a different background.
    let other = camera(p.cfg.width, p.cfg.height, p.cfg.format, 2, 100);
    let mut e2 =
        MoteEncoder::with_dictionary(CodecConfig::default(), p.cfg.clone(), dict.clone(), false)
            .unwrap();
    let mut d2 =
        MoteDecoder::with_dictionary(CodecConfig::default(), p.cfg.clone(), dict.clone()).unwrap();
    let kp = e2.encode(&other).unwrap().remove(0);
    let held_out = compare(&other, &d2.decode(&kp).unwrap().remove(0))
        .unwrap()
        .luma_psnr_db();

    let raw = p.cfg.frame_bytes();
    let dict_packet = MoteConfig::dictionary_packet_bytes(dict.len());
    eprintln!(
        "[{name}] {w}x{h} {fmt:?}, dictionary {n} entries ({ib}-bit index): \
         trained on {tf} frames in {train_s:.2} s; dictionary packet {dict_packet} B; \
         key frame {key_bytes} B ({key_ratio:.0}:1 vs raw {raw} B); \
         mean delta {mean_delta:.0} B; luma PSNR mean {pm:.1} dB / min {pmin:.1} dB \
         (held-out site, key frame: {held_out:.1} dB)\n  \
         steady-state bandwidth (delta frames only): {k1:.2} kbps @1 fps, {k5:.2} kbps @5 fps, \
         {k15:.2} kbps @15 fps; key-frame-only stream: {kk1:.1} kbps @1 fps",
        name = p.name,
        w = p.cfg.width,
        h = p.cfg.height,
        fmt = p.cfg.format,
        n = dict.len(),
        ib = dict.index_bits(),
        tf = p.train_frames,
        key_ratio = raw as f64 / key_bytes as f64,
        pm = psnr_sum / packets.len() as f64,
        pmin = psnr_min,
        k1 = mean_delta * 8.0 / 1000.0,
        k5 = mean_delta * 8.0 * 5.0 / 1000.0,
        k15 = mean_delta * 8.0 * 15.0 / 1000.0,
        kk1 = key_bytes as f64 * 8.0 / 1000.0,
    );
    let _ = CRC_TRAILER_BYTES;
    Prepared {
        dict,
        frames,
        packets,
    }
}

fn bench_point(c: &mut Criterion, p: &Point) {
    let prep = prepare(p);
    let mut group = c.benchmark_group(format!("mote/{}", p.name));
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(3));

    // Key frames only (interval 0): every patch searched.
    group.bench_function("encode_key_frame", |b| {
        b.iter(|| {
            let mut enc = MoteEncoder::with_dictionary_and_keyframe_interval(
                CodecConfig::default(),
                p.cfg.clone(),
                prep.dict.clone(),
                false,
                0,
            )
            .unwrap();
            for f in black_box(&prep.frames) {
                black_box(enc.encode(f).unwrap());
            }
        })
    });

    // Delta frames only: the key frame is encoded outside the timed region.
    group.bench_function("encode_delta_frame", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let mut enc = MoteEncoder::with_dictionary(
                    CodecConfig::default(),
                    p.cfg.clone(),
                    prep.dict.clone(),
                    false,
                )
                .unwrap();
                enc.encode(&prep.frames[0]).unwrap();
                let t = Instant::now();
                for f in black_box(&prep.frames[1..]) {
                    black_box(enc.encode(f).unwrap());
                }
                total += t.elapsed();
            }
            total
        })
    });

    group.bench_function("decode_key_frame", |b| {
        b.iter(|| {
            let mut dec = MoteDecoder::with_dictionary(
                CodecConfig::default(),
                p.cfg.clone(),
                prep.dict.clone(),
            )
            .unwrap();
            black_box(dec.decode(black_box(&prep.packets[0])).unwrap());
        })
    });

    group.bench_function("decode_delta_frame", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let mut dec = MoteDecoder::with_dictionary(
                    CodecConfig::default(),
                    p.cfg.clone(),
                    prep.dict.clone(),
                )
                .unwrap();
                dec.decode(&prep.packets[0]).unwrap();
                let t = Instant::now();
                for pkt in black_box(&prep.packets[1..]) {
                    black_box(dec.decode(pkt).unwrap());
                }
                total += t.elapsed();
            }
            total
        })
    });

    group.finish();
}

fn bench_training(c: &mut Criterion) {
    // Offline dictionary training, smaller than the working points so the
    // criterion sample loop stays short; the full-size training times are
    // printed by `prepare`.
    let frames: Vec<_> = (0..2)
        .map(|i| camera(320, 240, PixelFormat::Gray8, 1, i * 3))
        .collect();
    let mut group = c.benchmark_group("mote/training_qvga_gray8_2frames_256entries_4iter");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(3));
    group.bench_function("train", |b| {
        b.iter(|| {
            black_box(
                Dictionary::train_from_frames(
                    black_box(&frames),
                    &TrainConfig {
                        entries: 256,
                        iterations: 4,
                        ..TrainConfig::default()
                    },
                )
                .unwrap(),
            )
        })
    });
    group.finish();
}

fn bench_mote(c: &mut Criterion) {
    let base = MoteConfig::default();
    let points = [
        Point {
            name: "qvga_320x240_gray8_d4096",
            cfg: MoteConfig {
                width: 320,
                height: 240,
                format: PixelFormat::Gray8,
                ..base.clone()
            },
            entries: 4096,
            train_frames: 4,
        },
        Point {
            name: "qvga_320x240_gray8_d256",
            cfg: MoteConfig {
                width: 320,
                height: 240,
                format: PixelFormat::Gray8,
                ..base.clone()
            },
            entries: 256,
            train_frames: 4,
        },
        Point {
            name: "qvga_320x240_yuv420_d4096",
            cfg: MoteConfig {
                width: 320,
                height: 240,
                format: PixelFormat::Yuv420,
                ..base.clone()
            },
            entries: 4096,
            train_frames: 4,
        },
        Point {
            name: "vga_640x480_gray8_d4096",
            cfg: MoteConfig {
                width: 640,
                height: 480,
                format: PixelFormat::Gray8,
                ..base.clone()
            },
            entries: 4096,
            train_frames: 3,
        },
    ];
    for p in &points {
        bench_point(c, p);
    }
    bench_training(c);
}

criterion_group!(benches, bench_mote);
criterion_main!(benches);
