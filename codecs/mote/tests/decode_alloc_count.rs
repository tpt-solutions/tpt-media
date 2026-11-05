//! Counts heap allocation *calls* made while Mote's caller-buffer paths run:
//! `RealtimeDecoder::decode_into` for every frame packet of a clip, and
//! `MoteDecoder::conceal_into`. It asserts zero calls, and that both produce
//! the same bytes as the owned `Decoder::decode` and `conceal` on an identical
//! stream. `memory.rs` bounds Mote's resident and peak memory; this file asserts
//! the stronger property the caller-buffer design
//! (docs/caller-buffer-design.md) is for: no allocation per frame packet.
//!
//! The first packet of an embedded-dictionary stream installs the dictionary,
//! which is stream state rather than per-frame work. It is decoded during
//! warm-up, outside the measured region.
//!
//! This is a single-test binary, so no other thread allocates while the
//! counter runs.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static COUNTING: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(layout)
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc_zeroed(layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            CALLS.fetch_add(1, Ordering::Relaxed);
        }
        System.realloc(ptr, layout, new_size)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

use tpt_media_codec::{CodecConfig, Decoder, Encoder, RealtimeDecoder};
use tpt_media_core::{Packet, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_mote::{MoteConfig, MoteDecoder, MoteEncoder};

const W: u32 = 160;
const H: u32 = 112;
const FRAMES: usize = 12;

fn cfg() -> MoteConfig {
    MoteConfig {
        width: W,
        height: H,
        format: PixelFormat::Yuv420,
        ..MoteConfig::default()
    }
}

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

/// Run `f` with allocation counting on and return its result and the number
/// of allocation calls it made.
fn counted<T>(f: impl FnOnce() -> T) -> (T, usize) {
    CALLS.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    let out = f();
    COUNTING.store(false, Ordering::Relaxed);
    (out, CALLS.load(Ordering::Relaxed))
}

#[test]
fn realtime_decode_and_conceal_are_allocation_free_and_match_owned() {
    // Positive control: the counter must see an ordinary allocation, or a
    // zero count below proves nothing.
    let (_, control) = counted(|| vec![1u8; 4096]);
    assert!(
        control >= 1,
        "allocation counter is not observing allocations"
    );

    let fw = CodecConfig::default();
    let mut encoder = MoteEncoder::new(fw.clone(), cfg()).unwrap();
    let mut packets: Vec<Packet> = Vec::new();
    for i in 0..FRAMES {
        packets.extend(encoder.encode(&frame(i)).unwrap());
    }
    assert!(packets.len() > 1, "encoder produced too few packets");

    let mut owned = MoteDecoder::new(fw.clone(), cfg()).unwrap();
    let mut rt = MoteDecoder::new(fw.clone(), cfg()).unwrap();
    let frame_bytes = rt.max_output_bytes();
    let mut out = vec![0u8; frame_bytes];

    // Warm-up: the dictionary-install packet. Its output is empty on both paths.
    assert!(owned.decode(&packets[0]).unwrap().is_empty());
    assert_eq!(rt.decode_into(&packets[0], &mut out).unwrap(), 0);

    let mut owned_bytes: Vec<Vec<u8>> = Vec::new();
    let mut rt_bytes: Vec<Vec<u8>> = Vec::new();
    let mut total_calls = 0usize;
    for pkt in &packets[1..] {
        owned_bytes.push(owned.decode(pkt).unwrap().remove(0).pixels);

        let (res, calls) = counted(|| rt.decode_into(pkt, &mut out));
        assert_eq!(res.unwrap(), frame_bytes);
        total_calls += calls;
        rt_bytes.push(out.clone());
    }

    assert_eq!(
        total_calls,
        0,
        "decode_into made {total_calls} allocation calls across {} frame packets",
        packets.len() - 1
    );
    assert_eq!(
        owned_bytes, rt_bytes,
        "decode_into must produce the same bytes as decode"
    );

    // Concealment on a decoder with no reference: the grey start.
    let mut fresh = MoteDecoder::new(fw, cfg()).unwrap();
    let mut grey = vec![0u8; frame_bytes];
    let (res, calls) = counted(|| fresh.conceal_into(&mut grey));
    assert_eq!(res.unwrap(), frame_bytes);
    assert_eq!(calls, 0, "conceal_into (no reference) allocated");
    assert!(grey.iter().all(|&b| b == 128), "grey start must be 128");

    // Concealment after decoding: repeats the last decoded frame.
    let pts = Timestamp {
        ticks: 0,
        timebase: Timebase::new(1, 1000),
    };
    let last_owned = owned.conceal(pts).unwrap().pixels;
    let (res, calls) = counted(|| rt.conceal_into(&mut out));
    assert_eq!(res.unwrap(), frame_bytes);
    assert_eq!(calls, 0, "conceal_into (with reference) allocated");
    assert_eq!(out, last_owned, "conceal_into must match conceal");
}
