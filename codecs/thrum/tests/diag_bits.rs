//! Diagnostic (ignored by default): where Thrum's bits go on real footage.
//! Run with `cargo test -p tpt-video-thrum --test diag_bits -- --ignored --nocapture`
//! after `python benchmarks/run.py fetch`.

use tpt_media_codec::{CodecConfig, Encoder};
use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_thrum::{ThrumConfig, ThrumEncoder};

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

#[test]
#[ignore]
fn where_the_bits_go() {
    for clip in ["akiyo_cif", "foreman_cif", "mobile_cif"] {
        let frames = load_frames(&format!("../../benchmarks/corpus/video_c/{clip}_100f.y4m"), 20);
        for qp in [26u8, 14] {
            let cfg = ThrumConfig {
                width: W as u32,
                height: H as u32,
                format: PixelFormat::Yuv420,
                qp,
                ..ThrumConfig::default()
            };
            let mut enc = ThrumEncoder::new(CodecConfig::default(), cfg).unwrap();
            let mut bytes = 0usize;
            for (k, f) in frames.iter().enumerate() {
                let mut frame = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
                frame.frame.pts = Timestamp {
                    ticks: k as i64,
                    timebase: Timebase::new(1, 30),
                };
                frame.pixels.copy_from_slice(f);
                bytes += enc.encode(&frame).unwrap().remove(0).data.len();
            }
            let s = enc.stats();
            let total = (s.intra_bits + s.skip_bits + s.mv_bits + s.luma_bits + s.chroma_bits) as f64;
            let pct = |b: u64| 100.0 * b as f64 / total;
            let mbs = (s.intra_mbs + s.skip_mbs + s.inter_mbs) as f64;
            println!(
                "{clip:>12} qp {qp:>2}: {bytes:>7} B | MBs intra {:.0}% skip {:.0}% inter {:.0}% | bits: intra {:.0}% skip {:.0}% mv {:.0}% luma {:.0}% chroma {:.0}%",
                100.0 * s.intra_mbs as f64 / mbs,
                100.0 * s.skip_mbs as f64 / mbs,
                100.0 * s.inter_mbs as f64 / mbs,
                pct(s.intra_bits),
                pct(s.skip_bits),
                pct(s.mv_bits),
                pct(s.luma_bits),
                pct(s.chroma_bits),
            );
        }
    }
}
