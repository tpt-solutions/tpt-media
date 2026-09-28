//! Measured per-codec memory budgets (todo.md §42, "peak memory
//! measurements" and "allocation profiling"; §37's video `memory`).
//!
//! This is the only test binary that installs a tracking global
//! allocator, and that placement is deliberate. [`tpt_cli::memprobe`]
//! counts every allocation in the *process*, so a measurement taken in a
//! binary that also runs seven other tests in parallel includes whatever
//! those threads happen to be allocating — noise large enough to swamp
//! the number. Isolating the measurement into its own binary, where the
//! only work happening is the measurement, is what makes the numbers
//! below mean something.
//!
//! What is asserted, in order of strictness:
//!
//! 1. each codec's streaming working set fits its documented budget
//!    (`docs/performance.md`, "Memory Budgets") — the table there is
//!    *measured*, not aspirational;
//! 2. the working set does not grow with the length of the input, which
//!    is the property a real-time or streaming claim actually rests on;
//! 3. per-frame allocation churn stays proportional to a frame, so a
//!    codec cannot pass (2) by simply allocating nothing and still be
//!    usable.
//!
//! The absolute numbers are platform- and build-dependent, so every
//! bound is a deliberately generous multiple of the frame size rather
//! than a tight figure. What is being guarded is an order of magnitude.

use std::sync::{Mutex, MutexGuard};

use tpt_audio_cinder::{CinderDecoder, CinderEncoder, CinderEncoderConfig};
use tpt_cli::memprobe::{MemoryProbe, TrackingAlloc};
use tpt_cli::video::{corpus, measure_instrumented, working_set_peak, Scene};
use tpt_media_codec::CodecConfig;
use tpt_media_core::PixelFormat;
use tpt_video_thrum::ThrumConfig;

/// The one global allocator for this binary. See `tpt_cli::memprobe`.
#[global_allocator]
static GLOBAL: TrackingAlloc = TrackingAlloc;

/// Serialises the *whole* binary, not just the measurement windows.
///
/// [`MemoryProbe`] serialises probes against each other, but that is not
/// enough here: cargo runs this file's tests on parallel threads, and a
/// thread that is between its own measurements — generating a corpus,
/// building frames — is still allocating, and those bytes land in
/// whichever probe happens to be open. That is how the first version of
/// this file reported a 1.5 MB "working set" for a codec budgeted at
/// 8 KiB: the peak was the *other tests*, not the codec.
///
/// Holding this for a test's entire body means exactly one test exists
/// in the process at a time, so the only allocations inside a probe are
/// the ones the test itself made. The cost is that this file does not
/// use its cores; for seven memory measurements that is the right trade,
/// and it is why this is a dedicated binary rather than a module inside
/// the quality suite.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Hold for the duration of a test; see [`TEST_LOCK`].
fn serial() -> MutexGuard<'static, ()> {
    // A test that panicked must not wedge the ones after it.
    TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const W: u32 = 176;
const H: u32 = 144;
const FPS: u32 = 30;
const FRAMES: usize = 6;

/// One 176x144 4:2:0 frame is `176*144*1.5 = 38_016` bytes.
const FRAME_BYTES: u64 = 38_016;

/// Thrum's streaming working set fits the budget `docs/performance.md`
/// states, and does not grow with clip length.
///
/// A video decoder hands back a whole frame per call, so one frame's
/// worth is an *inherent* cost, not overhead; the encoder holds one
/// reference frame on top (todo.md §32). The documented budget is
/// "< 1 MiB"; the bound here is 8 frames, which the budget has to clear
/// many times over before it means anything.
#[test]
fn thrum_working_set_fits_budget_and_is_length_independent() {
    let _serial = serial();

    const BOUND: u64 = 8 * FRAME_BYTES;

    let cfg = cfg();
    let short = working_set_peak(&scene_named("talking-head", FRAMES), &cfg);
    assert!(short.peak_bytes > 0, "probe saw no allocation at all");
    assert!(
        short.peak_bytes < BOUND,
        "peak {} B exceeds the {BOUND} B working-set bound",
        short.peak_bytes
    );

    // The property that matters: 8x the input must not cost 8x the
    // memory. A codec keeping a history or an ever-growing buffer passes
    // every quality assertion and is still unusable in a stream.
    let long = working_set_peak(&scene_named("talking-head", FRAMES * 8), &cfg);
    assert!(
        long.peak_bytes <= short.peak_bytes * 2,
        "working set grew {} B -> {} B when the clip went 8x longer",
        short.peak_bytes,
        long.peak_bytes
    );

    // Cumulative churn, by contrast, *should* scale — that is the
    // per-frame allocation the real-time contract bounds, and it is
    // acceptable only because it stays proportional to a frame.
    assert!(
        long.total_bytes > short.total_bytes,
        "total allocation should grow with the clip"
    );
    let per_frame = long.total_bytes / (FRAMES * 8) as u64;
    assert!(
        per_frame <= FRAME_BYTES * 8,
        "per-frame churn {per_frame} B is disproportionate to a {FRAME_BYTES} B frame"
    );
}

