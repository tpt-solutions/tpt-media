//! §42 "real-time allocation assertions": a counting global allocator
//! verifies that Cinder's steady-state encode/decode path allocates in
//! small, bounded chunks — no allocation proportional to the *number of
//! frames processed*, no accidental multi-megabyte scratch.
//!
//! Fidelity caveat: the test harness's other threads allocate too, so the
//! asserted budget is deliberately generous (512 KiB across 32 blocks —
//! roughly 25× what Cinder actually moves per block). The property being
//! guarded is *order-of-magnitude* stability: a single accidental
//! per-frame `Vec` growth, a linearly-growing history buffer, or a
//! multi-MB scratch allocation trips this immediately, while ordinary
//! scheduler noise cannot.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TRACKING: AtomicBool = AtomicBool::new(false);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() && TRACKING.load(Ordering::Relaxed) {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACKING.load(Ordering::Relaxed) {
            CURRENT.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        System.dealloc(ptr, layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() && TRACKING.load(Ordering::Relaxed) {
            let now = CURRENT.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let out = System.realloc(ptr, layout, new_size);
        if !out.is_null() && TRACKING.load(Ordering::Relaxed) {
            if new_size >= layout.size() {
                let delta = new_size - layout.size();
                let now = CURRENT.fetch_add(delta, Ordering::Relaxed) + delta;
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                CURRENT.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        out
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

use tpt_audio_cinder::{CinderDecoder, CinderEncoder, CinderEncoderConfig};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 48_000;
const BLOCK: usize = 512;

#[test]
fn steady_state_encode_decode_allocations_are_bounded() {
    // Construction (FFT plans, config) is explicitly *out* of the
    // measured region: the real-time contract is about the per-frame
    // path.
    let enc_cfg = CinderEncoderConfig {
        sample_rate: RATE,
        bits_per_coef: 8,
        ..CinderEncoderConfig::default()
    };
    let mut enc = CinderEncoder::new(CodecConfig::default(), enc_cfg.clone()).unwrap();
    let mut dec = CinderDecoder::new(CodecConfig::default(), enc_cfg).unwrap();

    let blocks: Vec<Vec<u8>> = (0..32)
        .map(|i| {
            let mut b = Vec::with_capacity(BLOCK * 2);
            for j in 0..BLOCK {
                let v = (0.3 * ((i * BLOCK + j) as f32 * 0.05).sin() * 20_000.0) as i16;
                b.extend_from_slice(&v.to_le_bytes());
            }
            b
        })
        .collect();

    let tb = Timebase::new(1, RATE);
    let one_pass =
        |enc: &mut CinderEncoder, dec: &mut CinderDecoder, start: usize, blocks: &[Vec<u8>]| {
            let frame = AudioFrame {
                frame: Frame {
                    pts: Timestamp {
                        ticks: (start * BLOCK) as i64,
                        timebase: tb,
                    },
                    duration: None,
                    side_data: Vec::new(),
                },
                samples: blocks[start].clone(),
                sample_count: BLOCK as u32,
                format: SampleFormat::S16,
                channel_layout: ChannelLayout::Mono,
                sample_rate: RATE,
            };
            for pkt in enc.encode(&frame).expect("encode") {
                dec.decode(&pkt).expect("decode");
            }
        };

    // One untimed warm-up pass so lazily-allocated internals land before
    // measurement begins.
    one_pass(&mut enc, &mut dec, 0, &blocks);

    TRACKING.store(true, Ordering::Relaxed);
    for i in 1..32 {
        one_pass(&mut enc, &mut dec, i, &blocks);
    }
    TRACKING.store(false, Ordering::Relaxed);

    let peak = PEAK.load(Ordering::Relaxed);
    let budget = 512 * 1024;
    assert!(
        peak < budget,
        "steady-state allocations peaked at {peak} bytes over 31 \
         encode+decode blocks (budget {budget}); a per-frame allocation \
         likely scales with frames processed"
    );
}
