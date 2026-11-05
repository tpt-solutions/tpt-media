//! Version 0.2 packet runs (`PKTS`), checked through the public API only.

use tpt_media_container::Container;
use tpt_media_core::{Duration, Packet, PacketFlags, Timebase, Timestamp};
use tpt_media_format::{for_each_chunk, parse_tpt, TptDemuxer, TptMuxer, CHUNK_PKTS};

fn packet(
    track_id: u8,
    pts: i64,
    dts: i64,
    key: bool,
    duration: Option<i64>,
    data: &[u8],
) -> Packet {
    let tb = Timebase::new(1, 1000);
    let mut p = Packet::empty(
        track_id,
        Timestamp {
            ticks: pts,
            timebase: tb,
        },
    );
    p.dts = Timestamp {
        ticks: dts,
        timebase: tb,
    };
    p.data = data.to_vec();
    if key {
        p.flags |= PacketFlags::KEYFRAME;
    }
    p.duration = duration.map(|d| Duration {
        ticks: d,
        timebase: tb,
    });
    p
}

/// Audio on track 0 with a packet per tick, and video on track 1 every
/// tenth tick with B-frame-style dts; some packets carry durations.
fn interleaved_packets() -> Vec<Packet> {
    let mut out = Vec::new();
    for i in 0..200i64 {
        let key = i % 50 == 0;
        out.push(packet(
            0,
            i,
            i,
            key,
            if i % 3 == 0 { Some(7) } else { None },
            &[i as u8; 5],
        ));
        if i % 10 == 0 {
            // dts runs one tick behind pts, so it is stored explicitly.
            out.push(packet(1, i, i - 1, i % 20 == 0, None, &[0xEE; 40]));
        }
    }
    out
}

fn mux(packets: &[Packet], checksums: bool) -> Vec<u8> {
    let mut m = TptMuxer::new(Timebase::new(1, 1000));
    m.add_track(0, b"CIND", &[8]);
    m.add_track(1, b"THRM", &[1, 2, 3]);
    if checksums {
        m.enable_cluster_checksums();
    }
    for p in packets {
        m.write_packet_raw(p).unwrap();
    }
    m.finish()
}

fn key(p: &Packet) -> (u8, i64, i64, bool, Option<i64>, Vec<u8>) {
    (
        p.track_id,
        p.pts.ticks,
        p.dts.ticks,
        p.is_keyframe(),
        p.duration.map(|d| d.ticks),
        p.data.clone(),
    )
}

#[test]
fn interleaved_tracks_round_trip_every_field_in_order() {
    let input = interleaved_packets();
    let bytes = mux(&input, false);
    let (_, _, out) = parse_tpt(&bytes).expect("parses");
    assert_eq!(out.len(), input.len());
    for (a, b) in input.iter().zip(&out) {
        assert_eq!(key(a), key(b));
    }
}

#[test]
fn runs_hold_at_most_32_packets_of_one_track() {
    let input = interleaved_packets();
    let bytes = mux(&input, false);
    let mut records = 0usize;
    let mut runs = 0usize;
    for_each_chunk(&bytes, |id, body| {
        if id == &CHUNK_PKTS {
            runs += 1;
            let count = u16::from_le_bytes([body[2], body[3]]) as usize;
            assert!((1..=32).contains(&count), "run of {count}");
            records += count;
        }
    })
    .unwrap();
    assert_eq!(records, input.len(), "every packet is in some run");
    assert!(runs > 10, "interleaving forces many short runs, got {runs}");
}

#[test]
fn a_single_track_packs_into_full_runs() {
    let input: Vec<Packet> = (0..100)
        .map(|i| packet(0, i, i, true, None, &[1; 4]))
        .collect();
    let bytes = mux(&input, false);
    let mut counts = Vec::new();
    for_each_chunk(&bytes, |id, body| {
        if id == &CHUNK_PKTS {
            counts.push(u16::from_le_bytes([body[2], body[3]]));
        }
    })
    .unwrap();
    assert_eq!(counts, vec![32, 32, 32, 4]);
}

