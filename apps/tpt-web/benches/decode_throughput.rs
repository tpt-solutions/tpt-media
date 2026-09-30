//! Decode-throughput benchmark for the browser player core (todo.md section
//! 45, "performance benchmark").
//!
//! Measures the whole `tpt-web` path on the host: push bytes, parse the
//! container, decode with the real codec, convert to planar `f32` / RGBA8,
//! and pop the output. That is everything the wasm module does per file; the
//! only thing it leaves out is JavaScript and the browser.
//!
//! ```text
//! cargo bench -p tpt-web          # prints a markdown table
//! ```
//!
//! It is a plain `harness = false` timer rather than criterion: the number
//! that matters here is "how many times faster than real time", which is one
//! median over a whole-file decode, not a microbenchmark of a hot loop. The
//! fixtures are generated in memory by the real encoders (deterministic), and
//! generation is excluded from the timings.
//!
//! The same files can be timed as WebAssembly under Node: see
//! `apps/tpt-web/web/bench.mjs`; `docs/performance.md` records both.

use std::time::Instant;
use tpt_web::synth;
use tpt_web::{Player, Status, WebError, WebLimits};

const WARMUP: usize = 2;
const RUNS: usize = 15;
const PUSH_SLICE: usize = 16 * 1024;

/// Decode `file` completely; returns (audio samples, video frames).
fn decode_all(file: &[u8]) -> (u64, u64) {
    let mut p = Player::new(WebLimits::default());
    let (mut samples, mut frames) = (0u64, 0u64);
    let mut drain = |p: &mut Player| {
        while let Some(a) = p.pop_audio() {
            samples += u64::from(a.frames);
        }
        while let Some(v) = p.pop_video() {
            std::hint::black_box(&v.rgba);
            frames += 1;
        }
    };
    for chunk in file.chunks(PUSH_SLICE) {
        loop {
            match p.push(chunk) {
                Ok(()) => break,
                Err(WebError::InputFull) => {
                    p.pump().expect("pump");
                    drain(&mut p);
                }
                Err(e) => panic!("{e}"),
            }
        }
        p.pump().expect("pump");
        drain(&mut p);
    }
    p.finish_input();
    while p.pump().expect("pump") != Status::Ended {
        drain(&mut p);
    }
    drain(&mut p);
    (samples, frames)
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let fixtures = synth::benchmark_fixtures();

    println!(
        "tpt-web native decode throughput ({} runs each after {} warm-up, median; \
         push slices of {} KiB)\n",
        RUNS,
        WARMUP,
        PUSH_SLICE / 1024
    );
    println!("| fixture | file | median | min | realtime factor | input MB/s | video fps |");
    println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: |");
    for fx in &fixtures {
        for _ in 0..WARMUP {
            decode_all(&fx.bytes);
        }
        let mut ms = Vec::with_capacity(RUNS);
        let mut last = (0, 0);
        for _ in 0..RUNS {
            let t = Instant::now();
            last = decode_all(&fx.bytes);
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        assert_eq!(last.1, fx.video_frames, "{}: frame count", fx.name);
        let min = ms.iter().cloned().fold(f64::INFINITY, f64::min);
        let med = median(&mut ms);
        let secs = med / 1000.0;
        let fps = if fx.video_frames > 0 {
            format!("{:.0}", fx.video_frames as f64 / secs)
        } else {
            "-".to_string()
        };
        println!(
            "| {} | {:.1} KiB | {:.2} ms | {:.2} ms | {:.0}x | {:.0} | {} |",
            fx.name,
            fx.bytes.len() as f64 / 1024.0,
            med,
            min,
            fx.media_seconds / secs,
            fx.bytes.len() as f64 / secs / 1e6,
            fps
        );
    }
}