/// The §37 video report carries a *measured* memory figure alongside its
/// quality, rate and CPU figures.
///
/// This is the item that was previously unfillable: the report's
/// `memory` field existed and was hardcoded to `None`, so a caller had
/// quality and rate but never the cost. It is `Some` exactly when the
/// process installed the tracking allocator, which is why this assertion
/// lives here and not in `tests/video_quality.rs`.
#[test]
fn video_report_carries_a_measured_memory_figure() {
    let _serial = serial();

    let report = measure_instrumented(&scene_named("static-scene", FRAMES), &cfg(), true);
    let mem = report
        .memory
        .expect("instrumented report must carry memory");
    assert!(
        mem.peak_bytes > 0,
        "probe saw no allocation; is TrackingAlloc still installed?"
    );
    assert!(
        mem.total_bytes >= mem.peak_bytes,
        "cumulative churn {} B cannot be below the {}-byte peak",
        mem.total_bytes,
        mem.peak_bytes
    );
    assert!(mem.bytes_per_frame(report.frames).is_some());
}

/// The un-instrumented path reports no memory rather than a fabricated
/// zero — an absent measurement and a measurement of nothing are
/// different facts, and a caller must be able to tell them apart.
#[test]
fn uninstrumented_report_says_none_not_zero() {
    let _serial = serial();

    let report = measure_instrumented(&scene_named("static-scene", FRAMES), &cfg(), false);
    assert!(
        report.memory.is_none(),
        "uninstrumented run must not claim memory"
    );
}

/// The same measurement for the audio codecs, whose budgets are stated
/// in KiB rather than frame multiples.
///
/// These are the §42 "per-codec working-set table" numbers. They are
/// asserted against the budgets documented in `docs/performance.md` so
/// that table cannot silently drift away from reality.
mod audio {
    use super::*;
    use tpt_audio_aura::{AuraConfig, AuraDecoder, AuraEncoder};
    use tpt_audio_reed::{ReedConfig, ReedDecoder, ReedEncoder};
    use tpt_media_codec::{Decoder, Encoder};
    use tpt_media_core::{AudioFrame, ChannelLayout, Frame, SampleFormat, Timebase, Timestamp};

    const RATE: u32 = 48_000;
    const BLOCK: usize = 1024;
    const FRAMES: usize = 16;

    /// A deterministic voiced-ish block, so the measurement does not
    /// depend on which path a particular signal happens to take.
    fn audio_frame(i: usize, n: usize, rate: u32) -> AudioFrame {
        let samples: Vec<u8> = (0..n)
            .flat_map(|j| {
                let v = ((i * n + j) as f64 * 0.05 * rate as f64 / 48_000.0).sin() * 8000.0;
                (v as i16).to_le_bytes()
            })
            .collect();
        AudioFrame {
            frame: Frame {
                pts: Timestamp {
                    ticks: (i * n) as i64,
                    timebase: Timebase::new(1, rate),
                },
                duration: None,
                side_data: Vec::new(),
            },
            samples,
            sample_count: n as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: rate,
        }
    }

    /// A full encode+decode pass with every intermediate dropped as it
    /// is produced.
    ///
    /// This is the shape the working-set number has to be measured in. A
    /// caller that accumulated the packets would make the peak a
    /// function of clip length and would be measuring itself — an early
    /// version of this test did exactly that and reported a "working
    /// set" of 1.2 MB for a codec budgeted at 32 KiB, which was simply
    /// the test's own `Vec` of 16 packets.
    fn round_trip(
        frames: &[AudioFrame],
        enc: &mut impl Encoder<Frame = AudioFrame>,
        dec: &mut impl Decoder<Frame = AudioFrame>,
    ) {
        for f in frames {
            for p in enc.encode(f).expect("encode") {
                for out in dec.decode(&p).expect("decode") {
                    std::hint::black_box(&out);
                }
            }
        }
    }

