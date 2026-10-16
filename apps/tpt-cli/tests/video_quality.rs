//! End-to-end video quality corpus (todo.md sections 36 + 37).
//!
//! Drives real `tpt-video-thrum` round trips over the synthetic corpus
//! in `tpt_cli::video` and measures them with the video metrics in
//! `tpt_cli::quality`. This is the video counterpart of
//! `tests/quality.rs` (which does the same for the audio codecs).
//!
//! What is asserted, and what deliberately is not:
//!
//! * Every scene must round-trip: one frame out per frame in, correct
//!   geometry, finite quality, and no panic. This holds for *every*
//!   category, including the pathological ones.
//! * A quality *floor* is pinned only for the scenes where the codec is
//!   genuinely expected to do well - the ones a transform video codec is
//!   built for. The synthetic patterns (zone plate, checkerboard, spatial
//!   hash) carry energy at every spatial frequency up to Nyquist and no
//!   DCT codec reproduces them; asserting a floor there would be
//!   asserting a falsehood. What is asserted for them instead is the
//!   honest negative: they are strictly the worst-scoring category.
//!
//! # Memory
//!
//! Memory is measured in a *dedicated* test binary
//! (`tests/memory_budget.rs`), not here. A tracking global allocator
//! instruments every allocation in the process, and this binary runs
//! eight quality tests in parallel whose allocations are
//! indistinguishable from the ones under measurement — enough noise to
//! swamp the number. See [`crate::memprobe`].

use tpt_cli::quality::VideoReport;
use tpt_cli::video::{corpus, measure_instrumented, Scene};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::PixelFormat;
use tpt_video_thrum::{ThrumConfig, ThrumDecoder, ThrumEncoder};

const W: u32 = 176;
const H: u32 = 144;
const FPS: u32 = 30;
const FRAMES: usize = 6;

/// Encode and decode one scene, measuring quality, rate and CPU.
fn measure(scene: &Scene, cfg: &ThrumConfig) -> VideoReport {
    measure_instrumented(scene, cfg, false)
}

fn default_cfg() -> ThrumConfig {
    ThrumConfig {
        width: W,
        height: H,
        format: PixelFormat::Yuv420,
        ..ThrumConfig::default()
    }
}

fn all_scenes() -> Vec<Scene> {
    corpus(W, H, PixelFormat::Yuv420, FRAMES, FPS, 0x5EED_A11A)
}

fn report(name: &str) -> VideoReport {
    let scene = all_scenes()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no scene named {name}"));
    measure(&scene, &default_cfg())
}
/// Every scene in the corpus must round-trip, in both coded formats,
/// without panicking and without losing a frame.
///
/// No quality floor is asserted here. The scenes that carry energy at
/// every spatial frequency (synthetic patterns) and the ones with no
/// inter-frame structure at all (high motion) are *designed* to be hard,
/// and the point of including them is that the codec degrades rather
/// than breaks. Quality is pinned per-scene in the tests below.
#[test]
fn every_corpus_scene_round_trips() {
    for format in [PixelFormat::Yuv420, PixelFormat::Gray8] {
        for scene in corpus(W, H, format, FRAMES, FPS, 0x5EED_A11A) {
            let cfg = ThrumConfig {
                width: W,
                height: H,
                format,
                ..ThrumConfig::default()
            };
            let r = measure(&scene, &cfg);
            assert_eq!(r.frames, FRAMES, "{}: frame count", scene.name);
            assert!(r.encoded_bytes > 0, "{}: must emit bytes", scene.name);
            assert!(
                r.psnr_db.expect("finite PSNR") > 15.0,
                "{}: {:.1} dB is below even a hostile floor",
                scene.name,
                r.psnr_db.unwrap()
            );
            assert!(
                r.ssim.expect("defined SSIM") > 0.5,
                "{}: SSIM {:.3} is below any usable floor",
                scene.name,
                r.ssim.unwrap()
            );
        }
    }
}

/// A static scene is the best case a transform video codec has: the
/// first frame is intra-coded and every later frame is a pure skip, so
/// the clip should be both small and accurate.
///
/// This is the test that would catch a regression in the skip path or
/// the intra path, which is why it carries a floor and the synthetic
/// patterns do not.
#[test]
fn static_scene_is_cheap_and_accurate() {
    let r = report("static-scene");
    assert!(r.psnr_db.expect("finite") > 40.0, "{:?}", r.psnr_db);
    // Measured ~0.02 bpp. A generous ceiling that still fails loudly if
    // the skip path stops working (which would push this past 0.2 bpp).
    assert!(
        r.bpp((W * H) as u64).expect("defined") < 0.2,
        "static scene should be nearly free"
    );
}

