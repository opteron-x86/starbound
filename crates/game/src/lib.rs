// file: crates/game/src/lib.rs
//! Game orchestration — state management, save/load, main loop (future).
//!
//! Day 2: Persistence layer.
//! Day 3: Travel execution — applying travel plans to journey state.

pub mod persistence;
pub mod travel;
pub mod consequences;
pub mod supplies;
pub mod checks;
pub mod reputation;