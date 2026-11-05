#![no_main]

//! Fuzzes the `.tpt` container parser over arbitrary bytes. Errors are
//! expected; panics, overflows, and unbounded allocation are the bugs.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = tpt_media_format::parse_tpt(data);
});
