//! The §46 "C integration test": a real C translation unit
//! (`tests/integration.c`), compiled by the build script against
//! `include/tpt_media.h` alone, drives the ABI end to end. These tests
//! only assert on its exit codes — the actual encoder/decoder calls
//! happen in C, through the header a C consumer would use.

// The C object is compiled into a static archive by the build script
// and found via the crate's forwarded native search path. An explicit
// `#[link]` here (rather than relying on forwarded `-l` flags) keeps
// the test's linkage self-describing: this binary is the one that
// calls C entry points, so it is the one that names the C library.
#[link(name = "tpt_media_c_integration", kind = "static")]
extern "C" {
    fn tpt_c_integration_round_trip() -> i32;
    fn tpt_c_integration_null_arguments() -> i32;
}

#[test]
fn c_round_trips_aura_lossless_through_the_header() {
    // Referencing the crate does two jobs: it pins the Rust-side ABI
    // constant against the C header's `TPT_ABI_VERSION`, and it keeps
    // the rlib on the link line — the C object's `tpt_*` references
    // resolve against its objects, and an otherwise-unused crate would
    // be dropped from the link entirely.
    assert_eq!(tpt_media_sys::ABI_VERSION, 1);
    let rc = unsafe { tpt_c_integration_round_trip() };
    assert_eq!(rc, 0, "C round trip failed at stage {rc}");
}

#[test]
fn c_null_arguments_are_error_codes() {
    let rc = unsafe { tpt_c_integration_null_arguments() };
    assert_eq!(rc, 0, "C NULL-argument check failed at stage {rc}");
}