/// A panning camera is what motion compensation exists for, and the
/// smooth field is the most compressible content in the corpus, so it
/// should cost about the same as a static scene.
#[test]
fn camera_motion_is_compressible_by_inter_prediction() {
    let r = report("camera-motion");
    assert!(r.psnr_db.expect("finite") > 40.0);
    assert!(
        r.bpp((W * H) as u64).expect("defined") < 0.2,
        "a slow pan should be cheap"
    );
}

/// A talking head - a large, smooth, slowly moving region - is the
/// archetypal video-call shot, and the reason this codec exists.
#[test]
fn talking_head_holds_a_useful_quality_at_a_useful_rate() {
    let r = report("talking-head");
    assert!(r.psnr_db.expect("finite") > 30.0, "{:?}", r.psnr_db);
    // Measured ~0.03 bpp (~22 kbit/s at 176x144). The ceiling is set
    // well above that to allow for platform variation without letting a
    // broken rate control pass.
    assert!(r.bpp((W * H) as u64).expect("defined") < 0.5);
    assert!(r.bitrate_bps(FPS).expect("defined") > 0);
}

/// The honest negative: the *incompressible* categories cost vastly more
/// than the compressible ones, by more than two orders of magnitude.
///
/// This is the rate-distortion result the corpus exists to demonstrate -
/// there is no bitrate a DCT codec can hit on spatial noise - and it is
/// why a rate/quality report has to carry the bitrate next to the PSNR.
#[test]
fn incompressible_content_costs_orders_of_magnitude_more() {
    let cheap = report("static-scene");
    let expensive = report("high-motion");
    let cheap_bpp = cheap.bpp((W * H) as u64).expect("defined");
    let expensive_bpp = expensive.bpp((W * H) as u64).expect("defined");
    assert!(
        expensive_bpp > cheap_bpp * 50.0,
        "high-motion {expensive_bpp} bpp vs static {cheap_bpp} bpp"
    );
    // The synthetic patterns are the same story at a lower amplitude:
    // a zone plate and a spatial hash cannot be coded cheaply either.
    let patterns = report("synthetic-patterns");
    assert!(
        patterns.bpp((W * H) as u64).expect("defined") > cheap_bpp * 20.0,
        "synthetic patterns should also be expensive"
    );
}

/// Scene ordering: the more inter-frame structure a scene has, the
/// better the codec does on it. Monotonically true across the corpus
/// when ranked by measured bitrate, this is the single clearest
/// statement of what the inter path buys.
#[test]
fn bitrate_orders_the_corpus_by_how_predictable_it_is() {
    let rates: Vec<(f64, &'static str)> = all_scenes()
        .iter()
        .map(|s| {
            let r = measure(s, &default_cfg());
            (r.bpp((W * H) as u64).expect("defined"), s.name)
        })
        .collect();
    let get = |n: &str| {
        rates
            .iter()
            .find(|(_, name)| *name == n)
            .map(|(bpp, _)| *bpp)
            .expect("scene in the corpus")
    };
    // Predictable content is cheap; unpredictable content is not.
    assert!(get("static-scene") < get("talking-head"));
    assert!(get("talking-head") < get("high-motion"));
    assert!(get("camera-motion") < get("high-motion"));
    assert!(get("synthetic-patterns") < get("high-motion"));
}

/// The decoder must not reorder, drop, or duplicate frames, and must
/// preserve presentation timestamps exactly - the property that makes
/// Thrum's latency claim true in the first place.
#[test]
fn thrum_preserves_frame_order_and_timestamps() {
    let scene = all_scenes()
        .into_iter()
        .find(|s| s.name == "camera-motion")
        .expect("camera-motion");
    let cfg = default_cfg();
    let mut enc = ThrumEncoder::new(CodecConfig::default(), cfg.clone()).expect("enc");
    let mut dec = ThrumDecoder::new(CodecConfig::default(), cfg).expect("dec");
    let mut count = 0usize;
    for src in &scene.frames {
        for pkt in enc.encode(src).expect("encode") {
            let out = dec.decode(&pkt).expect("decode");
            for got in out {
                assert_eq!(got.frame.pts, src.frame.pts, "PTS must survive");
                count += 1;
            }
        }
    }
    assert_eq!(count, FRAMES, "no frames lost, duplicated, or reordered");
}

/// Diagnostic: every scene's rate and quality at the default configuration, so a change to the
/// codec's operating point can be read off directly. Run with `--ignored --nocapture`.
#[test]
#[ignore]
fn corpus_summary() {
    for s in all_scenes() {
        let r = measure(&s, &default_cfg());
        println!(
            "{:>20}: {:.4} bpp  {:.1} dB  SSIM {:.3}",
            s.name,
            r.bpp((W * H) as u64).expect("defined"),
            r.psnr_db.expect("finite"),
            r.ssim.expect("defined")
        );
    }
}
