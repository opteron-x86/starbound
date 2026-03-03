// file: crates/core/src/galaxy.rs
//! Galaxy data types — star systems, sectors, connections, factions.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

use crate::time::Timestamp;

// ---------------------------------------------------------------------------
// Star Systems
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StarSystem {
    pub id: Uuid,
    pub name: String,
    /// 2D position in the galaxy (light-years from origin).
    pub position: (f64, f64),
    pub star_type: StarType,
    pub planetary_bodies: Vec<PlanetaryBody>,
    /// Which faction controls this system, if any.
    pub controlling_faction: Option<Uuid>,
    /// Rough measure of development: stations, trade, population.
    pub infrastructure_level: InfrastructureLevel,
    /// Significant events that have occurred here, most recent last.
    pub history: Vec<HistoryEntry>,
    /// Thread IDs from the narrative ledger that are active here.
    pub active_threads: Vec<Uuid>,
    /// Temporal distortion factor for this system.
    ///
    /// 1.0 = normal time (a day here is a day everywhere).
    /// Higher values mean time passes faster outside while you're
    /// docked here. A system with factor 5.0 means one personal
    /// day costs five galactic days.
    ///
    /// Most settled systems are 1.0. Neutron stars, black holes,
    /// and anomalous systems at the edge of known space have
    /// higher factors. The mission leads toward increasingly
    /// distorted space — the further you push, the more time
    /// you sacrifice.
    pub time_factor: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum StarType {
    RedDwarf,
    YellowDwarf,
    BlueGiant,
    WhiteDwarf,
    Neutron,
    Binary,
    BlackHole,
    Anomalous,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanetaryBody {
    pub name: String,
    pub body_type: BodyType,
    /// Free-text notable features for LLM context.
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum BodyType {
    Terrestrial,
    GasGiant,
    IceWorld,
    Barren,
    Oceanic,
    Gaia,
    Artificial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum InfrastructureLevel {
    None,
    Outpost,
    Colony,
    Established,
    Hub,
    Capital,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: Timestamp,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Sectors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sector {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub system_ids: Vec<Uuid>,
}

// ---------------------------------------------------------------------------
// Connections (travel routes between systems)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub system_a: Uuid,
    pub system_b: Uuid,
    /// Distance in light-years.
    pub distance_ly: f64,
    pub route_type: RouteType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum RouteType {
    /// Standard travel lane.
    Open,
    /// Established, well-trafficked corridor. Slightly faster.
    Corridor,
    /// Dangerous — pirates, radiation, anomalies. Slower.
    Hazardous,
    /// Legacy: FTL-capable route. Now treated as Corridor.
    /// Kept for serialization compatibility.
    FtlLane,
}

// ---------------------------------------------------------------------------
// Factions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Faction {
    pub id: Uuid,
    pub name: String,
    /// The faction's governing philosophy — simulation parameters, not just labels.
    pub ethos: FactionEthos,
    /// Resources and capability scores that determine what the faction *can* do.
    pub capabilities: FactionCapabilities,
    /// How this faction views every other faction it knows about.
    pub relationships: HashMap<Uuid, FactionDisposition>,
    /// Internal political state — pressures, movements, stability.
    pub internal_dynamics: InternalDynamics,
}

/// Weighted values representing a faction's governing philosophy.
/// Each is 0.0–1.0. These drive behavior tree priorities in the simulation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FactionEthos {
    pub expansionist: f32,
    pub isolationist: f32,
    pub militaristic: f32,
    pub diplomatic: f32,
    pub theocratic: f32,
    pub mercantile: f32,
    pub technocratic: f32,
    pub communal: f32,
}

/// What a faction can actually do, regardless of what it wants to do.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FactionCapabilities {
    /// How many systems, how large the population.
    pub size: f32,
    /// Economic output and reserves.
    pub wealth: f32,
    /// Sophistication of technology.
    pub technology: f32,
    /// Military strength — ships, weapons, personnel.
    pub military: f32,
}

/// How one faction views another across multiple dimensions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FactionDisposition {
    /// Diplomatic warmth or hostility (-1.0 to 1.0).
    pub diplomatic: f32,
    /// Economic entanglement (0.0 = none, 1.0 = deeply interdependent).
    pub economic: f32,
    /// Military tension (-1.0 = active war, 0.0 = neutral, 1.0 = alliance).
    pub military: f32,
}

/// Internal pressures and political state within a faction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalDynamics {
    /// 0.0 = on the verge of collapse, 1.0 = rock solid.
    pub stability: f32,
    /// Active internal movements or pressures.
    pub pressures: Vec<String>,
}
