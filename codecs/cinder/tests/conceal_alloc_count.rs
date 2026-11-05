//! Counts heap allocation calls made by Cinder packet-loss concealment, for
//! the owned `CinderDecoder::conceal` and the caller-buffer
//! `CinderDecoder::conceal_into`, and checks the two produce identical bytes.
//! Covers the stream-start fallback, the pitch path on a tone, the spectrum
//! fallback on noise, and a frame-size switch. Single-test binary, so no other
//! thread allocates while the counter runs.

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

fn counted<T>(f: impl FnOnce() -> T) -> (T, usize) {
    CALLS.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    let out = f();
    COUNTING.store(false, Ordering::Relaxed);
    (out, CALLS.load(Ordering::Relaxed))
}

fn encode_stream(frame_size: u16, sample: impl Fn(usize) -> f32, blocks: usize) -> Vec<Packet> {
    let cfg = CinderEncoderConfig {
        sample_rate: RATE,
        bits_per_coef: 8,
        frame_size,
        ..CinderEncoderConfig::default()
    };
    let mut enc = CinderEncoder::new(CodecConfig::default(), cfg).unwrap();
    let tb = Timebase::new(1, RATE);
    let mut packets = Vec::new();
    for i in 0..blocks {
        let mut bytes = Vec::with_capacity(BLOCK * 2);
        for j in 0..BLOCK {
            let v = (sample(i * BLOCK + j) * 20_000.0) as i16;
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
    packets
}

#[test]
fn concealment_is_allocation_free_and_matches_owned_conceal() {
    let tone = |t: usize| 0.3 * (t as f32 * 0.05).sin();
    // Deterministic noise: a hash of the sample index, no RNG state.
    let noise = |t: usize| {
        let h = (t as u32).wrapping_mul(2654435761) ^ 0x9E37_79B9;
        (h as f32 / u32::MAX as f32) * 0.6 - 0.3
    };
    let tone_256 = encode_stream(256, tone, 24);
    let noise_128 = encode_stream(128, noise, 24);

    let cfg = |fs: u16| CinderEncoderConfig {
        sample_rate: RATE,
        bits_per_coef: 8,
        frame_size: fs,
        ..CinderEncoderConfig::default()
    };
    let mut owned = CinderDecoder::new(CodecConfig::default(), cfg(256)).unwrap();
    let mut rt = CinderDecoder::new(CodecConfig::default(), cfg(256)).unwrap();
    let mut out_buf = vec![0u8; rt.max_output_bytes()];

    // Each step: a real packet (decoded into both), or a loss (concealed by
    // both). Losses before the first packet take the spectrum fallback with
    // no history; the tone then exercises pitch concealment; the noise run
    // exercises the spectrum fallback; the last run switches frame size.
    enum Step<'a> {
        Packet(&'a Packet),
        Lost,
    }
    let mut steps: Vec<Step> = Vec::new();
    steps.push(Step::Lost);
    steps.push(Step::Lost);
    for (i, p) in tone_256.iter().enumerate() {
        if (10..14).contains(&i) {
            steps.push(Step::Lost);
        } else {
            steps.push(Step::Packet(p));
        }
    }
    steps.push(Step::Lost);
    steps.push(Step::Lost);
    for p in noise_128.iter().take(12) {
        steps.push(Step::Packet(p));
    }
    for _ in 0..3 {
        steps.push(Step::Lost);
    }

    let mut lost_calls = Vec::new();
    let mut lost_count = 0usize;
    let mut pts_seq = 0i64;
    for step in &steps {
        match step {
            Step::Packet(p) => {
                owned.decode(p).expect("owned decode");
                rt.decode_into(p, &mut out_buf).expect("decode_into");
            }
            Step::Lost => {
                let pts = Timestamp {
                    ticks: pts_seq,
                    timebase: Timebase::new(1, RATE),
                };
                pts_seq += BLOCK as i64;
                let reference = owned.conceal(pts).expect("owned conceal");
                let (n, calls) = counted(|| rt.conceal_into(&mut out_buf).expect("conceal_into"));
                assert_eq!(
                    &out_buf[..n],
                    reference.samples.as_slice(),
                    "conceal_into must produce exactly the owned conceal's bytes"
                );
                lost_calls.push(calls);
                lost_count += 1;
            }
        }
    }

    let max = lost_calls.iter().copied().max().unwrap_or(0);
    eprintln!(
        "cinder allocation calls per concealed block over {lost_count} losses: \
         conceal_into max={max}"
    );
    assert!(lost_count > 0, "no losses were concealed");
    assert_eq!(max, 0, "conceal_into allocated (max {max} calls per block)");
}
