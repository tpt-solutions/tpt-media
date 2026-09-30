//! todo.md section 42: measured peak working set of Vault's streaming
//! encode + decode, with a counting global allocator.
//!
//! Vault is *not* a real-time codec and claims no zero-allocation path
//! (it allocates a candidate block per RDO evaluation). What this pins is
//! the property a budget actually needs: the peak is **bounded by the
//! reference store and the frame size, and independent of stream length**
//! - no history that grows with the number of frames.
//!
//! The counter is process-global and the test harness runs tests on
//! several threads, so this file contains exactly one test.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static TRACKING: AtomicBool = AtomicBool::new(false);

struct Counting;

fn add(n: usize) {
    if TRACKING.load(Ordering::Relaxed) {
        let now = CURRENT.fetch_add(n, Ordering::Relaxed) + n;
        PEAK.fetch_max(now, Ordering::Relaxed);
    }
}

fn sub(n: usize) {
    if TRACKING.load(Ordering::Relaxed) {
        // Saturating: memory allocated before tracking began may be freed
        // inside the tracked region.
        let _ = CURRENT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |c| {
            Some(c.saturating_sub(n))
        });
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            add(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        sub(layout.size());
        System.dealloc(ptr, layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(layout);
        if !p.is_null() {
            add(layout.size());
        }
        p
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let out = System.realloc(ptr, layout, new_size);
        if !out.is_null() {
            if new_size >= layout.size() {
                add(new_size - layout.size());
            } else {
                sub(layout.size() - new_size);
            }
        }
        out
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

use tpt_media_codec::{CodecConfig, Decoder, Encoder, ResourceLimits};
use tpt_media_core::{PixelFormat, VideoFrame};
use tpt_video_vault::{VaultConfig, VaultDecoder, VaultEncoder};

const W: u32 = 176;
const H: u32 = 144;

fn clip(n: usize) -> Vec<VideoFrame> {
    (0..n)
        .map(|i| {
            let mut f = VideoFrame::alloc(W, H, PixelFormat::Yuv420);
            let (w, h) = (W as usize, H as usize);
            for y in 0..h {
                for x in 0..w {
                    f.pixels[y * w + x] = (((x + i * 3) * 2 + y) % 200) as u8;
                }
            }
            f
        })
        .collect()
}

/// Peak bytes allocated on top of what existed before, for a full
/// encode + decode of `frames` frames with `max_refs` references.
fn peak(frames: usize, max_refs: u8) -> usize {
    let cfg = VaultConfig {
        width: W,
        height: H,
        max_refs,
        ..VaultConfig::default()
    };
    let fw = CodecConfig {
        limits: ResourceLimits::permissive(),
        ..CodecConfig::default()
    };
    let input = clip(frames);
    CURRENT.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    TRACKING.store(true, Ordering::Relaxed);
    {
        let mut enc = VaultEncoder::new(fw.clone(), cfg.clone()).unwrap();
        let mut dec = VaultDecoder::new(fw, cfg).unwrap();
        for f in &input {
            // Nothing is retained: every packet and frame is dropped
            // immediately, as in a streaming pipeline.
            for p in enc.encode(f).unwrap() {
                dec.decode(&p).unwrap();
            }
        }
    }
    TRACKING.store(false, Ordering::Relaxed);
    PEAK.load(Ordering::Relaxed)
}

#[test]
fn peak_working_set_is_bounded_and_independent_of_stream_length() {
    let frame_bytes = (W * H * 3 / 2) as usize;
    // (max_refs, short stream, long stream): the store is full after
    // `max_refs` frames, so both lengths are past the fill point.
    // Measured once at 16 references / 24 frames: 1 402 096 B, i.e.
    // exactly two frames per extra reference (encoder + decoder store) - not
    // re-run here because the unoptimised test profile makes it slow.
    for (max_refs, n_short, n_long) in [(1u8, 3usize, 6usize), (4, 6, 10)] {
        let short = peak(n_short, max_refs);
        let long = peak(n_long, max_refs);
        println!(
            "vault memory {W}x{H} 4:2:0, max_refs {max_refs}: peak {short} B over {n_short} frames, \
             {long} B over {n_long} ({:.1} frames' worth)",
            long as f64 / frame_bytes as f64
        );
        // Independent of stream length: the reference store fills within
        // `max_refs` frames and then stops growing.
        let diff = long.abs_diff(short) as f64 / long as f64;
        assert!(
            diff < 0.05,
            "peak grew with stream length: {short} -> {long}"
        );
        // Bounded by the reference stores (encoder and decoder each hold
        // `max_refs` pictures) plus a fixed number of working pictures and
        // one superblock's candidates.
        let bound = (2 * usize::from(max_refs) + 12) * frame_bytes;
        assert!(long < bound, "peak {long} B exceeds {bound} B");
    }
}
