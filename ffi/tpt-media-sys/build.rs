//! Compiles the C integration test (`tests/integration.c`) against this
//! crate's header (`include/tpt_media.h`) so `tests/c_abi.rs` can drive
//! the real C ABI end-to-end, not just the same functions re-declared in
//! Rust.
//!
//! The C object only ever participates in a *link* when the test binary
//! is built — the `rlib`/`staticlib` builds of this crate have no link
//! step consuming it — so a normal `cargo build` is unaffected beyond
//! needing a C compiler to exist (it must anyway: rustc's own MSVC/
//! MinGW toolchain implies one).

fn main() {
    println!("cargo:rerun-if-changed=tests/integration.c");
    println!("cargo:rerun-if-changed=include/tpt_media.h");
    cc::Build::new()
        .file("tests/integration.c")
        .include("include")
        .warnings(true)
        .compile("tpt_media_c_integration");
}
