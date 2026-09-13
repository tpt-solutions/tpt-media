//! §41 pipeline benchmark: end-to-end pull/drive throughput of a
//! `Source -> map -> Sink` chain over 1 000 packets — the minimal
//! meaningful pipeline, so the number is dominated by the framework's own
//! per-item cost.

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use tpt_media_core::{Packet, Timebase, Timestamp};
use tpt_media_pipeline::{NodeOps, Sink, Source};

fn bench_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("pipeline");
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(2));

    let tb = Timebase {
        num: 1,
        den: 48_000,
    };
    let packets: Vec<Packet> = (0..1000)
        .map(|i| {
            let mut p = Packet::empty(
                1,
                Timestamp {
                    ticks: (i * 480) as i64,
                    timebase: tb,
                },
            );
            p.data = vec![0x5A; 32];
            p
        })
        .collect();

    group.bench_function("drive/1000_packets_source_map_sink", |b| {
        b.iter(|| {
            let mut pipeline = Source::new(black_box(&packets).clone())
                .map(|pkt: Packet| pkt.track_id)
                .then(Sink::new())
                .into_pipeline();
            black_box(pipeline.drive(4096).expect("run completes"))
        })
    });

    group.finish();
}

criterion_group!(benches, bench_pipeline);
criterion_main!(benches);
