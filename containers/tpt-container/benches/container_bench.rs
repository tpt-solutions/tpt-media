//! §41 container benchmarks: `.tpt` mux and demux throughput over a
//! realistic 200-packet stream (~30-byte payloads, monotonic PTS).

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use tpt_container::{TptDemuxer, TptMuxer};
use tpt_media_container::Container as _;
use tpt_media_core::{Packet, Timebase, Timestamp};

const PACKETS: usize = 200;

fn stream() -> Vec<Packet> {
    let tb = Timebase {
        num: 1,
        den: 48_000,
    };
    let mut state = 0x5EED_C0D7u64;
    (0..PACKETS)
        .map(|i| {
            let len = 8 + (state % 48) as usize;
            let mut p = Packet::empty(
                1,
                Timestamp {
                    ticks: (i * 480) as i64,
                    timebase: tb,
                },
            );
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            p.data = (0..len).map(|_| state as u8).collect();
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            p.flags = tpt_media_core::PacketFlags::KEYFRAME;
            p
        })
        .collect()
}

fn bench_container(c: &mut Criterion) {
    let mut group = c.benchmark_group("tpt_container");
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(2));

    let packets = stream();

    group.bench_function("mux/200_packets", |b| {
        b.iter(|| {
            let mut muxer = TptMuxer::new(Timebase {
                num: 1,
                den: 48_000,
            });
            muxer.add_track(1, b"PULS", &[]);
            for p in black_box(&packets) {
                muxer.write_packet_raw(p).expect("mux");
            }
            black_box(muxer.finish())
        })
    });

    let bytes = {
        let mut muxer = TptMuxer::new(Timebase {
            num: 1,
            den: 48_000,
        });
        muxer.add_track(1, b"PULS", &[]);
        for p in &packets {
            muxer.write_packet_raw(p).expect("mux");
        }
        muxer.finish()
    };

    group.bench_function("demux/200_packets", |b| {
        b.iter(|| {
            let mut demux = TptDemuxer::new(black_box(&bytes)).expect("parse");
            let mut count = 0usize;
            loop {
                match demux.next_packet() {
                    Ok(_) => count += 1,
                    Err(tpt_media_container::ContainerError::Truncated) => break,
                    Err(e) => panic!("unexpected {e:?}"),
                }
            }
            assert_eq!(count, PACKETS);
        })
    });

    group.finish();
}

criterion_group!(benches, bench_container);
criterion_main!(benches);
