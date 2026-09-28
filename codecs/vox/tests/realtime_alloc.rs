//! §42 "real-time allocation assertions": a counting global allocator
//! verifies that Vox's steady-state encode/decode paths allocate in
//! small, bounded chunks — no allocation proportional to the *number of
//! frames processed*, no accidental multi-megabyte scratch. Vox's decoder
//! claims `real_time_decoder`, so this is the property that claim rests
//! on (same approach as `codecs/pulse/tests/realtime_alloc.rs`).
//!
//! Fidelity caveat: the test harness's other threads allocate too, so the
//! asserted budget is deliberately generous (512 KiB across 32 blocks —
//! orders of magnitude above what Vox actually moves per 320-sample
//! frame). The property guarded is *order-of-magnitude* stability: a
//! per-frame `Vec` growth, a linearly-growing history buffer, or a
//! multi-MB scratch allocation trips this immediately, while ordinary
//! scheduler noise cannot. (Vox's encoder runs a 1024-entry codebook
//! search per unvoiced frame; even that regenerates entries into one
//! fixed scratch frame rather than allocating per candidate.)

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

use tpt_audio_vox::{VoxConfig, VoxDecoder, VoxEncoder};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 16_000;
const BLOCK: usize = 320;
const BLOCKS: usize = 32;

#[test]
fn steady_state_encode_decode_allocations_are_bounded() {
    // Construction (analysis window, config) is explicitly *out of* the
    // measured region: the real-time contract is about the per-frame
    // path.
    let fw = CodecConfig::default();
    let mut encoder = VoxEncoder::new(fw.clone(), VoxConfig::default()).unwrap();
    let mut decoder = VoxDecoder::new(fw, VoxConfig::default()).unwrap();

    let mut state = 0x5EED_A11A_0000_0003u64;
    let mut blocks = Vec::new();
    for i in 0..BLOCKS {
        let mut b = Vec::with_capacity(BLOCK * 2);
        for j in 0..BLOCK {
            let n = i * BLOCK + j;
            let v: f64 = if i % 2 == 0 {
                let t = n as f64 / RATE as f64;
                (core::f64::consts::TAU * 200.0 * t).sin() * 8000.0
            } else {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((state >> 33) as i32 % 16_384 - 8_192) as f64
            };
            let v = v as i16;
            b.extend_from_slice(&v.to_le_bytes());
        }
        blocks.push(AudioFrame {
            frame: Frame {
                pts: Timestamp {
                    ticks: (i * BLOCK) as i64,
                    timebase: Timebase::new(1, RATE),
                },
                duration: None,
                side_data: Vec::new(),
            },
            samples: b,
            sample_count: BLOCK as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: RATE,
        });
    }

    // Warm up (first calls take the fallback/one-time paths), then start
    // counting over a purely steady-state region.
    let mut packets = Vec::new();
    for frame in &blocks {
        packets.extend(encoder.encode(frame).unwrap());
    }
    for pkt in &packets {
        decoder.decode(pkt).unwrap();
    }

    CURRENT.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    TRACKING.store(true, Ordering::Relaxed);

    let mut steady_packets = Vec::new();
    for frame in &blocks {
        steady_packets.extend(encoder.encode(frame).unwrap());
    }
    for pkt in &steady_packets {
        decoder.decode(pkt).unwrap();
    }

    TRACKING.store(false, Ordering::Relaxed);

    let peak = PEAK.load(Ordering::Relaxed);
    assert!(
        peak < 512 * 1024,
        "steady-state peak allocation {peak} bytes exceeds the 512 KiB bound"
    );
}
