//! Deterministic synthetic signal generators (`todo.md` §56, §9 of the
//! research prompt). Every generator here is a closed-form or seeded
//! function of `(x, y, t)` — no randomness beyond a fixed, reproducible seed
//! — so results are exactly repeatable across runs.

pub mod spatial;
pub mod spatiotemporal;
