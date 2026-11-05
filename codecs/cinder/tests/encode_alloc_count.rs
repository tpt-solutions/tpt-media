//! Counts heap allocation calls made by the Cinder caller-buffer encoder
//! (`RealtimeEncoder`), and checks its packets match the owned `Encoder`
//! output byte for byte, at both frame sizes. Like `decode_alloc_count.rs`,
//! this is a single-test binary so no other thread allocates during the
//! counted calls. Frames are built before counting starts, because building
//! an `AudioFrame` is the caller's allocation, not the codec's.

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

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
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
}

#[global_allocator]
static GLOBAL: Counting = Counting;

use tpt_media_cinder::{CinderEncoder, CinderEncoderConfig};
use tpt_media_codec::{CodecConfig, Encoder, RealtimeEncoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 48_000;
const CHUNK: usize = 512;
const CHUNKS: usize = 32;

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize) {
    CALLS.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    let out = f();
    COUNTING.store(false, Ordering::Relaxed);
    (out, CALLS.load(Ordering::Relaxed))
}

/// Build the input audio up front: `CHUNKS` frames of `CHUNK` samples each.
fn make_frames(tb: Timebase, phase: f32) -> Vec<AudioFrame> {
    (0..CHUNKS)
        .map(|i| {
            let mut bytes = Vec::with_capacity(CHUNK * 2);
            for j in 0..CHUNK {
                let t = (i * CHUNK + j) as f32;
                let v = (0.25 * (t * 0.07 + phase).sin() * 18_000.0) as i16;
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            AudioFrame {
                frame: Frame {
                    pts: Timestamp {
                        ticks: (i * CHUNK) as i64,
                        timebase: tb,
                    },
                    duration: None,
                    side_data: Vec::new(),
                },
                samples: bytes,
                sample_count: CHUNK as u32,
                format: SampleFormat::S16,
                channel_layout: ChannelLayout::Mono,
                sample_rate: RATE,
            }
        })
        .collect()
}

fn check_frame_size(frame_size: u16) {
    let cfg = CinderEncoderConfig {
        sample_rate: RATE,
        bits_per_coef: 8,
        frame_size,
        ..CinderEncoderConfig::default()
    };
    let tb = Timebase::new(1, RATE);
    let frames = make_frames(tb, 0.0);

    // Reference: the owned encoder, end to end, including the drain tail.
    let mut owned = CinderEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let mut reference: Vec<(Vec<u8>, i64)> = Vec::new();
    for f in &frames {
        for p in owned.encode(f).expect("owned encode") {
            reference.push((p.data, p.pts.ticks));
        }
    }
    for p in owned.drain().expect("owned drain") {
        reference.push((p.data, p.pts.ticks));
    }

    // Caller-buffer path. The output buffer is allocated once, before counting.
    let mut rt = CinderEncoder::new(CodecConfig::default(), cfg).unwrap();
    let mut out = vec![0u8; rt.max_packet_bytes()];
    let mut emitted: Vec<(Vec<u8>, i64)> = Vec::new();
    let mut push_calls = Vec::new();
    let mut pop_calls = Vec::new();

    for f in &frames {
        let ((), c) = counted(|| rt.push_frame(f).expect("push_frame"));
        push_calls.push(c);
        loop {
            let (r, c) = counted(|| rt.pop_packet_into(&mut out).expect("pop_packet_into"));
            pop_calls.push(c);
            match r {
                Some((len, pts)) => emitted.push((out[..len].to_vec(), pts.ticks)),
                None => break,
            }
        }
    }
    loop {
        let (r, c) = counted(|| rt.drain_packet_into(&mut out).expect("drain_packet_into"));
        pop_calls.push(c);
        match r {
            Some((len, pts)) => emitted.push((out[..len].to_vec(), pts.ticks)),
            None => break,
        }
    }

    assert_eq!(
        emitted.len(),
        reference.len(),
        "frame_size {frame_size}: packet count differs from the owned encoder"
    );
    for (i, (got, want)) in emitted.iter().zip(&reference).enumerate() {
        assert_eq!(
            got, want,
            "frame_size {frame_size}: packet {i} differs from the owned encoder"
        );
    }

    // The first push may grow `pending` once: it reserves two blocks, and a
    // 512-sample push at 128-sample blocks needs more. The capacity is kept
    // afterwards, so the steady state is measured from the second push on.
    let max_push = push_calls.iter().skip(1).copied().max().unwrap_or(0);
    let max_pop = pop_calls.iter().copied().max().unwrap_or(0);
    eprintln!(
        "cinder encode (frame_size {frame_size}): {} packets; allocation calls per \
         push_frame max={max_push}; per pop/drain call max={max_pop}",
        emitted.len()
    );
    assert_eq!(max_push, 0, "push_frame allocated in steady state");
    assert_eq!(max_pop, 0, "pop/drain allocated in steady state");
}

#[test]
fn realtime_encode_is_allocation_free_and_matches_owned_encode() {
    check_frame_size(256);
    check_frame_size(128);
}
