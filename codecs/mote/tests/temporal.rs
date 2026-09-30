//! todo.md section 34 "investigate temporal prediction".
//!
//! Question: what does inter-frame prediction buy a dictionary codec, and
//! which form of it is worth having? Mote implements exactly one form -
//! **conditional replenishment** (a delta frame re-codes only the patches
//! the previous reconstruction no longer explains) - and this test
//! measures it against the alternative of coding every frame
//! independently, across the skip threshold that trades rate for quality.
//! Run with `--nocapture` to see the table recorded in
//! `docs/codec-design.md`.
//!
//! Not investigated / not built: motion-compensated prediction (a shifted
//! previous-frame patch as a candidate), and index-delta or context
//! coding of the index stream. Both are listed in the design notes as
//! open, with the reasons.

use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{PacketFlags, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_mote::{compare, Dictionary, MoteConfig, MoteDecoder, MoteEncoder, TrainConfig};

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 33
}

/// Static textured background + per-frame noise (+/-1) + a 24x24 object.
fn camera(i: i64, moving: bool) -> VideoFrame {
    let (w, h) = (320u32, 240u32);
    let mut f = VideoFrame::alloc(w, h, PixelFormat::Gray8);
    let (wu, hu) = (w as usize, h as usize);
    let mut noise = 0x77u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
    let ox = if moving {
        (i as usize * 4) % (wu - 24)
    } else {
        40
    };
    for y in 0..hu {
        for x in 0..wu {
            let mut t = ((x as u64) << 20) ^ y as u64 ^ 0xABCD;
            let tex = (lcg(&mut t) % 11) as i32 - 5;
            let mut v =
                70 + ((x * 3 + y * 2) % 100) as i32 + ((x / 16 + y / 16) % 3) as i32 * 15 + tex;
            v += (lcg(&mut noise) % 3) as i32 - 1;
            if (ox..ox + 24).contains(&x) && (100..124).contains(&y) {
                v = 230;
            }
            f.pixels[y * wu + x] = v.clamp(0, 255) as u8;
        }
    }
    f.frame.pts = Timestamp {
        ticks: i,
        timebase: Timebase::new(1, 1),
    };
    f
}

struct Run {
    mean_bytes: f64,
    mean_psnr: f64,
    deltas: usize,
}

fn run(dict: &Dictionary, temporal: bool, skip_mse: u16, moving: bool) -> Run {
    let c = MoteConfig {
        width: 320,
        height: 240,
        temporal,
        skip_mse,
        ..MoteConfig::default()
    };
    let mut enc =
        MoteEncoder::with_dictionary(CodecConfig::default(), c.clone(), dict.clone(), false)
            .unwrap();
    let mut dec = MoteDecoder::with_dictionary(CodecConfig::default(), c, dict.clone()).unwrap();
    let (mut bytes, mut psnr, mut n, mut deltas) = (0usize, 0.0f64, 0usize, 0usize);
    for i in 1..=12 {
        let f = camera(i, moving);
        for p in enc.encode(&f).unwrap() {
            // Skip the very first (key) frame: it is the same in every mode.
            if i > 1 {
                bytes += p.data.len();
                n += 1;
                if !p.flags.contains(PacketFlags::KEYFRAME) {
                    deltas += 1;
                }
                let out = dec.decode(&p).unwrap().remove(0);
                psnr += compare(&f, &out).unwrap().luma_psnr_db();
            } else {
                dec.decode(&p).unwrap();
            }
        }
    }
    Run {
        mean_bytes: bytes as f64 / n as f64,
        mean_psnr: psnr / n as f64,
        deltas,
    }
}

#[test]
fn conditional_replenishment_is_the_temporal_predictor_worth_having() {
    let train: Vec<_> = (0..3).map(|i| camera(i * 5, true)).collect();
    let dict = Dictionary::train_from_frames(
        &train,
        &TrainConfig {
            entries: 192,
            iterations: 3,
            ..TrainConfig::default()
        },
    )
    .unwrap();

    for moving in [false, true] {
        let scene = if moving {
            "object moving"
        } else {
            "object still "
        };
        let intra = run(&dict, false, 4, moving);
        eprintln!(
            "{scene} | temporal off        : {:>7.0} B/frame, {:.1} dB",
            intra.mean_bytes, intra.mean_psnr
        );
        assert_eq!(intra.deltas, 0);
        let mut prev_bytes = f64::MAX;
        for skip in [0u16, 2, 4, 16, 64] {
            let r = run(&dict, true, skip, moving);
            eprintln!(
                "{scene} | temporal skip_mse={skip:<2}: {:>7.0} B/frame, {:.1} dB",
                r.mean_bytes, r.mean_psnr
            );
            assert!(r.deltas > 0);
            assert!(
                r.mean_bytes < intra.mean_bytes,
                "replenishment must beat independent coding ({scene}, skip {skip})"
            );
            assert!(
                r.mean_bytes <= prev_bytes + 1e-9,
                "a looser skip threshold must not cost more bytes"
            );
            prev_bytes = r.mean_bytes;
        }
    }
}
