// file: crates/simulation/src/lib.rs
//! Galaxy simulation — generation, travel, faction behavior, galactic ticks.
//!
//! - `generate`: Deterministic galaxy generation from a seed (systems,
//!   civilizations, factions, NPCs, connections, economies).
//! - `templates`: JSON template loaders for generation data.
//! - `travel`: FTL-default travel planning with place-based time dilation.
//! - `faction_ai`: Civilization behavior trees (ethos-weighted priorities).
//! - `faction_tick`: Per-tick faction presence drift, expansion, and retreat.
//! - `tick`: Galactic tick engine — batches elapsed time into yearly ticks.

pub mod generate;
pub mod templates;
pub mod travel;
pub mod faction_ai;
pub mod faction_tick;
pub mod tick;