#[test]
fn audio_packets_cost_about_ten_bytes_of_framing_each() {
    // 2000 one-tick audio packets of 17 bytes, the Cinder shape.
    let input: Vec<Packet> = (0..2000)
        .map(|i| packet(0, i, i, true, None, &[7; 17]))
        .collect();
    let bytes = mux(&input, false);
    let payload = 2000 * 17;
    let framing_per_packet = (bytes.len() - payload) as f64 / 2000.0;
    assert!(
        framing_per_packet < 12.0,
        "framing is {framing_per_packet:.1} bytes per packet"
    );
}

#[test]
fn checksums_cover_each_run_and_a_flipped_byte_is_reported() {
    let input = interleaved_packets();
    let mut bytes = mux(&input, true);
    let demux = TptDemuxer::new(&bytes).unwrap();
    assert!(demux.verify_cluster_checksums().is_ok());
    drop(demux);

    // Flip a payload byte inside the first run: the last 4 bytes of a run
    // are its CRC, so the byte just before them is inside the last record.
    let mut target = None;
    for_each_chunk(&bytes, |id, body| {
        if id == &CHUNK_PKTS && target.is_none() {
            target = Some(body.len());
        }
    })
    .unwrap();
    let body_len = target.expect("a run exists");
    let mut body_start = None;
    let mut pos = 20usize;
    while pos + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if &bytes[pos..pos + 4] == b"PKTS" {
            body_start = Some(pos + 8);
            break;
        }
        pos += 8 + size;
    }
    let start = body_start.unwrap();
    bytes[start + body_len - 4 - 1] ^= 0xFF;
    let demux = TptDemuxer::new(&bytes).unwrap();
    let bad = demux
        .verify_cluster_checksums()
        .expect_err("corruption is reported");
    assert_eq!(bad.len(), 1, "one run, one report");
}

#[test]
fn version_0_1_files_of_single_packet_clusters_still_read() {
    let mut m = TptMuxer::new(Timebase::new(1, 1000));
    m.add_track(3, b"AURA", &[]);
    m.add_cluster(10, 0x01, 3, b"first");
    m.add_cluster(20, 0x00, 3, b"second");
    let mut bytes = m.finish();
    // Rewrite the header to version 0.1, as the previous writer produced.
    bytes[4] = 0x01;
    bytes[5] = 0x00;
    let (header, _, packets) = parse_tpt(&bytes).expect("0.1 file parses");
    assert_eq!(header.version, 0x0001);
    assert_eq!(packets.len(), 2);
    assert_eq!(packets[0].data, b"first");
    assert!(packets[0].is_keyframe());
    assert_eq!(packets[1].pts.ticks, 20);
}

#[test]
fn keyframe_seek_lands_on_the_keyframe_packet_inside_a_run() {
    // One run of 10 packets; only the one at pts 5 is a keyframe.
    let input: Vec<Packet> = (0..10)
        .map(|i| packet(0, i, i, i == 5, None, &[i as u8]))
        .collect();
    let bytes = mux(&input, false);
    let mut d = TptDemuxer::new(&bytes).unwrap();
    d.seek_to_keyframe_on_track(0, 3).unwrap();
    let first = d.next_packet().unwrap();
    assert_eq!(
        first.pts.ticks, 5,
        "seek must not start at the run's first packet"
    );
    assert_eq!(first.data, vec![5u8]);
}

#[test]
fn disk_index_has_one_entry_per_run() {
    let input: Vec<Packet> = (0..70).map(|i| packet(0, i, i, true, None, &[1])).collect();
    let bytes = mux(&input, false);
    let d = TptDemuxer::new(&bytes).unwrap();
    let index = d.disk_index().expect("index present");
    assert_eq!(index.version, 2);
    assert_eq!(index.tracks[&0].len(), 3, "70 packets in runs of 32, 32, 6");
}
