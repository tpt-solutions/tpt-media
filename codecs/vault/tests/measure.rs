//! Reproducible measurement harness behind the tables in
//! `docs/performance.md` (Vault section). Not a pass/fail test: every test
//! here is `#[ignore]`d and only prints.
//!
//! ```text
//! cargo test --release -p tpt-video-vault --test measure -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Rates and PSNRs are deterministic (the codec is a pure function of its
//! input); the millisecond figures are machine- and load-dependent.

use std::time::Instant;

use tpt_media_codec::{CodecConfig, Decoder, Encoder, ResourceLimits};
use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_vault::{VaultConfig, VaultDecoder, VaultEncoder};

const W: usize = 176;
const H: usize = 144;
const FRAMES: usize = 8;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// Gradient + panning textured block + `noise_amp` LSBs of noise, 4:2:0.
fn panning_clip(noise_amp: i32) -> Vec<VideoFrame> {
    (0..FRAMES)
        .map(|i| {
            let mut f = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
            let mut state = 0x5EED_A11A_u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
            let pan = i * 3;
            for y in 0..H {
                for x in 0..W {
                    let mut v = 40 + (x * 120 / W) as i32 + (y * 70 / H) as i32;
                    let bx = (x + W * 4 - pan) % W;
                    if bx > W / 2 && bx < W / 2 + W / 4 && y > H / 4 && y < H / 2 + H / 8 {
                        v = 90 + ((bx * 7 + y * 5) % 90) as i32;
                    }
                    if noise_amp > 0 {
                        v += (lcg(&mut state) >> 60) as i32 % (noise_amp + 1);
                    }
                    f.pixels[y * W + x] = v.clamp(0, 255) as u8;
                }
            }
            let (cw, ch) = (W / 2, H / 2);
            for c in 0..2 {
                for y in 0..ch {
                    for x in 0..cw {
                        f.pixels[W * H + c * cw * ch + y * cw + x] =
                            (110 + x * 30 / cw + c * 20) as u8;
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

/// Bilinearly interpolated coarse noise ("texture"), panning 2 px/frame.
fn texture_clip() -> Vec<VideoFrame> {
    let mut state = 77u64;
    let (gw, gh) = ((W + 64) / 6 + 2, H / 6 + 2);
    let grid: Vec<f32> = (0..gw * gh)
        .map(|_| 40.0 + (lcg(&mut state) >> 56) as f32 * 0.6)
        .collect();
    (0..FRAMES)
        .map(|i| {
            let mut f = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
            for y in 0..H {
                for x in 0..W {
                    let (fx, fy) = ((x + i * 2) as f32 / 6.0, y as f32 / 6.0);
                    let (ix, iy) = (fx as usize, fy as usize);
                    let (ax, ay) = (fx - ix as f32, fy - iy as f32);
                    let g = |gx: usize, gy: usize| grid[gy * gw + gx];
                    let v = g(ix, iy) * (1.0 - ax) * (1.0 - ay)
                        + g(ix + 1, iy) * ax * (1.0 - ay)
                        + g(ix, iy + 1) * (1.0 - ax) * ay
                        + g(ix + 1, iy + 1) * ax * ay;
                    f.pixels[y * W + x] = v as u8;
                }
            }
            f.pixels[W * H..].fill(128);
            f
        })
        .collect()
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let mse = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let d = f64::from(*x) - f64::from(*y);
            d * d
        })
        .sum::<f64>()
        / a.len() as f64;
    if mse == 0.0 {
        99.0
    } else {
        10.0 * (255.0f64 * 255.0 / mse).log10()
    }
}

/// Encode + decode the clip; mean bytes per frame, mean luma PSNR, ms/frame
/// (encode + decode), and the last frame's census.
fn run(input: &[VideoFrame], cfg: VaultConfig) -> (usize, f64, f64, String) {
    let fw = CodecConfig {
        limits: ResourceLimits::permissive(),
        ..CodecConfig::default()
    };
    let mut enc = VaultEncoder::new(fw.clone(), cfg.clone()).unwrap();
    let mut dec = VaultDecoder::new(fw, cfg).unwrap();
    let (mut bytes, mut ps) = (0usize, 0.0);
    let t = Instant::now();
    for f in input {
        let p = enc.encode(f).unwrap().remove(0);
        bytes += p.data.len();
        let o = dec.decode(&p).unwrap().remove(0);
        ps += psnr(&f.pixels[..W * H], &o.pixels[..W * H]);
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / input.len() as f64;
    let s = dec.last_stats();
    let census = format!(
        "leaves[8,16,32,64]={:?} skip={} inter={} intra={} half_pel={}",
        s.leaves_by_size, s.skip, s.inter, s.intra, s.half_pel
    );
    (bytes / input.len(), ps / input.len() as f64, ms, census)
}

fn base() -> VaultConfig {
    VaultConfig {
        width: W as u32,
        height: H as u32,
        ..VaultConfig::default()
    }
}

#[test]
#[ignore]
fn rate_quality_by_qp() {
    for (name, clip) in [("noisy(3)", panning_clip(3)), ("clean(0)", panning_clip(0))] {
        println!("== 176x144 4:2:0, {FRAMES} frames, clip {name}");
        for qp in [14u8, 20, 26, 32, 38, 44] {
            let (b, p, _, c) = run(&clip, VaultConfig { qp, ..base() });
            println!("qp {qp:2}: {b:5} B/frame {p:.2} dB  {c}");
        }
    }
}

#[test]
#[ignore]
fn loop_filter_on_and_off() {
    for (name, clip, qps) in [
        ("panning noisy(3)", panning_clip(3), [26u8, 38, 46]),
        ("texture", texture_clip(), [26u8, 38, 44]),
    ] {
        println!("== loop filter, clip {name}");
        for qp in qps {
            for lf in [false, true] {
                let (b, p, _, _) = run(
                    &clip,
                    VaultConfig {
                        qp,
                        loop_filter: lf,
                        ..base()
                    },
                );
                println!("qp {qp} loop_filter {lf:5}: {b:4} B/frame {p:.3} dB");
            }
        }
    }
}

#[test]
#[ignore]
fn max_block_size_and_references() {
    let clip = panning_clip(3);
    println!("== max_block_size, panning noisy(3), qp 26");
    for mb in [8usize, 16, 32, 64] {
        let (b, p, ms, _) = run(
            &clip,
            VaultConfig {
                qp: 26,
                max_block_size: mb,
                ..base()
            },
        );
        println!("max_block_size {mb:2}: {b:4} B/frame {p:.3} dB  {ms:.0} ms/frame");
    }
    println!("== max_refs, panning noisy(3), default qp");
    for refs in [1u8, 4, 16] {
        let (b, p, ms, _) = run(
            &clip,
            VaultConfig {
                max_refs: refs,
                ..base()
            },
        );
        println!("max_refs {refs:2}: {b:4} B/frame {p:.3} dB  {ms:.0} ms/frame");
    }
}

/// Per-pixel uniform noise, reseeded per frame — the one content shape
/// where deblocking moves *every* edge sample away from the source
/// (the source's own edges are noise), so the same-frame distortion
/// genuinely prefers the filter off.
fn noise_clip() -> Vec<VideoFrame> {
    (0..FRAMES)
        .map(|i| {
            let mut f = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
            let mut state = 0xB00B_1E55_u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
            for y in 0..H {
                for x in 0..W {
                    f.pixels[y * W + x] = (((lcg(&mut state) >> 56) * 256) >> 8) as u8;
                }
            }
            f.pixels[W * H..].fill(128);
            f.frame.pts = Timestamp {
                ticks: i as i64,
                timebase: Timebase::new(1, 30),
            };
            f
        })
        .collect()
}

/// The RD-decided loop-filter flag: fixed off, fixed on, and per-frame
/// RD, with the RD run's per-frame decision printed from the decoder's
/// census.
#[test]
#[ignore]
fn loop_filter_rdo_decision() {
    for (name, clip, qps) in [
        ("panning noisy(3)", panning_clip(3), [26u8, 46]),
        ("texture", texture_clip(), [26, 38]),
        ("pure noise", noise_clip(), [26, 40]),
    ] {
        println!("== loop filter rdo, clip {name}");
        for qp in qps {
            for (mode, lf, rdo) in [("off", false, false), ("on ", true, false), ("rdo", true, true)]
            {
                let cfg = VaultConfig {
                    qp,
                    loop_filter: lf,
                    loop_filter_rdo: rdo,
                    ..base()
                };
                let fwc = CodecConfig {
                    limits: ResourceLimits::permissive(),
                    ..CodecConfig::default()
                };
                let mut enc = VaultEncoder::new(fwc.clone(), cfg.clone()).unwrap();
                let mut dec = VaultDecoder::new(fwc, cfg).unwrap();
                let (mut bytes, mut ps) = (0usize, 0.0);
                let mut flags = Vec::new();
                for f in &clip {
                    let p = enc.encode(f).unwrap().remove(0);
                    bytes += p.data.len();
                    let o = dec.decode(&p).unwrap().remove(0);
                    ps += psnr(&f.pixels[..W * H], &o.pixels[..W * H]);
                    flags.push(if dec.last_stats().loop_filter { 'F' } else { '.' });
                }
                let decisions: String = flags.into_iter().collect();
                let ps = ps / clip.len() as f64;
                println!(
                    "qp {qp:2} filter {mode}: {bytes:4} B/clip {ps:.3} dB/clip  [{decisions}] (F = filtered)"
                );
            }
        }
    }
}

/// Flat synthetic scenes (a dark rising gradient vs a bright falling one
/// with different chroma) with a hard cut halfway — the `scene_cut`
/// knob's home turf. With the keyframe schedule parked far away, prints
/// intra count, tail (post-cut) bytes and mean post-cut PSNR for the
/// knob off and on.
#[test]
#[ignore]
fn scene_cut_on_and_off() {
    let clip: Vec<VideoFrame> = (0..16)
        .map(|i| {
            let mut f = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
            let pan = i * 2;
            for y in 0..H {
                for x in 0..W {
                    let g = (x * 140 / W + y * 60 / H) as i32;
                    let stripe = (x as i32 + pan) % 24;
                    let v = if i < 8 {
                        30 + (g + stripe) / 2
                    } else {
                        225 - (g + stripe) / 3
                    };
                    f.pixels[y * W + x] = v.clamp(0, 255) as u8;
                }
            }
            f.pixels[W * H..].fill(if i < 8 { 128 } else { 100 });
            f.frame.pts = Timestamp {
                ticks: i as i64,
                timebase: Timebase::new(1, 30),
            };
            f
        })
        .collect();
    for scene_cut in [false, true] {
        let cfg = VaultConfig { scene_cut, ..base() };
        let fwc = CodecConfig {
            limits: ResourceLimits::permissive(),
            ..CodecConfig::default()
        };
        let mut enc = VaultEncoder::with_keyframe_interval(fwc.clone(), cfg.clone(), 10_000)
            .unwrap();
        let mut dec = VaultDecoder::new(fwc, cfg).unwrap();
        let mut packets = Vec::new();
        for f in &clip {
            packets.extend(enc.encode(f).unwrap());
        }
        let keys = packets
            .iter()
            .filter(|p| p.flags.contains(tpt_media_core::PacketFlags::KEYFRAME))
            .count();
        let mut post_psnr = 0.0;
        let mut post_n = 0usize;
        for (i, p) in packets.iter().enumerate() {
            for o in dec.decode(p).unwrap() {
                if i >= 8 {
                    post_psnr += psnr(&clip[i].pixels[..W * H], &o.pixels[..W * H]);
                    post_n += 1;
                }
            }
        }
        println!(
            "scene_cut {scene_cut:5}: {keys} intra, total {} B, post-cut tail {} B, post-cut PSNR {:.2} dB",
            packets.iter().map(|p| p.data.len()).sum::<usize>(),
            packets[8..].iter().map(|p| p.data.len()).sum::<usize>(),
            post_psnr / post_n.max(1) as f64,
        );
    }
}

/// The per-frame bi-prediction weight (v3): which weight the frame-`J`
/// search picks on content where averaging is right (the pan clip) and
/// where a quarter weight is right (an exposure ramp, whose B frame is
/// exactly 3/4 of one reference). Prints per-B-frame decisions, bytes
/// and PSNR.
#[test]
#[ignore]
fn b_weight_decisions() {
    for (name, frames) in [
        ("pan", panning_clip(3)),
        ("exposure ramp", (0..9)
            .map(|i| {
                let mut f = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
                for y in 0..H {
                    for x in 0..W {
                        // Frame i fades quadratically toward bright: the
                        // midpoint is NOT the average of its neighbours.
                        let t = (i * i) as f32 / 64.0;
                        f.pixels[y * W + x] =
                            (60.0 + (180.0 - 60.0) * t + (x / 8) as f32) as u8;
                    }
                }
                f.pixels[W * H..].fill(110);
                f.frame.pts = Timestamp {
                    ticks: i as i64,
                    timebase: Timebase::new(1, 30),
                };
                f
            })
            .collect()),
    ] {
        for qp in [30u8, 18] {
        let cfg = VaultConfig {
            b_frames: 2,
            qp,
            ..base()
        };
        let fwc = CodecConfig {
            limits: ResourceLimits::permissive(),
            ..CodecConfig::default()
        };
        let mut enc = VaultEncoder::new(fwc.clone(), cfg.clone()).unwrap();
        let mut dec = VaultDecoder::new(fwc, cfg).unwrap();
        println!("== b weight, clip {name}, qp {qp}");
        for f in &frames {
            for p in enc.encode(f).unwrap() {
                let out = dec.decode(&p).unwrap();
                let st = dec.last_stats();
                if st.b_weight_past != 0 {
                    for o in &out {
                        let i = o.frame.pts.ticks as usize;
                        let psnr_v = psnr(&frames[i].pixels[..W * H], &o.pixels[..W * H]);
                        println!(
                            "  B@{i}: w_past={}/4, {} B, {:.2} dB",
                            st.b_weight_past,
                            p.data.len(),
                            psnr_v
                        );
                    }
                }
            }
        }
        for p in enc.flush().unwrap() {
            for o in dec.decode(&p).unwrap() {
                let _ = o;
            }
        }
        }
    }
}
