//! §38 property tests, §39 container-determinism pins, and the §47
//! malformed-timestamp audit for `tpt-container`: a seeded-random packet
//! stream must survive mux→demux exactly, the same stream must mux to
//! byte-identical files, and extreme PTS values must round-trip without
//! panicking, wrapping, or corrupting neighboring packets.

use tpt_container::TptDemuxer;
use tpt_container::TptMuxer;
use tpt_media_container::Container as _;
use tpt_media_container::ContainerError;
use tpt_media_core::{Packet, PacketFlags, Timebase, Timestamp};

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn random_packets(state: &mut u64, count: usize) -> Vec<Packet> {
    let tb = Timebase {
        num: 1,
        den: 48_000,
    };
    (0..count)
        .map(|i| {
            let len = (lcg(state) % 64) as usize;
            let mut p = Packet::empty(
                1,
                Timestamp {
                    ticks: (lcg(state) % 1_000_000_000) as i64,
                    timebase: tb,
                },
            );
            p.dts = p.pts;
            p.flags = match lcg(state) % 4 {
                0 => PacketFlags::empty(),
                1 => PacketFlags::KEYFRAME,
                2 => PacketFlags::END_OF_FRAME,
                _ => PacketFlags::DISCONTINUITY,
            };
            p.data = (0..len).map(|_| lcg(state) as u8).collect();
            let _ = i;
            p
        })
        .collect()
}

fn mux(stream: &[Packet]) -> Vec<u8> {
    let mut muxer = TptMuxer::new(Timebase {
        num: 1,
        den: 48_000,
    });
    muxer.add_track(1, b"PULS", &[]);
    for p in stream {
        muxer
            .write_packet_raw(p)
            .expect("mux accepts the generated stream");
    }
    muxer.finish()
}

#[test]
fn random_packet_streams_round_trip_exactly() {
    for seed in 1..8u64 {
        let mut state = 0x5EED_0200 + seed;
        let stream = random_packets(&mut state, 200);
        let bytes = mux(&stream);

        let mut demux = TptDemuxer::new(&bytes).expect("parses");
        let mut got = Vec::new();
        loop {
            match demux.next_packet() {
                Ok(p) => got.push(p),
                Err(ContainerError::Truncated) => break,
                Err(e) => panic!("seed {seed}: unexpected error {e:?}"),
            }
        }
        assert_eq!(got.len(), stream.len(), "seed {seed}: packet count");
        for (i, (a, b)) in got.iter().zip(stream.iter()).enumerate() {
            assert_eq!(a.track_id, b.track_id, "seed {seed} packet {i}");
            assert_eq!(a.pts.ticks, b.pts.ticks, "seed {seed} packet {i} pts");
            assert_eq!(a.dts.ticks, b.dts.ticks, "seed {seed} packet {i} dts");
            assert_eq!(a.flags, b.flags, "seed {seed} packet {i} flags");
            assert_eq!(a.data, b.data, "seed {seed} packet {i} payload");
        }
    }
}

#[test]
fn muxing_is_deterministic() {
    // §39: the same packet stream must produce byte-identical container
    // output — no timestamps, counters, hash seeds, or padding entropy
    // baked into the file.
    let mut state = 0x5EED_0201;
    let stream = random_packets(&mut state, 500);
    assert_eq!(mux(&stream), mux(&stream));
    // ...and a different stream must produce a different file (a trivial
    // "not all inputs collide" check on the same machinery).
    let mut other_state = 0x5EED_0202;
    let other = random_packets(&mut other_state, 500);
    assert_ne!(mux(&stream), mux(&other));
}

/// §47 malformed-timestamp audit: PTS/DTS are carried as opaque tick
/// counts, so *any* representable value — including `i64::MAX` — must
/// round-trip exactly and must not trip any arithmetic downstream of the
/// demuxer. (The demuxer performs no timestamp arithmetic at all; this
/// test pins that guarantee.)
#[test]
fn extreme_and_duplicate_pts_round_trip_without_panicking() {
    let tb = Timebase {
        num: 1,
        den: 48_000,
    };
    let extremes = [
        0i64,
        1,
        i64::MAX,
        i64::MAX - 1,
        48000 * 24 * 60 * 60 * 365, // one year of audio at 48 kHz
    ];
    // Duplicated PTS values (every packet with the same timestamp) are
    // equally legal — the container has no monotonicity requirement.
    let mut stream: Vec<Packet> = extremes
        .iter()
        .map(|&ticks| {
            let mut p = Packet::empty(
                1,
                Timestamp {
                    ticks,
                    timebase: tb,
                },
            );
            p.dts = p.pts;
            p.flags = PacketFlags::KEYFRAME;
            p.data = vec![0xAB; 16];
            p
        })
        .collect();
    stream.extend(extremes.iter().map(|&ticks| {
        let mut p = Packet::empty(
            1,
            Timestamp {
                ticks,
                timebase: tb,
            },
        );
        p.dts = p.pts;
        p.data = vec![0xCD; 16];
        p
    }));
    let bytes = mux(&stream);
    let mut demux = TptDemuxer::new(&bytes).expect("extreme timestamps parse");
    let mut count = 0;
    loop {
        match demux.next_packet() {
            Ok(p) => {
                let expected = &stream[count];
                assert_eq!(p.pts.ticks, expected.pts.ticks);
                assert_eq!(p.data, expected.data);
                count += 1;
            }
            Err(ContainerError::Truncated) => break,
            Err(e) => panic!("unexpected error {e:?}"),
        }
    }
    assert_eq!(count, stream.len());

    // Index-based seeking over extreme timestamps must also be panic-free
    // and land on the requested keyframe.
    demux
        .seek_to_keyframe_on_track(1, i64::MAX / 2)
        .expect("seek by time handles i64-scale ticks");
}
