//! Representation-research harness for `tpt-media`: synthetic signal
//! generators, deterministic PRNG seeding, empirical entropy estimation, the
//! matched-rate distortion harness, the concrete representations under test,
//! and the oracle experiment / methodology phases run by `experiments.rs`.
//!
//! See `docs/representation-research.md` and `todo.md` §56 for the
//! methodology this crate exists to support. The crate has no representation-
//! codec logic of its own; it consumes the abstract `Representation`
//! contract from `tpt-media-representation` and the generic DSP primitives
//! from `tpt-dsp-core`, and exposes measurement helpers (synthetic inputs,
//! entropy estimators, deterministic RNG) that the harness uses to compare
//! representations honestly.
//!
//! # Determinism
//!
//! Every helper in this crate is a closed-form or seeded function — there
//! is no `rand`, no thread-local state, no I/O. Two runs with the same
//! inputs and the same seed are byte-identical.

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod det_rng;
pub mod diagnostics;
pub mod entropy;
pub mod experiments;
pub mod harness;
pub mod kmeans;
pub mod representations;
pub mod signals;
pub mod uniform;
