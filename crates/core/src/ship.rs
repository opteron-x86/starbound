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
    /// Arbitrary fuel units. Travel consumes fuel; running out is bad.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TravelMode {
    /// Docked or in orbit. No time cost.
    Stationary,
    /// Free and safe, but decades pass in the galaxy.
    Sublight,
    /// Expensive, rare, dangerous — but keeps you in sync.
    Ftl,
}
