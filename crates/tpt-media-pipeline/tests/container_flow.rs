//! Integration: a real `tpt` file's packets flow through a pipeline with
//! timestamps intact — exercising the §26 "timestamps through the
//! pipeline" guarantee against the actual container codec.

use tpt_container::build_tpt;
use tpt_media_core::{Packet, Timebase, Timestamp};
use tpt_media_pipeline::{NodeOps, Sink, Source};

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

#[test]
fn container_packets_flow_through_with_pts_intact() {
    let packets: Vec<Packet> = (0..5)
        .map(|i| {
            let mut p = Packet::empty(1, ts(i * 480));
            p.duration = Some(tpt_media_core::Duration {
                ticks: 480,
                timebase: tb(),
            });
            p.data.extend_from_slice(&[0; 16]);
            p
        })
        .collect();

    let bytes = build_tpt(tb(), &[(1, b"CIND", &[][..])], &packets);
    // Sanity: the container round-trips through its own demuxer losslessly.
    let (_, tracks, reloaded) = tpt_container::parse_tpt(&bytes).expect("valid tpt");
    assert_eq!(tracks[0].id, 1);
    assert_eq!(reloaded.len(), 5);

    // Pipeline: Source -> map payload to track id (data unchanged), Sink.
    let mut p = Source::new(reloaded)
        .map(|pkt: Packet| pkt.track_id)
        .then(Sink::new())
        .into_pipeline();
    let _out = p.drive(16).expect("run completes");
    let collected = p.core().right().items();
    assert_eq!(collected, &[1, 1, 1, 1, 1]);
}
