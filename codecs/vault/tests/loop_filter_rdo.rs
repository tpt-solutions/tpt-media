//! The RD-decided loop-filter flag (todo.md §33): with `loop_filter_rdo`
//! the encoder chooses the per-frame header bit by comparing the
//! filtered and unfiltered reconstruction of the *same* coded frame
//! against the source, and the decoder — which only ever reads that bit
//! — stays in lock-step either way.
//!
//! What the decision can and cannot see, measured in `measure.rs`'s
//! `loop_filter_rdo_decision` table: on textured content and on pure
//! noise the same-frame choice agrees with the stream-level PSNR the
//! fixed-flag table recorded, but on the smooth panning clip the filter
//! wins per frame (it smooths quantisation staircasing toward the
//! source) even though the stream is better off without it — the
//! unfiltered reference predicts the pan better. That is the documented
//! greedy limitation, not a bug: the decision prices this frame only.

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

/// Gradient + panning textured block + noise, 4:2:0 — the smooth clip
/// `docs/performance.md`'s loop-filter table measured the filter
/// harmful on.
fn panning_clip() -> Vec<VideoFrame> {
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
                    v += (lcg(&mut state) >> 60) as i32 % 4;
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

/// Bilinearly interpolated coarse noise ("texture"), panning 2 px/frame —
/// the clip the same table measured the filter helpful on.
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
            f.frame.pts = Timestamp {
                ticks: i as i64,
                timebase: Timebase::new(1, 30),
            };
            f
        })
        .collect()
}

fn fw() -> CodecConfig {
    CodecConfig {
        limits: ResourceLimits::permissive(),
        ..CodecConfig::default()
    }
}

/// Encode + decode the whole clip with the RD decision on. Returns the
/// packets, the decoded frames, and the per-frame loop-filter flags the
/// decoder observed (one per frame, in decode order).
fn run_rdo(
    clip: &[VideoFrame],
    qp: u8,
) -> (Vec<tpt_media_core::Packet>, Vec<VideoFrame>, Vec<bool>) {
    let cfg = VaultConfig {
        width: W as u32,
        height: H as u32,
        format: PixelFormat::Yuv420,
        qp,
        loop_filter: true,
        loop_filter_rdo: true,
        ..VaultConfig::default()
    };
    let mut enc = VaultEncoder::new(fw(), cfg.clone()).unwrap();
    let mut dec = VaultDecoder::new(fw(), cfg).unwrap();
    let mut packets = Vec::new();
    let mut decoded = Vec::new();
    let mut flags = Vec::new();
    for f in clip {
        for p in enc.encode(f).unwrap() {
            // The reconstruction the encoder kept must be exactly what a
            // decoder produces from the packet — whatever the decided bit
            // was. This is the pin that the patched header bit and the
            // stored reference agree.
            let recon = enc.reconstruction().unwrap();
            let mut out = dec.decode(&p).unwrap();
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].pixels, recon.pixels, "decoder must match the encoder's kept reconstruction");
            flags.push(dec.last_stats().loop_filter);
            decoded.push(out.remove(0));
            packets.push(p);
        }
    }
    (packets, decoded, flags)
}

/// Per-pixel uniform noise, reseeded per frame — the one content shape
/// where deblocking moves *every* edge sample away from the source (the
/// source's own edges are noise), so the same-frame distortion genuinely
/// prefers the filter off, as does the stream.
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

/// On pure noise the same-frame distortion prefers the filter off (the
/// source's edges are noise; smoothing them only moves samples away), so
/// the decision must go against the config's `loop_filter: true`. On the
/// textured clip it prefers on, exactly as the stream-level fixed-flag
/// table did. The smooth panning clip pins nothing here beyond the
/// drift-free round trip: there the same-frame winner (filter on) is the
/// stream-level loser, which a per-frame decision cannot see — recorded
/// in `measure.rs`'s table instead of asserted.
#[test]
fn rdo_decides_both_ways_and_stays_drift_free() {
    let (_, _, noise_flags) = run_rdo(&noise_clip(), 40);
    assert!(
        noise_flags.iter().any(|on| !on),
        "pure noise must decide the filter off somewhere: {noise_flags:?}"
    );

    let (_, _, texture_flags) = run_rdo(&texture_clip(), 38);
    assert!(
        texture_flags.iter().any(|on| *on),
        "the textured clip must keep the filter somewhere: {texture_flags:?}"
    );

    // Every frame of every run above already pinned the decoder against
    // the encoder's kept reconstruction inside `run_rdo`; the smooth clip
    // adds its round trip here so all three content shapes are covered.
    let (_, decoded, _) = run_rdo(&panning_clip(), 46);
    assert_eq!(decoded.len(), FRAMES);
}

/// The decision is a pure function of the frame and the state: two runs
/// produce byte-identical packets.
#[test]
fn rdo_is_deterministic() {
    for (name, clip) in [("smooth", panning_clip()), ("texture", texture_clip())] {
        let (a, _, _) = run_rdo(&clip, 30);
        let (b, _, _) = run_rdo(&clip, 30);
        assert_eq!(a.len(), b.len(), "{name}");
        for (pa, pb) in a.iter().zip(&b) {
            assert_eq!(pa.data, pb.data, "{name}");
        }
    }
}