    /// Cinder's documented budget is "< 32 KiB". An audio decoder returns
    /// one block of samples per call, so a few KiB of output is inherent
    /// and the budget has to clear it with room to spare.
    #[test]
    fn cinder_working_set_fits_its_documented_budget() {
        let _serial = serial();

        let fw = CodecConfig::default();
        let enc_cfg = CinderEncoderConfig {
            sample_rate: RATE,
            ..CinderEncoderConfig::default()
        };
        let mut enc = CinderEncoder::new(fw.clone(), enc_cfg.clone()).expect("cinder encoder");
        let mut dec = CinderDecoder::new(fw, enc_cfg).expect("cinder decoder");
        let frames: Vec<_> = (0..FRAMES).map(|i| audio_frame(i, BLOCK, RATE)).collect();

        // Warm up, then measure a steady-state pass.
        // Warm up outside the window, then measure a steady-state pass.
        // in which nothing is retained.
        round_trip(&frames, &mut enc, &mut dec);

        let probe = MemoryProbe::start();
        round_trip(&frames, &mut enc, &mut dec);
        let mem = probe.stop();

        const BOUND: u64 = 32 * 1024;
        assert!(mem.peak_bytes > 0, "probe saw no allocation");
        assert!(
            mem.peak_bytes < BOUND,
            "cinder peak {} B exceeds its {BOUND} B budget",
            mem.peak_bytes
        );
    }

    /// Aura's documented budget is "< 256 KiB", the largest of the audio
    /// codecs because its MDCT/LPC working set is.
    #[test]
    fn aura_working_set_fits_its_documented_budget() {
        let _serial = serial();

        let fw = CodecConfig::default();
        let cfg = AuraConfig {
            sample_rate: RATE,
            block_size: BLOCK as u16,
            ..AuraConfig::default()
        };
        let mut enc = AuraEncoder::new(fw.clone(), cfg.clone()).expect("aura encoder");
        let mut dec = AuraDecoder::new(fw, cfg).expect("aura decoder");
        let frames: Vec<_> = (0..FRAMES).map(|i| audio_frame(i, BLOCK, RATE)).collect();

        // Warm up outside the window, then measure a steady-state pass.
        // in which nothing is retained.
        round_trip(&frames, &mut enc, &mut dec);

        let probe = MemoryProbe::start();
        round_trip(&frames, &mut enc, &mut dec);
        let mem = probe.stop();

        const BOUND: u64 = 256 * 1024;
        assert!(mem.peak_bytes > 0, "probe saw no allocation");
        assert!(
            mem.peak_bytes < BOUND,
            "aura peak {} B exceeds its {BOUND} B budget",
            mem.peak_bytes
        );
    }

    /// Reed's documented budget is "< 16 KiB" — the smallest codec here
    /// and the only `no_std` one. It runs at 16 kHz on 320-sample
    /// frames, per its own configuration.
    ///
    /// The budget was originally written as "< 8 KiB" before anything
    /// was measured; the first run of this test measured the steady-state
    /// peak at 8 140 B — 99% of that budget, passing by 52 bytes. A
    /// ceiling the implementation fills is not a budget, so the
    /// documented figure was raised to the next sane power of two rather
    /// than left as a flake in waiting.
    #[test]
    fn reed_working_set_fits_its_documented_budget() {
        let _serial = serial();

        const REED_RATE: u32 = 16_000;
        const N: usize = 320;
        let fw = CodecConfig::default();
        let mut enc = ReedEncoder::new(fw.clone(), ReedConfig::default()).expect("reed encoder");
        let mut dec = ReedDecoder::new(fw, ReedConfig::default()).expect("reed decoder");
        let frames: Vec<_> = (0..FRAMES).map(|i| audio_frame(i, N, REED_RATE)).collect();

        // Warm up outside the window, then measure a steady-state pass.
        // in which nothing is retained.
        round_trip(&frames, &mut enc, &mut dec);

        let probe = MemoryProbe::start();
        round_trip(&frames, &mut enc, &mut dec);
        let mem = probe.stop();

        const BOUND: u64 = 16 * 1024;
        assert!(mem.peak_bytes > 0, "probe saw no allocation");
        assert!(
            mem.peak_bytes < BOUND,
            "reed peak {} B exceeds its {BOUND} B budget",
            mem.peak_bytes
        );
    }
}

/// Thrum's per-frame churn, measured rather than assumed.
///
/// `docs/performance.md` claims 64 KiB of per-frame scratch; this is a
/// bound on what a frame may allocate in practice, expressed in frame
/// multiples so it does not depend on the scratch figure being exact.
#[test]
fn thrum_per_frame_churn_is_bounded() {
    let _serial = serial();

    let report = measure_instrumented(&scene_named("camera-motion", FRAMES * 4), &cfg(), true);
    let mem = report.memory.expect("instrumented");
    let per_frame = mem.bytes_per_frame(report.frames).expect("non-zero frames");
    assert!(
        per_frame <= FRAME_BYTES as f64 * 8.0,
        "per-frame churn {per_frame} B is disproportionate to a {FRAME_BYTES} B frame"
    );
}

fn cfg() -> ThrumConfig {
    ThrumConfig {
        width: W,
        height: H,
        format: PixelFormat::Yuv420,
        ..ThrumConfig::default()
    }
}

fn scene_named(name: &str, frames: usize) -> Scene {
    corpus(W, H, PixelFormat::Yuv420, frames, FPS, 0x5EED_A11A)
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no scene named {name}"))
}
