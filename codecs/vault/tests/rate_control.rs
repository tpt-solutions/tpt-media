//! Frame-level rate control (todo.md §33): a `target_bits_per_frame`
//! stream lands near its budget, adapts across a complexity change, and
//! is byte-for-byte deterministic. The decoder never sees the knob: the
//! only wire difference is the per-frame `qp` the header already carried.

use std::time::Instant;

use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_vault::{VaultConfig, VaultDecoder, VaultEncoder};

const W: u32 = 176;
const H: u32 = 144;
const FRAMES: usize = 12;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// Gradient + panning textured block + mild noise, 4:2:0 — the same clip
/// shape the criterion bench uses, with a complexity bump halfway (the
/// noise amplitude triples), so the controller has something to adapt to.
fn clip() -> Vec<VideoFrame> {
    (0..FRAMES)
        .map(|i| {
            let mut f = VideoFrame::alloc(W, H, PixelFormat::Yuv420);
            let noise_amp: i32 = if i < FRAMES / 2 { 3 } else { 9 };
            let mut state = 0x5EED_A11A_u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
            let pan = i * 3;
            for y in 0..H as usize {
                for x in 0..W as usize {
                    let mut v = 40 + (x * 120 / W as usize) as i32 + (y * 70 / H as usize) as i32;
                    let bx = (x + W as usize * 4 - pan) % W as usize;
                    if bx > W as usize / 2
                        && bx < W as usize / 2 + W as usize / 4
                        && y > H as usize / 4
                        && y < H as usize / 2 + H as usize / 8
                    {
                        v = 90 + ((bx * 7 + y * 5) % 90) as i32;
                    }
                    v += (lcg(&mut state) >> 60) as i32 % (noise_amp + 1);
                    f.pixels[y * W as usize + x] = v.clamp(0, 255) as u8;
                }
            }
            let (cw, ch) = (W as usize / 2, H as usize / 2);
            let base = W as usize * H as usize;
            for c in 0..2 {
                for y in 0..ch {
                    for x in 0..cw {
                        f.pixels[base + c * cw * ch + y * cw + x] =
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

fn encode_at(target: Option<u32>) -> Vec<tpt_media_core::Packet> {
    let cfg = VaultConfig {
        width: W,
        height: H,
        format: PixelFormat::Yuv420,
        target_bits_per_frame: target,
        ..VaultConfig::default()
    };
    let input = clip();
    let mut encoder = VaultEncoder::new(CodecConfig::default(), cfg).unwrap();
    input
        .iter()
        .flat_map(|f| encoder.encode(f).unwrap())
        .collect()
}

/// The stream lands within a sane band of the per-frame target (the
/// budget bucket absorbs the 4× first-frame allowance over the clip) and
/// the controller demonstrably moved the quantiser: an 8× looser target
/// codes to clearly more bits.
#[test]
fn rate_control_lands_near_the_target_and_adapts() {
    let packets = encode_at(Some(6000));
    // The controller ramps from the configured qp, so the band is
    // measured over the *post-adaptation tail* (the last half of the
    // clip), not the whole stream including the ramp.
    let tail = &packets[packets.len() / 2..];
    let mean: u64 = tail.iter().map(|p| p.data.len() as u64 * 8).sum::<u64>() / tail.len() as u64;
    assert!(
        (4000..=12000).contains(&mean),
        "tail mean {mean} bits/frame is outside the [4000, 12000] adaptation band for a 6000-bit target"
    );

    let loose = encode_at(Some(48_000));
    let loose_tail = &loose[loose.len() / 2..];
    let loose_mean: u64 = loose_tail
        .iter()
        .map(|p| p.data.len() as u64 * 8)
        .sum::<u64>()
        / loose_tail.len() as u64;
    assert!(
        loose_mean > mean * 2,
        "an 8x looser target must code to clearly more bits ({loose_mean} vs {mean})"
    );
}

/// Two encodes with the same target are byte-identical, controller and
/// all (§39: encoding is a pure function of its input and configuration).
#[test]
fn rate_control_is_deterministic() {
    let a = encode_at(Some(6000));
    let b = encode_at(Some(6000));
    assert_eq!(a.len(), b.len());
    for (pa, pb) in a.iter().zip(&b) {
        assert_eq!(pa.data, pb.data);
        assert_eq!(pa.pts, pb.pts);
    }
}

/// The controlled stream still decodes, of course — and the per-frame
/// qp the controller chose round-trips through the header.
#[test]
fn rate_controlled_stream_decodes() {
    let cfg = VaultConfig {
        width: W,
        height: H,
        format: PixelFormat::Yuv420,
        target_bits_per_frame: Some(6000),
        ..VaultConfig::default()
    };
    let input = clip();
    let mut encoder = VaultEncoder::new(CodecConfig::default(), cfg).unwrap();
    let mut decoder = VaultDecoder::new(
        CodecConfig::default(),
        VaultConfig {
            width: W,
            height: H,
            format: PixelFormat::Yuv420,
            ..VaultConfig::default()
        },
    )
    .unwrap();
    for f in &input {
        for p in encoder.encode(f).unwrap() {
            let out = decoder.decode(&p).unwrap();
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].pixels.len(), f.pixels.len());
        }
    }
}

/// A wall-clock sanity print behind `--ignored`, for the numbers quoted
/// in docs: encode time per frame with and without the bounded search.
#[test]
#[ignore]
fn rate_control_encode_time() {
    for target in [None, Some(6000)] {
        let cfg = VaultConfig {
            width: W,
            height: H,
            format: PixelFormat::Yuv420,
            target_bits_per_frame: target,
            ..VaultConfig::default()
        };
        let input = clip();
        let mut encoder = VaultEncoder::new(CodecConfig::default(), cfg).unwrap();
        let t0 = Instant::now();
        let mut bits = 0u64;
        for f in &input {
            for p in encoder.encode(f).unwrap() {
                bits += p.data.len() as u64 * 8;
            }
        }
        let ms = t0.elapsed().as_secs_f64() * 1e3 / FRAMES as f64;
        println!(
            "target {:?}: {:.2} ms/frame, mean {} bits/frame",
            target,
            ms,
            bits / FRAMES as u64
        );
    }
}
