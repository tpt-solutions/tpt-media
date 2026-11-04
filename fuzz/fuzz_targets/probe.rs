#![no_main]

//! Fuzzes `tpt-media-probe` over arbitrary bytes. `probe` never returns an
//! error, so the only failure this target can report is a panic.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = tpt_media_probe::probe(data);
});
