//! todo.md sections 34 ("benchmark memory") and 42: a counting global
//! allocator measures Mote's resident and steady-state memory with the
//! full 4096-entry dictionary.
//!
//! One `#[test]` on purpose: the counters are process-global, so a second
//! concurrently running test would pollute them.
//!
//! Run `cargo test --release -p tpt-video-mote --test memory -- --nocapture`
//! to see the figures `docs/performance.md` records. The assertions are
//! order-of-magnitude bounds that hold in both profiles.

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
        // Saturating: blocks allocated before tracking began may be freed
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

use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{PixelFormat, VideoFrame};
use tpt_video_mote::{Dictionary, MoteConfig, MoteDecoder, MoteEncoder};

const W: u32 = 160;
const H: u32 = 112;

fn frame(i: usize) -> VideoFrame {
    let mut f = VideoFrame::alloc(W, H, PixelFormat::Yuv420);
    let w = W as usize;
    for y in 0..H as usize {
        for x in 0..w {
            f.pixels[y * w + x] = ((x * 2 + y + (i % 3) * 5) % 220) as u8;
        }
    }
    f
}

fn begin() {
    CURRENT.store(0, Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    TRACKING.store(true, Ordering::Relaxed);
}

fn end() -> (usize, usize) {
    TRACKING.store(false, Ordering::Relaxed);
    (
        CURRENT.load(Ordering::Relaxed),
        PEAK.load(Ordering::Relaxed),
    )
}

#[test]
fn resident_and_steady_state_memory_with_the_full_dictionary() {
    // Synthetic 4096-entry dictionary: memory does not depend on content.
    let mut s = 3u64;
    let bytes: Vec<u8> = (0..4096 * 64)
        .map(|_| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            (s >> 40) as u8
        })
        .collect();
    let dict = Dictionary::from_entries(bytes).unwrap();
    let c = MoteConfig {
        width: W,
        height: H,
        format: PixelFormat::Yuv420,
        ..MoteConfig::default()
    };
    let frames: Vec<_> = (0..6).map(frame).collect();

    // Decoder: warm up outside the tracked region, then measure a
    // steady-state pass with nothing retained.
    let mut enc =
        MoteEncoder::with_dictionary(CodecConfig::default(), c.clone(), dict.clone(), false)
            .unwrap();
    let packets: Vec<_> = frames.iter().flat_map(|f| enc.encode(f).unwrap()).collect();

    begin();
    let mut dec =
        MoteDecoder::with_dictionary(CodecConfig::default(), c.clone(), dict.clone()).unwrap();
    let (dec_resident_ctor, _) = (CURRENT.load(Ordering::Relaxed), 0);
    let mut warm = Vec::new();
    for p in &packets {
        warm.extend(dec.decode(p).unwrap());
    }
    drop(warm);
    let (dec_resident, dec_peak) = end();

    begin();
    for p in &packets {
        drop(dec.decode(p).unwrap());
    }
    let (_, dec_steady_peak) = end();

    // Encoder steady state (dictionary already installed).
    begin();
    let mut enc2 =
        MoteEncoder::with_dictionary(CodecConfig::default(), c, dict.clone(), false).unwrap();
    let (enc_ctor, _) = (CURRENT.load(Ordering::Relaxed), 0);
    for f in &frames {
        drop(enc2.encode(f).unwrap());
    }
    let (enc_resident, enc_peak) = end();

    let frame_bytes = (W * H * 3 / 2) as usize;
    eprintln!(
        "memory @ {W}x{H} 4:2:0, 4096-entry dictionary ({} B):\n  \
         decoder: constructed {dec_resident_ctor} B, resident after stream {dec_resident} B, \
         peak while decoding {dec_peak} B, steady-state peak {dec_steady_peak} B\n  \
         encoder: constructed {enc_ctor} B, resident after stream {enc_resident} B, peak {enc_peak} B\n  \
         one frame = {frame_bytes} B",
        dict.memory_bytes()
    );

    let dict_bytes = dict.memory_bytes();
    // The decoder is the dictionary plus a few frames' worth of pictures.
    assert!(dec_steady_peak < dict_bytes + 12 * frame_bytes);
    assert!(dec_resident < dict_bytes + 12 * frame_bytes);
    // The encoder additionally holds the f32 search copy (4x the dictionary).
    assert!(enc_peak < 5 * dict_bytes + 64 * frame_bytes + (1 << 20));
}
