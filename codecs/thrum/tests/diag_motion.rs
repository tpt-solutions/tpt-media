//! Diagnostic (ignored by default): how much each motion-search range and
//! intra-refresh setting costs on real footage. Run with
//! `cargo test -p tpt-video-thrum --test diag_motion -- --ignored --nocapture`
//! after `python benchmarks/run.py fetch`.

use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_thrum::{ThrumConfig, ThrumDecoder, ThrumEncoder};

const W: usize = 352;
const H: usize = 288;

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

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let n = W * H;
    let mse: f64 = a[..n]
        .iter()
        .zip(&b[..n])
        .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
        .sum::<f64>()
        / n as f64;
    10.0 * (255.0 * 255.0 / mse.max(1e-9)).log10()
}

fn run(frames: &[Vec<u8>], cfg: ThrumConfig) -> (usize, f64) {
    let fw = CodecConfig::default();
    let mut enc = ThrumEncoder::new(fw.clone(), cfg.clone()).unwrap();
    let mut dec = ThrumDecoder::new(fw, cfg).unwrap();
    let mut bytes = 0usize;
    let mut total_psnr = 0.0;
    for (k, f) in frames.iter().enumerate() {
        let mut frame = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
        frame.frame.pts = Timestamp {
            ticks: k as i64,
            timebase: Timebase::new(1, 30),
        };
        frame.pixels.copy_from_slice(f);
        let p = enc.encode(&frame).unwrap().remove(0);
        bytes += p.data.len();
        let got = dec.decode(&p).unwrap().remove(0);
        total_psnr += psnr(f, &got.pixels);
    }
    (bytes, total_psnr / frames.len() as f64)
}

#[test]
#[ignore]
fn motion_range_and_refresh_costs_on_mobile() {
    let frames = load_frames("../../benchmarks/corpus/video_c/mobile_cif_100f.y4m", 10);
    for (label, search, refresh) in [
        ("range 0", 0u8, 5u8),
        ("range 8 (default)", 8, 5),
        ("range 16", 16, 5),
        ("range 8, refresh 0", 8, 0),
    ] {
        let cfg = ThrumConfig {
            width: W as u32,
            height: H as u32,
            format: PixelFormat::Yuv420,
            qp: 26,
            search_range: search,
            intra_refresh_percent: refresh,
        };
        let (bytes, p) = run(&frames, cfg);
        println!("{label:>22}: {:>7} bytes over 10 frames, mean Y PSNR {p:.2} dB", bytes);
    }
}
