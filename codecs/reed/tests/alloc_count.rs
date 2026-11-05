//! Counts heap allocation calls per Reed packet for the owned `decode` and the
//! caller-buffer `RealtimeDecoder::decode_into`, and checks the two produce
//! identical bytes. The encoder is not covered here: Reed's own capabilities
//! say its encoder is not claimed real-time-safe (the unvoiced codebook
//! search), so only the decode path is in scope. Single-test binary, so no
//! other thread allocates while the counter runs.

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

use tpt_media_codec::{CodecConfig, Decoder, Encoder, RealtimeDecoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, Packet, SampleFormat, Timebase, Timestamp};
use tpt_media_reed::{ReedConfig, ReedDecoder, ReedEncoder};

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize) {
    CALLS.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    let out = f();
    COUNTING.store(false, Ordering::Relaxed);
    (out, CALLS.load(Ordering::Relaxed))
}

#[test]
fn reed_decode_is_allocation_free_and_matches_owned_decode() {
    let cfg = ReedConfig::default();
    let fs = cfg.frame_size();
    let rate = cfg.sample_rate;
    let tb = Timebase::new(1, rate);
    let mut enc = ReedEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let mut owned = ReedDecoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let mut rt = ReedDecoder::new(CodecConfig::default(), cfg).unwrap();

    // Encode up front, outside every measured region.
    let frames: Vec<AudioFrame> = (0..40)
        .map(|i| {
            let mut bytes = Vec::with_capacity(fs * 2);
            for j in 0..fs {
                let t = (i * fs + j) as f32;
                let v = (0.3 * (t * 0.11).sin() * 12_000.0) as i16;
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            AudioFrame {
                frame: Frame {
                    pts: Timestamp {
                        ticks: (i * fs) as i64,
                        timebase: tb,
                    },
                    duration: None,
                    side_data: Vec::new(),
                },
                samples: bytes,
                sample_count: fs as u32,
                format: SampleFormat::S16,
                channel_layout: ChannelLayout::Mono,
                sample_rate: rate,
            }
        })
        .collect();
    let mut packets: Vec<Packet> = Vec::new();
    for f in &frames {
        packets.extend(enc.encode(f).expect("encode"));
    }

    // The caller's output buffer is allocated once, before measuring.
    let mut out_buf = vec![0u8; rt.max_output_bytes()];
    let mut owned_calls = Vec::new();
    let mut rt_calls = Vec::new();

    for pkt in &packets {
        let (frames_out, owned_n) = counted(|| owned.decode(pkt).expect("owned decode"));
        let reference = frames_out[0].samples.clone();

        let (n, rt_n) = counted(|| rt.decode_into(pkt, &mut out_buf).expect("decode_into"));

        assert_eq!(n, reference.len());
        assert_eq!(
            &out_buf[..n],
            reference.as_slice(),
            "decode_into must produce exactly the owned decode's bytes"
        );
        owned_calls.push(owned_n);
        rt_calls.push(rt_n);
    }

    // Skip the first packet: it creates the stored synthesis state once.
    let steady_owned = &owned_calls[1..];
    let steady_rt = &rt_calls[1..];
    let range = |v: &[usize]| {
        (
            v.iter().min().copied().unwrap_or(0),
            v.iter().max().copied().unwrap_or(0),
        )
    };
    let (o_min, o_max) = range(steady_owned);
    let (r_min, r_max) = range(steady_rt);
    eprintln!(
        "reed decode (frame {fs} samples, {} packets): owned calls per packet \
         min={o_min} max={o_max}; decode_into min={r_min} max={r_max}",
        steady_rt.len()
    );
    assert_eq!(r_max, 0, "decode_into allocated on a steady-state packet");
}
