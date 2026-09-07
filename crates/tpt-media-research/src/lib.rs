//! Representation-research harness for `tpt-media`: synthetic signal
//! generators, deterministic PRNG seeding, and empirical entropy
//! estimation.
//!
//! See `docs/representation-research.md` and `todo.md` for the
//! methodology this crate exists to support. The crate has no
//! representation-codec logic of its own; it consumes the abstract
//! `Representation` contract from `tpt-media-representation` and the
//! generic DSP primitives from `tpt-dsp-core`, and exposes measurement
//! helpers (synthetic inputs, entropy estimators, deterministic RNG) the
//! harness uses to compare representations honestly.
//!
//! # Determinism
//!
//! Every helper in this crate is a closed-form or seeded function — there
//! is no `rand`, no thread-local state, no I/O. Two runs with the same
//! inputs and the same seed are byte-identical.

pub mod det_rng;
pub mod entropy;
pub mod signals;
