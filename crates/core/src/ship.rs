// file: crates/core/src/ship.rs
//! Ship data types — the player's home, identity, and persistent companion.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

/// The ship is home, identity, and the one thing that persists while
/// the galaxy ages around you. It accumulates scars, modifications,
/// and history. After enough play, no two ships should be alike.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ship {
    pub name: String,
    /// 0.0 = destroyed, 1.0 = pristine.
    pub hull_condition: f32,
    /// Arbitrary fuel units. FTL travel consumes fuel; running out
    /// means limping on sublight — slow and costly in time.
    pub fuel: f32,
    pub fuel_capacity: f32,
    /// Named cargo items with quantities.
    pub cargo: HashMap<String, u32>,
    pub cargo_capacity: u32,
    /// The ship's installed systems — each with independent condition.
    pub modules: ShipModules,
}

/// The ship's core systems. Each module has a condition score (0.0–1.0)
/// and can be upgraded, damaged, or jury-rigged with alien tech.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShipModules {
    pub engine: Module,
    pub sensors: Module,
    pub comms: Module,
    pub weapons: Module,
    pub life_support: Module,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    /// What kind of module (for display and LLM context).
    pub variant: String,
    /// 0.0 = non-functional, 1.0 = factory fresh.
    pub condition: f32,
    /// Freeform notes — "jury-rigged with Venn crystalline arrays",
    /// "damaged in pirate ambush near Cygnus Gate".
    pub notes: Vec<String>,
}

impl Module {
    pub fn standard(variant: &str) -> Self {
        Self {
            variant: variant.to_string(),
            condition: 1.0,
            notes: Vec::new(),
        }
    }
}

/// How the ship is currently traveling (or not).
///
/// FTL is the standard mode of interstellar travel. Every ship has
/// a drive; fuel is the constraint, not the technology. Sublight is
/// the emergency fallback — your drive is damaged or you can't
/// afford fuel. It's slow and has modest time dilation (3:1 ratio),
/// but it won't strand you forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TravelMode {
    /// Docked or in orbit. No time cost.
    Stationary,
    /// Standard FTL transit. Costs fuel, takes days to weeks.
    /// Personal and galactic time stay roughly in sync.
    Ftl,
    /// Emergency sublight. Free but slow — weeks to months per
    /// light-year. Modest time dilation (~3:1 galactic/personal).
    /// Used when the FTL drive is damaged or fuel is depleted.
    Sublight,
}
