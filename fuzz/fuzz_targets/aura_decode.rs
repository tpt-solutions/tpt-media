#![no_main]

//! Fuzzes `AuraDecoder::decode` on arbitrary packet bodies.
//!
//! Every Aura packet ends in a CRC-32 over its body, and the decoder rejects
//! a mismatch before parsing anything. Random bytes would almost always fail
//! that check and never reach the parser, so this target appends a correct
//! CRC to each fuzzer input. The corpus then explores the bitstream itself.

use libfuzzer_sys::fuzz_target;
use tpt_audio_aura::{AuraConfig, AuraDecoder};
use tpt_media_codec::{CodecConfig, Decoder};
use tpt_media_core::{Packet, PacketFlags, Timebase, Timestamp};

/// IEEE 802.3 CRC-32, the same polynomial Aura uses for its packet trailer.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fuzz_target!(|data: &[u8]| {
    // Both modes are reached by the packet's own mode bit, so one decoder
    // configuration covers them. The block and LPC sizes match the unit tests.
    let cfg = AuraConfig {
        block_size: 256,
        lpc_order: 8,
        ..Default::default()
    };
    let Ok(mut dec) = AuraDecoder::new(CodecConfig::default(), cfg) else {
        return;
    };
    let mut body = data.to_vec();
    let crc = crc32(&body);
    body.extend_from_slice(&crc.to_le_bytes());
    let packet = Packet {
        data: body,
        pts: Timestamp::zero(Timebase::new(1, 48_000)),
        dts: Timestamp::zero(Timebase::new(1, 48_000)),
        flags: PacketFlags::KEYFRAME,
        track_id: 0,
        duration: None,
    };
    let _ = dec.decode(&packet);
});
