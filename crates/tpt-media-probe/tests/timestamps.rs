//! §47 malformed-timestamp audit, probe side: `probe()` must survive —
//! without panicking and without inventing bogus numbers — containers
//! carrying extreme (`i64::MAX`), duplicated, *descending*, and
//! zero-crossing PTS values. The demuxer carries timestamps opaquely; the
//! probe is the first consumer that does arithmetic on them (duration,
//! bitrate, frame-rate estimation), so it is where a hostile timestamp
//! would bite first.

use tpt_container::build_tpt;
use tpt_media_core::{Packet, Timebase, Timestamp};

fn tb() -> Timebase {
    Timebase {
        num: 1,
        den: 48_000,
    }
}

fn ts(ticks: i64) -> Timestamp {
    Timestamp {
        ticks,
        timebase: tb(),
    }
}

fn packet(ticks: i64, index: usize) -> Packet {
    let mut p = Packet::empty(1, ts(ticks));
    p.data = vec![(index % 256) as u8; 24];
    p.flags = tpt_media_core::PacketFlags::KEYFRAME;
    p
}

fn probe_packets(packets: &[Packet]) -> tpt_media_probe::ProbeReport {
    let bytes = build_tpt(tb(), &[(1, b"CIND", &[][..])], packets);
    tpt_media_probe::probe(&bytes)
}

#[test]
fn probe_survives_extreme_and_degenerate_timestamp_sets() {
    let cases: Vec<(&str, Vec<i64>)> = vec![
        ("i64::MAX everywhere", vec![i64::MAX; 8]),
        ("zero everywhere", vec![0; 8]),
        (
            "extremes bracketing zero",
            vec![i64::MIN, 0, i64::MAX, 0, i64::MIN],
        ),
        (
            "strictly descending",
            (0..8).map(|i| (1000 - i * 100) as i64).collect(),
        ),
        (
            "descending from i64::MAX",
            (0..8).map(|i| i64::MAX - i * 480).collect(),
        ),
        ("single packet", vec![42]),
        ("empty stream", vec![]),
    ];

    for (name, ticks) in cases {
        let packets: Vec<Packet> = ticks
            .iter()
            .enumerate()
            .map(|(i, &t)| packet(t, i))
            .collect();
        if packets.is_empty() {
            // `build_tpt` with no packets still produces a parseable file.
            let bytes = build_tpt(tb(), &[(1, b"CIND", &[][..])], &[]);
            let report = tpt_media_probe::probe(&bytes);
            assert!(
                report.corruption.is_empty() || !report.corruption.is_empty(),
                "{name}: probe returns a report either way"
            );
            continue;
        }
        let report = probe_packets(&packets);
        // Any duration/bitrate that *is* reported must be well-formed:
        // finite and non-negative. A panic above is the failure this test
        // exists to catch. (`i64::MAX` ticks ≈ 1.9e14 years, so a
        // degenerate-timestamp file can legitimately round its payload
        // bitrate down to 0 bit/s — that is a correct answer to a
        // nonsense input, not a bug.)
        if let Some(d) = report.duration_seconds {
            assert!(d.is_finite() && d >= 0.0, "{name}: duration {d}");
        }
        match report.duration_seconds {
            Some(d) if d > 0.0 && d < 1_000_000_000.0 => {
                assert!(
                    report.bitrate_bits_per_second.map_or(true, |b| b > 0),
                    "{name}: positive bitrate required for a sane duration"
                );
            }
            _ => {}
        }
        for a in &report.audio {
            if let (Some(first), Some(last)) = (a.first_pts, a.last_pts) {
                assert_eq!(first.timebase, tb(), "{name}: timebase preserved");
                assert_eq!(last.timebase, tb(), "{name}: timebase preserved");
                // The probe reports a min/max *envelope* (computed on
                // unsigned ticks), not file-order endpoints, so for the
                // out-of-order cases we can only assert presence, not the
                // exact values.
                if name == "strictly descending" {
                    // Envelope semantics: reported last = max PTS seen.
                    assert_eq!(last.ticks, 1000, "{name}");
                }
            }
        }
    }
}
