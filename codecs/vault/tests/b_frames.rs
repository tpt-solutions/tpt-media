//! B-frame reconstruction quality — the regression pin for the v0.2
//! B-leaf defect. The first B-frame implementation wrote the skip bit
//! and the four motion vectors but *no coefficient tiles* (the writer
//! returned before the shared coefficient tail), and priced its
//! candidates against single-reference predictions the decoder never
//! produces: B frames decoded prediction-only at a 7-12 dB discount
//! while the encoder believed it had bought full quality. The wire is
//! unchanged by the fix — the B leaves simply carry their coefficients,
//! and the candidates are priced against exactly the predictions the
//! decoder renders — so this test pins the user-visible invariant: a
//! `b_frames` stream decodes every frame at the quality of the same
//! clip coded without B frames.

use tpt_media_codec::{CodecConfig, Decoder, Encoder, ResourceLimits};
use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_vault::{VaultConfig, VaultDecoder, VaultEncoder};

const W: usize = 176;
const H: usize = 144;
const FRAMES: usize = 9;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// Gradient + panning textured block + mild noise, 4:2:0 — motion on
/// every frame, so bi-prediction has real work to do.
fn clip() -> Vec<VideoFrame> {
    (0..FRAMES)
        .map(|i| {
            let mut f = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
            let mut state = 0x5EED_A11A_u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
            let pan = i * 4;
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
            f.pixels[W * H..].fill(128);
            f.frame.pts = Timestamp {
                ticks: i as i64,
                timebase: Timebase::new(1, 30),
            };
            f
        })
        .collect()
}

/// Encode + decode a whole stream (flush included); returns each display
/// position's luma PSNR against the source.
fn decode_quality(b_frames: usize) -> Vec<f64> {
    let cfg = VaultConfig {
        width: W as u32,
        height: H as u32,
        format: PixelFormat::Yuv420,
        qp: 26,
        b_frames,
        ..VaultConfig::default()
    };
    let fw = CodecConfig {
        limits: ResourceLimits::permissive(),
        ..CodecConfig::default()
    };
    let mut enc = VaultEncoder::new(fw.clone(), cfg.clone()).unwrap();
    let mut dec = VaultDecoder::new(fw, cfg).unwrap();
    let input = clip();
    let mut packets = Vec::new();
    for f in &input {
        packets.extend(Encoder::encode(&mut enc, f).unwrap());
    }
    packets.extend(Encoder::flush(&mut enc).unwrap());
    // Display order by construction: the decoder reorders B frames by
    // poc, so decoded pts identify the source frame — never compare
    // against the frame a packet happened to be produced from.
    let mut psnr = vec![0.0; FRAMES];
    let mut shown = 0usize;
    for p in &packets {
        for fr in Decoder::decode(&mut dec, p).unwrap() {
            let i = fr.frame.pts.ticks as usize;
            let mse = input[i].pixels[..W * H]
                .iter()
                .zip(&fr.pixels[..W * H])
                .map(|(a, b)| {
                    let d = f64::from(*a) - f64::from(*b);
                    d * d
                })
                .sum::<f64>()
                / (W * H) as f64;
            psnr[i] = if mse == 0.0 {
                99.0
            } else {
                10.0 * (255.0f64 * 255.0 / mse).log10()
            };
            shown += 1;
        }
    }
    for fr in Decoder::flush(&mut dec).unwrap() {
        shown += 1;
        let _ = fr;
    }
    assert_eq!(shown, FRAMES, "every frame decoded exactly once");
    psnr
}

/// Every frame of the `b_frames = 2` stream decodes within 3 dB of the
/// same clip coded as a plain I/P stream. (Post-fix the observed worst
/// gap is under 1 dB; pre-fix the B frames sat 7-12 dB below it.)
#[test]
fn b_frames_decode_at_reference_quality() {
    let p_only = decode_quality(0);
    let with_b = decode_quality(2);
    for (i, (p, b)) in p_only.iter().zip(with_b.iter()).enumerate() {
        let (p, b) = (*p, *b);
        assert!(
            b >= p - 3.0,
            "frame {i}: B-stream PSNR {b:.2} dB is more than 3 dB below the I/P stream's {p:.2} dB"
        );
    }
}
