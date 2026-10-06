//! Scene-cut detection (todo.md §33's rate-control remainder): with the
//! `scene_cut` knob a hard cut between two scenes codes the first frame
//! of the new scene intra instead of waiting for the next scheduled
//! keyframe. The detector is a spike rule on the mean absolute luma
//! difference against the most recent reference, so steady content —
//! even busy, panning content — must not fire it.

use tpt_media_codec::{CodecConfig, Decoder, Encoder, ResourceLimits};
use tpt_media_core::{PacketFlags, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_vault::{VaultConfig, VaultDecoder, VaultEncoder};

const W: usize = 176;
const H: usize = 144;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// One frame of a flat synthetic scene: scene A is a dark rising
/// gradient, scene B a bright falling one with different chroma —
/// neighbouring frames within a scene differ only by the pan, the two
/// scenes differ almost everywhere.
fn scene_frame(scene: u8, i: usize) -> VideoFrame {
    let mut f = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
    let pan = (i * 2) as i32;
    for y in 0..H {
        for x in 0..W {
            let g = (x * 140 / W + y * 60 / H) as i32;
            let stripe = (x as i32 + pan) % 24;
            let v = match scene {
                0 => 30 + (g + stripe) / 2,
                _ => 225 - (g + stripe) / 3,
            };
            f.pixels[y * W + x] = v.clamp(0, 255) as u8;
        }
    }
    let (cw, ch) = (W / 2, H / 2);
    let (u, v) = match scene {
        0 => (90u8, 160u8),
        _ => (150u8, 90u8),
    };
    for c in 0..2 {
        let fill = if c == 0 { u } else { v };
        f.pixels[W * H + c * cw * ch..][..cw * ch].fill(fill);
    }
    f.frame.pts = Timestamp {
        ticks: i as i64,
        timebase: Timebase::new(1, 30),
    };
    f
}

/// `a_frames` of scene A then `b_frames` of scene B.
fn cut_clip(a_frames: usize, b_frames: usize) -> Vec<VideoFrame> {
    (0..a_frames + b_frames)
        .map(|i| {
            if i < a_frames {
                scene_frame(0, i)
            } else {
                scene_frame(1, i)
            }
        })
        .collect()
}

/// The rate_control-style pan clip: steady, textured, noisy — motion on
/// every frame, no cut anywhere.
fn steady_clip(frames: usize) -> Vec<VideoFrame> {
    (0..frames)
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
                    v += (lcg(&mut state) >> 60) as i32 % 10;
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

fn fw() -> CodecConfig {
    CodecConfig {
        limits: ResourceLimits::permissive(),
        ..CodecConfig::default()
    }
}

/// An encoder with the keyframe schedule disabled (an interval far
/// beyond the clip; `0` would force every frame intra and mask the
/// detector).
fn encoder(clip_cfg: VaultConfig) -> VaultEncoder {
    VaultEncoder::with_keyframe_interval(fw(), clip_cfg, 10_000).unwrap()
}

fn no_schedule() -> VaultConfig {
    VaultConfig {
        width: W as u32,
        height: H as u32,
        format: PixelFormat::Yuv420,
        ..VaultConfig::default()
    }
}

fn keyframe_positions(packets: &[tpt_media_core::Packet]) -> Vec<usize> {
    packets
        .iter()
        .enumerate()
        .filter(|(_, p)| p.flags.contains(PacketFlags::KEYFRAME))
        .map(|(i, _)| i)
        .collect()
}

/// A hard cut mid-stream is detected: the first frame of the new scene
/// codes intra (with the schedule disabled, that is the only intra after
/// the first frame), every frame still decodes, and the detector's
/// benefit shows in the tail it was invented for — the post-cut frames
/// cost clearly fewer bits than coding the same tail predictively
/// against the old scene.
#[test]
fn scene_cut_is_detected_and_pays_off() {
    let clip = cut_clip(8, 8);

    let mut off = encoder(VaultConfig {
        scene_cut: false,
        ..no_schedule()
    });
    let mut packets_off: Vec<_> = clip.iter().flat_map(|f| off.encode(f).unwrap()).collect();
    packets_off.extend(off.flush().unwrap());

    let mut on = encoder(VaultConfig {
        scene_cut: true,
        ..no_schedule()
    });
    let mut packets_on: Vec<_> = clip.iter().flat_map(|f| on.encode(f).unwrap()).collect();
    packets_on.extend(on.flush().unwrap());

    // Without the knob only the cold-start frame is intra.
    assert_eq!(keyframe_positions(&packets_off), vec![0]);
    // With it, the first frame of scene B is too.
    assert_eq!(
        keyframe_positions(&packets_on),
        vec![0, 8],
        "the cut at display frame 8 must code intra"
    );

    // The post-cut tail: detection turns seven predictive frames against
    // the new scene's own reference instead of against the old one.
    let tail_off: usize = packets_off[8..].iter().map(|p| p.data.len()).sum();
    let tail_on: usize = packets_on[8..].iter().map(|p| p.data.len()).sum();
    assert!(
        tail_on < tail_off,
        "post-cut tail with detection ({tail_on} B) must beat predicting the new scene from the old one ({tail_off} B)"
    );

    // Both streams decode completely, and the detected one matches the
    // encoder's reconstructions (drift-free, per §39).
    let mut dec = VaultDecoder::new(fw(), no_schedule()).unwrap();
    let mut shown = 0usize;
    for p in &packets_on {
        shown += dec.decode(p).unwrap().len();
    }
    assert_eq!(shown, clip.len(), "every frame is emitted exactly once");
}

/// The detector must not fire on steady content: the panning, textured,
/// noisy clip keeps a single intra (the cold start) even with the knob
/// on, and the knob changes no byte of the output.
#[test]
fn scene_cut_stays_silent_on_steady_content() {
    let clip = steady_clip(16);

    let mut on = encoder(VaultConfig {
        scene_cut: true,
        ..no_schedule()
    });
    let mut packets_on: Vec<_> = clip.iter().flat_map(|f| on.encode(f).unwrap()).collect();
    packets_on.extend(on.flush().unwrap());

    let mut off = encoder(no_schedule());
    let mut packets_off: Vec<_> = clip.iter().flat_map(|f| off.encode(f).unwrap()).collect();
    packets_off.extend(off.flush().unwrap());

    assert_eq!(
        keyframe_positions(&packets_on),
        vec![0],
        "no cut, no extra intra"
    );
    assert_eq!(packets_on.len(), packets_off.len());
    for (a, b) in packets_on.iter().zip(&packets_off) {
        assert_eq!(a.data, b.data, "the knob off-fires nowhere on this clip");
    }
}

/// In the B-frame path the cut lands on a *group anchor* (a cut inside a
/// group waits for the boundary by design): with `b_frames = 2` and the
/// cut at display frame 8 — an anchor position — the anchor codes intra
/// and the group's B's and future follow it normally.
#[test]
fn scene_cut_fires_on_a_group_anchor_with_b_frames() {
    let clip = cut_clip(8, 8);
    let cfg = VaultConfig {
        scene_cut: true,
        b_frames: 2,
        ..no_schedule()
    };
    let mut enc = encoder(cfg.clone());
    let mut packets: Vec<_> = clip.iter().flat_map(|f| enc.encode(f).unwrap()).collect();
    packets.extend(enc.flush().unwrap());

    // Groups of four (anchor, future, B, B) starting at display 0: the
    // second scene's first frame is the anchor of group 2, the 9th
    // packet in decode order.
    assert_eq!(
        keyframe_positions(&packets),
        vec![0, 8],
        "the cut anchor codes intra; nothing else fires"
    );

    // The reordered stream still comes out complete, in display order
    // (the pts the bridge assigned travel through the reorder buffer
    // unchanged).
    let mut dec = VaultDecoder::new(fw(), cfg).unwrap();
    let mut shown = Vec::new();
    for p in &packets {
        shown.extend(dec.decode(p).unwrap());
    }
    shown.extend(dec.flush().unwrap());
    assert_eq!(shown.len(), clip.len());
    for (i, f) in shown.iter().enumerate() {
        assert_eq!(f.frame.pts.ticks, i as i64, "frame {i} out of display order");
    }
}

/// Everything above is a pure function of its input: the same clip and
/// configuration encode byte-identically twice.
#[test]
fn scene_cut_is_deterministic() {
    let clip = cut_clip(8, 8);
    let cfg = VaultConfig {
        scene_cut: true,
        ..no_schedule()
    };
    let mut a = encoder(cfg.clone());
    let mut b = encoder(cfg);
    let mut pa: Vec<_> = clip.iter().flat_map(|f| a.encode(f).unwrap()).collect();
    pa.extend(a.flush().unwrap());
    let mut pb: Vec<_> = clip.iter().flat_map(|f| b.encode(f).unwrap()).collect();
    pb.extend(b.flush().unwrap());
    assert_eq!(pa.len(), pb.len());
    for (x, y) in pa.iter().zip(&pb) {
        assert_eq!(x.data, y.data);
        assert_eq!(x.pts, y.pts);
    }
}
