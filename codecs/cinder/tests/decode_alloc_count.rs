//! Counts heap allocation *calls* made by one steady-state Cinder decode,
//! for both the owned `Decoder::decode` and the caller-buffer
//! `RealtimeDecoder::decode_into`, and checks the two produce identical
//! bytes. `realtime_alloc.rs` bounds peak live bytes; this file asserts the
//! stronger property the caller-buffer design (docs/caller-buffer-design.md)
//! is for: zero allocation calls per packet on the real-time path.
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

use tpt_media_cinder::{CinderDecoder, CinderEncoder, CinderEncoderConfig};
use tpt_media_codec::{CodecConfig, Decoder, Encoder, RealtimeDecoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, Packet, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 48_000;
const BLOCK: usize = 512;
const BLOCKS: usize = 32;

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize) {
    CALLS.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    let out = f();
    COUNTING.store(false, Ordering::Relaxed);
    (out, CALLS.load(Ordering::Relaxed))
}

#[test]
fn realtime_decode_is_allocation_free_and_matches_owned_decode() {
    let enc_cfg = CinderEncoderConfig {
        sample_rate: RATE,
        bits_per_coef: 8,
        ..CinderEncoderConfig::default()
    };
    let mut enc = CinderEncoder::new(CodecConfig::default(), enc_cfg.clone()).unwrap();
    let mut owned = CinderDecoder::new(CodecConfig::default(), enc_cfg.clone()).unwrap();
    let mut rt = CinderDecoder::new(CodecConfig::default(), enc_cfg).unwrap();

    // Encode up front, outside every measured region.
    let tb = Timebase::new(1, RATE);
    let mut packets: Vec<Packet> = Vec::new();
    for i in 0..BLOCKS {
        let mut bytes = Vec::with_capacity(BLOCK * 2);
        for j in 0..BLOCK {
            let v = (0.3 * ((i * BLOCK + j) as f32 * 0.05).sin() * 20_000.0) as i16;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let frame = AudioFrame {
            frame: Frame {
                pts: Timestamp {
                    ticks: (i * BLOCK) as i64,
                    timebase: tb,
                },
                duration: None,
                side_data: Vec::new(),
            },
            samples: bytes,
            sample_count: BLOCK as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: RATE,
        };
        packets.extend(enc.encode(&frame).expect("encode"));
    }

    // The caller's output buffer is allocated once, before measuring.
    let mut out_buf = vec![0u8; rt.max_output_bytes()];
    let mut owned_calls = Vec::new();
    let mut rt_calls = Vec::new();

    // A 128-sample stream, decoded by the same decoders after the 256-sample
    // one, exercises the frame-size switch in the overlap state.
    let enc_128 = CinderEncoderConfig {
        sample_rate: RATE,
        bits_per_coef: 8,
        frame_size: 128,
        ..CinderEncoderConfig::default()
    };
    let mut enc_128 = CinderEncoder::new(CodecConfig::default(), enc_128).unwrap();
    let mut packets_128: Vec<Packet> = Vec::new();
    for i in 0..BLOCKS {
        let mut bytes = Vec::with_capacity(BLOCK * 2);
        for j in 0..BLOCK {
            let v = (0.2 * ((i * BLOCK + j) as f32 * 0.09).cos() * 15_000.0) as i16;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let frame = AudioFrame {
            frame: Frame {
                pts: Timestamp {
                    ticks: ((BLOCKS + i) * BLOCK) as i64,
                    timebase: tb,
                },
                duration: None,
                side_data: Vec::new(),
            },
            samples: bytes,
            sample_count: BLOCK as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: RATE,
        };
        packets_128.extend(enc_128.encode(&frame).expect("encode 128"));
    }

    for pkt in packets.iter().chain(packets_128.iter()) {
        // Owned path: the reference output, and its allocation count.
        let (frames, owned_n) = counted(|| owned.decode(pkt).expect("owned decode"));
        let reference: Vec<u8> = frames.iter().flat_map(|f| f.samples.clone()).collect();

        // Real-time path, measured.
        let (rt_n, n_calls) = counted(|| rt.decode_into(pkt, &mut out_buf).expect("decode_into"));

        assert_eq!(
            &out_buf[..rt_n],
            reference.as_slice(),
            "decode_into must produce exactly the owned decode's bytes"
        );
        // The owned path emits nothing while priming; so must the real-time one.
        assert_eq!(rt_n == 0, frames.is_empty());

        // Only packets that emit output count toward the steady state; the
        // first packet primes the overlap and emits nothing.
        if rt_n > 0 {
            owned_calls.push(owned_n);
            rt_calls.push(n_calls);
        }
    }

    let range = |v: &[usize]| {
        (
            v.iter().min().copied().unwrap_or(0),
            v.iter().max().copied().unwrap_or(0),
        )
    };
    let (o_min, o_max) = range(&owned_calls);
    let (r_min, r_max) = range(&rt_calls);
    eprintln!(
        "cinder allocation calls per packet over {} steady-state packets: \
         owned decode min={o_min} max={o_max}; decode_into min={r_min} max={r_max}",
        rt_calls.len()
    );
    assert!(!rt_calls.is_empty(), "no steady-state packets were decoded");
    assert_eq!(
        r_max, 0,
        "decode_into allocated on a steady-state packet (max {r_max} calls)"
    );
}
