//! The crate used strictly through its public API, as a downstream page /
//! app would: nothing here reaches into private modules.

use tpt_web::synth::{self, AudioKind};
use tpt_web::{DecoderTable, Player, Status, StreamParser, WebError, WebLimits};

#[test]
fn the_documented_usage_pattern_plays_a_file_end_to_end() {
    let file = synth::av_tpt(AudioKind::Aura, 48_000, 0.3, 32, 32, 10);
    let mut player = Player::new(WebLimits::default());
    let (mut samples, mut pictures) = (0u64, 0u64);
    for chunk in file.chunks(700) {
        loop {
            match player.push(chunk) {
                Ok(()) => break,
                Err(WebError::InputFull) => {
                    player.pump().unwrap();
                    while player.pop_audio().is_some() {}
                    while player.pop_video().is_some() {}
                }
                Err(e) => panic!("{e}"),
            }
        }
        player.pump().unwrap();
        while let Some(a) = player.pop_audio() {
            samples += u64::from(a.frames);
        }
        while player.pop_video().is_some() {
            pictures += 1;
        }
    }
    player.finish_input();
    while player.pump().unwrap() != Status::Ended {
        while let Some(a) = player.pop_audio() {
            samples += u64::from(a.frames);
        }
        while player.pop_video().is_some() {
            pictures += 1;
        }
    }
    while let Some(a) = player.pop_audio() {
        samples += u64::from(a.frames);
    }
    while player.pop_video().is_some() {
        pictures += 1;
    }
    assert_eq!(samples, 14_400); // 0.3 s at 48 kHz, padding trimmed
    assert_eq!(pictures, 3);
    assert_eq!(player.stats().parser.crc_failures, 0);
}

#[test]
fn an_empty_table_plays_nothing_but_fails_cleanly() {
    let file = synth::audio_tpt(AudioKind::Cinder, 48_000, 0.1);
    let mut player = Player::with_table(WebLimits::default(), DecoderTable::empty());
    player.push(&file).unwrap();
    player.finish_input();
    let err = loop {
        match player.pump() {
            Ok(Status::Ended) => panic!("ended without a decoder"),
            Ok(_) => {}
            Err(e) => break e,
        }
    };
    assert!(matches!(err, WebError::Unsupported(_)));
    assert_eq!(player.status(), Status::Failed);
}

#[test]
fn the_parser_is_usable_on_its_own() {
    let file = synth::video_tpt(32, 32, 10, 2);
    let mut p = StreamParser::new(WebLimits::default());
    p.push(&file).unwrap();
    let mut packets = 0;
    while let Some(ev) = p.poll().unwrap() {
        if matches!(ev, tpt_web::ParseEvent::Packet(_)) {
            packets += 1;
        }
    }
    assert_eq!(packets, 2);
    assert_eq!(p.tracks().len(), 1);
}
