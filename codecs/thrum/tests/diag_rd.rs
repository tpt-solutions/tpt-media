//! Diagnostic (ignored by default): Thrum's rate-distortion curve on real
//! footage, as `RD,clip,qp,bytes,psnr` lines, so two builds can be compared at
//! equal PSNR. Run in release mode:
//! `cargo test --release -p tpt-video-thrum --test diag_rd -- --ignored --nocapture`
//! after `python benchmarks/run.py fetch`.

use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_thrum::{ThrumConfig, ThrumDecoder, ThrumEncoder};

const W: usize = 352;
const H: usize = 288;
const FRAMES: usize = 20;

fn load_frames(path: &str, count: usize) -> Vec<Vec<u8>> {
    let b = std::fs::read(path).expect("run `python benchmarks/run.py fetch` first");
    let mut i = b.iter().position(|&c| c == b'\n').unwrap() + 1;
    let fs = W * H * 3 / 2;
    let mut out = Vec::new();
    while i < b.len() && out.len() < count {
        let j = i + b[i..].iter().position(|&c| c == b'\n').unwrap() + 1;
        out.push(b[j..j + fs].to_vec());
        i = j + fs;
    }
    out
}

fn psnr_y(a: &[u8], b: &[u8]) -> f64 {
    let n = W * H;
    let mse: f64 = a[..n]
        .iter()
        .zip(&b[..n])
        .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
        .sum::<f64>()
        / n as f64;
    10.0 * (255.0 * 255.0 / mse.max(1e-9)).log10()
}

#[test]
#[ignore]
fn rate_distortion_curves() {
    for clip in ["akiyo_cif", "foreman_cif", "mobile_cif"] {
        let frames = load_frames(&format!("../../benchmarks/corpus/video_c/{clip}_100f.y4m"), FRAMES);
        for qp in [10u8, 16, 22, 28, 34] {
            let cfg = ThrumConfig {
                width: W as u32,
                height: H as u32,
                format: PixelFormat::Yuv420,
                qp,
                ..ThrumConfig::default()
            };
            let mut enc = ThrumEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
            let mut dec = ThrumDecoder::new(CodecConfig::default(), cfg).unwrap();
            let (mut bytes, mut psnr) = (0usize, 0.0f64);
            for (k, f) in frames.iter().enumerate() {
                let mut frame = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
                frame.frame.pts = Timestamp {
                    ticks: k as i64,
                    timebase: Timebase::new(1, 30),
                };
                frame.pixels.copy_from_slice(f);
                let p = enc.encode(&frame).unwrap().remove(0);
                bytes += p.data.len();
                psnr += psnr_y(f, &dec.decode(&p).unwrap().remove(0).pixels);
            }
            println!("RD,{clip},{qp},{bytes},{:.3}", psnr / FRAMES as f64);
        }
    }
}
