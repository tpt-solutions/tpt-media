//! DSP primitives used by the tpt-media codec suite.
//!
//! All functions are pure-Rust, deterministic, and free of heap allocation
//! (the 2-D DCT allocates a small stack buffer for the working matrix when
//! sizes exceed 8×8).

pub mod dct;
pub mod lpc;
pub mod mdct;