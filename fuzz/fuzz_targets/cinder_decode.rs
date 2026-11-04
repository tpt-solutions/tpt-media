#![no_main]

//! Fuzzes `CinderDecoder::decode` on arbitrary packet bytes. Cinder has no
//! packet checksum, so the fuzzer input is passed through unchanged.

use libfuzzer_sys::fuzz_target;
use tpt_audio_cinder::{CinderDecoder, CinderEncoderConfig};
use tpt_media_codec::{CodecConfig, Decoder};
use tpt_media_core::{Packet, PacketFlags, Timebase, Timestamp};

fuzz_target!(|data: &[u8]| {
    let Ok(mut dec) = CinderDecoder::new(CodecConfig::default(), CinderEncoderConfig::default())
    else {
        return;
    };
    let packet = Packet {
        data: data.to_vec(),
        pts: Timestamp::zero(Timebase::new(1, 48_000)),
        dts: Timestamp::zero(Timebase::new(1, 48_000)),
        flags: PacketFlags::KEYFRAME,
        track_id: 0,
        duration: None,
    };
    let _ = dec.decode(&packet);
});
