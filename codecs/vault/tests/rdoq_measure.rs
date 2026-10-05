//! RDOQ on/off measurement, behind the `rdoq` knob's default-off decision
//! (todo.md §33). Not a pass/fail test: `#[ignore]`d, prints only.
//!
//! ```text
//! cargo test --release -p tpt-video-vault --test rdoq_measure -- --ignored --nocapture
//! ```
//!
//! Rates and PSNRs are deterministic; milliseconds are machine-dependent.

use std::time::Instant;

use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_vault::{VaultConfig, VaultEncoder};

const W: u32 = 176;
const H: u32 = 144;
const FRAMES: usize = 8;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// The same clip shape the criterion bench and `measure.rs` use:
/// gradient + panning textured block + mild noise, 4:2:0.
fn clip() -> Vec<VideoFrame> {
    (0..FRAMES)
        .map(|i| {
            let mut f = VideoFrame::alloc(W, H, PixelFormat::Yuv420);
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
                    v += (lcg(&mut state) >> 60) as i32 & 3;
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

fn run(qp: u8, rdoq: bool) -> (f64, usize, f64) {
    let cfg = VaultConfig {
        width: W,
        height: H,
        format: PixelFormat::Yuv420,
        qp,
        rdoq,
        ..VaultConfig::default()
    };
    let input = clip();
    let mut encoder = VaultEncoder::new(CodecConfig::default(), cfg).unwrap();
    let mut decoder = tpt_video_vault::VaultDecoder::new(
        CodecConfig::default(),
        VaultConfig {
            width: W,
            height: H,
            format: PixelFormat::Yuv420,
            ..VaultConfig::default()
        },
    )
    .unwrap();
    let luma = (W * H) as usize;
    let t0 = Instant::now();
    let mut bytes = 0usize;
    let mut psnr_acc = 0.0;
    for f in &input {
        let p = encoder.encode(f).unwrap().remove(0);
        let out = decoder.decode(&p).unwrap().remove(0);
        bytes += p.data.len();
        let mse: f64 = f.pixels[..luma]
            .iter()
            .zip(&out.pixels[..luma])
            .map(|(a, b)| {
                let d = f64::from(*a) - f64::from(*b);
                d * d
            })
            .sum::<f64>()
            / luma as f64;
        psnr_acc += if mse == 0.0 {
            99.0
        } else {
            10.0 * (255.0 * 255.0 / mse).log10()
        };
    }
    (
        psnr_acc / FRAMES as f64,
        bytes / FRAMES,
        t0.elapsed().as_secs_f64() * 1e3 / FRAMES as f64,
    )
}

#[test]
#[ignore]
fn rdoq_on_off_at_two_operating_points() {
    for qp in [30u8, 12] {
        let (psnr_off, bytes_off, ms_off) = run(qp, false);
        let (psnr_on, bytes_on, ms_on) = run(qp, true);
        println!(
            "qp {qp}: off = {psnr_off:.2} dB / {bytes_off} B / {ms_off:.1} ms   \
             on = {psnr_on:.2} dB / {bytes_on} B / {ms_on:.1} ms   \
             (rate {:+.1}%, psnr {:+.3} dB, time {:+.0}%)",
            100.0 * (bytes_on as f64 - bytes_off as f64) / bytes_off as f64,
            psnr_on - psnr_off,
            100.0 * (ms_on - ms_off) / ms_off,
        );
    }
}
