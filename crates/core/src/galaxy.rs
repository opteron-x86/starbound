// file: crates/core/src/galaxy.rs
//! Galaxy data types — star systems, sectors, connections, civilizations, factions.

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
    /// Visitable places within this system — stations, planets, moons, belts.
    pub locations: Vec<Location>,
    /// Which civilization controls this system, if any.
    pub controlling_civ: Option<Uuid>,
    /// Rough measure of overall development (max of location infrastructures).
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
    pub time_factor: f64,
    /// Which factions have a presence in this system and how visible
    /// they are. A Hegemony military base has Military Command at
    /// strength 0.9 / visibility 1.0. The Corridor Guild might be
    /// here too — strength 0.3, visibility 0.1.
    pub faction_presence: Vec<FactionPresence>,
}

/// Stellar classification. Drives planet generation, habitability,
/// orbital distances, and location descriptions.
///
/// Based on real stellar classification (O through M main sequence,
/// plus evolved/exotic types). Each type carries physical properties
/// that constrain what kind of systems form around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum StarType {
    /// Class O — blue supergiant. Hottest, most massive main-sequence.
    /// 1–10 Myr lifespan. Intense radiation sterilizes surrounding space.
    BlueSuperGiant,
    /// Class B — large blue main-sequence. Rare, luminous, short-lived.
    /// 10–100 Myr lifespan. Heavy UV, minimal habitability.
    BlueGiant,
    /// Class A — white/blue-white main-sequence. Fast-rotating.
    /// ~1–2 Gyr lifespan. Wide habitable zone but high UV.
    WhiteStar,
    /// Class F — yellow-white dwarf. Good habitability candidate.
    /// Wide habitable zone, moderate UV.
    YellowWhiteDwarf,
    /// Class G — yellow dwarf (Sol-type). Optimal for life.
    /// ~10 Gyr lifespan. The gold standard for habitable systems.
    YellowDwarf,
    /// Class K — orange dwarf. Very stable, long-lived (~30 Gyr).
    /// Narrower habitable zone but excellent long-term habitability.
    OrangeDwarf,
    /// Class M — red dwarf. Most common star type in the universe.
    /// Extremely long lifespan but planets face tidal locking and flares.
    RedDwarf,
    /// Evolved M-class red giant. Former sun-like star in late evolution.
    /// High luminosity pushes habitable zone very far out.
    RedGiant,
    /// Class T — brown dwarf (substellar). Too small for hydrogen fusion.
    /// Extremely dim and cool. Negligible habitable zone.
    BrownDwarf,
    /// White dwarf — burnt-out core of a low/medium-mass star.
    /// Earth-sized, extremely dense. Narrow unstable habitable zone.
    WhiteDwarf,
    /// Wolf-Rayet — massive evolved star shedding outer layers.
    /// Extremely hot and luminous. Rich in heavy elements. Uninhabitable.
    WolfRayet,
    /// Pulsar — magnetized neutron star emitting radiation pulses.
    /// Precise timekeeping, lethal radiation. No habitable zone.
    Pulsar,
    /// Neutron star — ultra-dense supernova remnant.
    /// ~10km across, more massive than a G-star. Exotic physics.
    Neutron,
    /// Black hole — collapsed massive star. Extreme gravity.
    /// No habitable zone. Exotic gravitational effects.
    BlackHole,
    /// Binary system — two stars orbiting each other.
    /// Complex orbital dynamics. Habitability varies widely.
    Binary,
    /// Anomalous — something that doesn't fit standard classification.
    /// Distorted spacetime, unknown physics. The weird ones.
    Anomalous,
}

impl StarType {
    /// Habitability percentage — chance that a given planet in this system
    /// rolls from the habitable body type pool vs. the inhospitable pool.
    pub fn habitability(&self) -> f64 {
        match self {
            StarType::BlueSuperGiant => 0.05,
            StarType::BlueGiant => 0.15,
            StarType::WhiteStar => 0.30,
            StarType::YellowWhiteDwarf => 1.0,
            StarType::YellowDwarf => 1.0,
            StarType::OrangeDwarf => 1.0,
            StarType::RedDwarf => 0.40,
            StarType::RedGiant => 0.10,
            StarType::BrownDwarf => 0.10,
            StarType::WhiteDwarf => 0.05,
            StarType::WolfRayet => 0.0,
            StarType::Pulsar => 0.0,
            StarType::Neutron => 0.0,
            StarType::BlackHole => 0.0,
            StarType::Binary => 0.60,
            StarType::Anomalous => 0.20,
        }
    }

    /// Typical planet count range for this star type.
    pub fn planet_count_range(&self) -> (usize, usize) {
        match self {
            StarType::BlueSuperGiant => (0, 2),
            StarType::BlueGiant => (1, 3),
            StarType::WhiteStar => (2, 4),
            StarType::YellowWhiteDwarf => (3, 6),
            StarType::YellowDwarf => (3, 6),
            StarType::OrangeDwarf => (2, 5),
            StarType::RedDwarf => (2, 4),
            StarType::RedGiant => (1, 3),
            StarType::BrownDwarf => (0, 2),
            StarType::WhiteDwarf => (0, 2),
            StarType::WolfRayet => (0, 1),
            StarType::Pulsar => (0, 1),
            StarType::Neutron => (0, 1),
            StarType::BlackHole => (0, 2),
            StarType::Binary => (2, 5),
            StarType::Anomalous => (0, 3),
        }
    }

    /// Inner orbital distance (AU) — where the first planet can form.
    /// Driven by radiation pressure and tidal forces.
    pub fn inner_orbit_au(&self) -> f32 {
        match self {
            StarType::BlueSuperGiant => 5.0,
            StarType::BlueGiant => 3.0,
            StarType::WhiteStar => 1.5,
            StarType::YellowWhiteDwarf => 0.5,
            StarType::YellowDwarf => 0.3,
            StarType::OrangeDwarf => 0.15,
            StarType::RedDwarf => 0.05,
            StarType::RedGiant => 3.0,
            StarType::BrownDwarf => 0.01,
            StarType::WhiteDwarf => 0.005,
            StarType::WolfRayet => 10.0,
            StarType::Pulsar => 0.5,
            StarType::Neutron => 0.3,
            StarType::BlackHole => 1.0,
            StarType::Binary => 1.0,
            StarType::Anomalous => 0.1,
        }
    }

    /// Orbital spacing multiplier — how far apart planets tend to be.
    /// Luminous stars spread planets further; dim stars pack them in.
    pub fn orbital_spacing(&self) -> f32 {
        match self {
            StarType::BlueSuperGiant => 4.0,
            StarType::BlueGiant => 3.0,
            StarType::WhiteStar => 2.0,
            StarType::YellowWhiteDwarf => 1.5,
            StarType::YellowDwarf => 1.0,
            StarType::OrangeDwarf => 0.7,
            StarType::RedDwarf => 0.4,
            StarType::RedGiant => 3.0,
            StarType::BrownDwarf => 0.15,
            StarType::WhiteDwarf => 0.3,
            StarType::WolfRayet => 5.0,
            StarType::Pulsar => 1.0,
            StarType::Neutron => 0.5,
            StarType::BlackHole => 2.0,
            StarType::Binary => 1.5,
            StarType::Anomalous => 1.0,
        }
    }

    /// Light color/quality — used in location descriptions.
    pub fn light_description(&self) -> &'static str {
        match self {
            StarType::BlueSuperGiant => "searing blue-white light that bleaches everything it touches",
            StarType::BlueGiant => "harsh blue-white glare that casts sharp, cold shadows",
            StarType::WhiteStar => "clean white light with a faint blue edge",
            StarType::YellowWhiteDwarf => "warm yellow-white light, brighter than Sol",
            StarType::YellowDwarf => "familiar yellow sunlight",
            StarType::OrangeDwarf => "deep amber light that tints everything in warm tones",
            StarType::RedDwarf => "dim red light that never quite reaches full day",
            StarType::RedGiant => "bloated orange glow that fills half the sky",
            StarType::BrownDwarf => "faint infrared warmth — barely visible, more felt than seen",
            StarType::WhiteDwarf => "intense pinpoint glare from a star the size of a planet",
            StarType::WolfRayet => "violent blue-violet radiance flickering through stellar winds",
            StarType::Pulsar => "rhythmic flash of radiation sweeping like a lighthouse",
            StarType::Neutron => "faint, hard light from an object smaller than a city",
            StarType::BlackHole => "no light — just the warped glow of the accretion disk",
            StarType::Binary => "shifting double shadows from twin stars",
            StarType::Anomalous => "light that doesn't behave the way light should",
        }
    }

    /// Radiation environment — affects station shielding descriptions
    /// and surface habitability flavor.
    pub fn radiation_level(&self) -> &'static str {
        match self {
            StarType::BlueSuperGiant | StarType::WolfRayet => "extreme",
            StarType::BlueGiant | StarType::Pulsar => "severe",
            StarType::WhiteStar => "high",
            StarType::YellowWhiteDwarf => "moderate",
            StarType::YellowDwarf | StarType::OrangeDwarf | StarType::Binary => "normal",
            StarType::RedDwarf => "low (with flare risk)",
            StarType::RedGiant => "moderate (UV low, infrared high)",
            StarType::BrownDwarf => "negligible",
            StarType::WhiteDwarf => "high (UV intense at close range)",
            StarType::Neutron | StarType::BlackHole => "extreme (gravitational/magnetic)",
            StarType::Anomalous => "unpredictable",
        }
    }

    /// Short descriptor for the star itself — used in system descriptions.
    pub fn star_descriptor(&self) -> &'static str {
        match self {
            StarType::BlueSuperGiant => "a massive blue supergiant burning through its brief, violent life",
            StarType::BlueGiant => "a luminous blue giant, bright enough to see across the sector",
            StarType::WhiteStar => "a white main-sequence star with a wide habitable zone",
            StarType::YellowWhiteDwarf => "a steady yellow-white star, slightly brighter than Sol",
            StarType::YellowDwarf => "a yellow dwarf — the kind of star civilizations grow around",
            StarType::OrangeDwarf => "a stable orange dwarf that will burn for tens of billions of years",
            StarType::RedDwarf => "a dim red dwarf, the most common star in the galaxy",
            StarType::RedGiant => "a swollen red giant, a sun-like star in its dying expansion",
            StarType::BrownDwarf => "a brown dwarf — a failed star, barely glowing",
            StarType::WhiteDwarf => "a dense white dwarf, the burnt-out core of a dead star",
            StarType::WolfRayet => "a Wolf-Rayet star tearing itself apart in stellar winds",
            StarType::Pulsar => "a spinning pulsar sweeping radiation beams across the void",
            StarType::Neutron => "a neutron star — a city-sized remnant denser than atomic nuclei",
            StarType::BlackHole => "a black hole, visible only by the light it bends and devours",
            StarType::Binary => "a binary pair, two stars locked in gravitational embrace",
            StarType::Anomalous => "something that resists classification",
        }
    }
}

// ---------------------------------------------------------------------------
// Locations — visitable places within a star system
// ---------------------------------------------------------------------------

/// A visitable place within a star system. Stations, planets, moons,
/// asteroid belts, deep space anomalies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub id: Uuid,
    pub name: String,
    pub location_type: LocationType,
    /// Distance from the star in AU. Drives sublight travel time.
    pub orbital_distance: f32,
    /// Infrastructure at this specific location.
    pub infrastructure: InfrastructureLevel,
    /// Who controls this location (faction ID, may differ from system civ).
    pub controlling_faction: Option<Uuid>,
    /// Economy at this location. Different locations in the same system
    /// can have different prices.
    pub economy: Option<SystemEconomy>,
    /// Short description for display.
    pub description: String,
    /// What the player can do here.
    pub services: Vec<LocationService>,
    /// Is this location known to the player? Hidden locations show as "???".
    #[serde(default = "default_discovered")]
    pub discovered: bool,
}

fn default_discovered() -> bool { true }

/// What kind of place this is.
#[derive(Debug, Clone, Serialize, Deserialize, Display)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum LocationType {
    /// Orbital station — trade hub, military base, guild hall.
    Station,
    /// Planet surface — colony, city, outpost.
    PlanetSurface { body_type: BodyType },
    /// Moon of a planet.
    Moon { parent_body: String, body_type: BodyType },
    /// Asteroid belt — mining ops, hideouts, salvage.
    AsteroidBelt,
    /// Deep space point of interest — anomaly, derelict, signal source.
    DeepSpace,
    /// Megastructure — ringworld, dyson sphere, ancient construct.
    Megastructure { kind: String },
}

/// What the player can do at a location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum LocationService {
    Docking,
    Trade,
    Repair,
    Refuel,
    Contracts,
    Recruitment,
    Rumors,
}

impl LocationType {
    /// Broad category string for encounter matching.
    pub fn category_str(&self) -> &'static str {
        match self {
            LocationType::Station => "station",
            LocationType::PlanetSurface { .. } => "planet_surface",
            LocationType::Moon { .. } => "moon",
            LocationType::AsteroidBelt => "asteroid_belt",
            LocationType::DeepSpace => "deep_space",
            LocationType::Megastructure { .. } => "megastructure",
        }
    }
}

/// Body type for planets, moons, and surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Display, EnumString)]
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

impl InfrastructureLevel {
    /// Numeric rank (0–5). Useful where a scalar comparison or
    /// arithmetic on level is needed. For simple ≤/≥ comparisons
    /// prefer the derived `PartialOrd` directly.
    pub fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Outpost => 1,
            Self::Colony => 2,
            Self::Established => 3,
            Self::Hub => 4,
            Self::Capital => 5,
        }
    }

    /// Normalized value (0.0–1.0) for use in equilibrium, scoring,
    /// and economic calculations.
    pub fn value(self) -> f64 {
        match self {
            Self::None => 0.0,
            Self::Outpost => 0.2,
            Self::Colony => 0.4,
            Self::Established => 0.6,
            Self::Hub => 0.8,
            Self::Capital => 1.0,
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
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

// ===========================================================================
// CIVILIZATIONS
// ===========================================================================

/// A civilization — a galactic-scale polity that controls territory,
/// fields a military, and conducts diplomacy. This is the macro-layer
/// entity that ticks forward with galactic time.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Civilization {
    pub id: Uuid,
    pub name: String,
    /// The civilization's governing philosophy — simulation parameters,
    /// not just labels. Drives behavior tree priorities.
    pub ethos: CivEthos,
    /// Resources and capability scores that determine what the civ *can* do.
    pub capabilities: CivCapabilities,
    /// How this civ views every other civ it knows about.
    pub relationships: HashMap<Uuid, CivDisposition>,
    /// Internal political state — pressures, movements, stability.
    /// Pressures are now grounded in real faction entities pulling
    /// the civ in different directions.
    pub internal_dynamics: InternalDynamics,
    /// Factions operating within this civilization's borders.
    /// Transnational factions may appear in multiple civs' lists.
    pub faction_ids: Vec<Uuid>,
}

/// Weighted values representing a civilization's governing philosophy.
/// Each is 0.0-1.0. These drive behavior tree priorities in the simulation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CivEthos {
    pub expansionist: f32,
    pub isolationist: f32,
    pub militaristic: f32,
    pub diplomatic: f32,
    pub theocratic: f32,
    pub mercantile: f32,
    pub technocratic: f32,
    pub communal: f32,
}

/// What a civilization can actually do, regardless of what it wants to do.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CivCapabilities {
    /// How many systems, how large the population.
    pub size: f32,
    /// Economic output and reserves.
    pub wealth: f32,
    /// Sophistication of technology.
    pub technology: f32,
    /// Military strength — ships, weapons, personnel.
    pub military: f32,
}

/// How one civilization views another across multiple dimensions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CivDisposition {
    /// Diplomatic warmth or hostility (-1.0 to 1.0).
    pub diplomatic: f32,
    /// Economic entanglement (0.0 = none, 1.0 = deeply interdependent).
    pub economic: f32,
    /// Military tension (-1.0 = active war, 0.0 = neutral, 1.0 = alliance).
    pub military: f32,
}

/// Internal pressures and political state within a civilization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalDynamics {
    /// 0.0 = on the verge of collapse, 1.0 = rock solid.
    pub stability: f32,
    /// Active internal movements or pressures. Each may be linked
    /// to a faction entity that is driving the pressure.
    pub pressures: Vec<CivPressure>,
}

/// An internal pressure within a civilization. May be linked to a
/// specific faction entity, or may be a broader societal force.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CivPressure {
    /// Human-readable description of the pressure.
    pub description: String,
    /// The faction driving this pressure, if any.
    /// When populated, the pressure's intensity correlates with
    /// the faction's influence within the civ.
    pub source_faction: Option<Uuid>,
}

// ===========================================================================
// FACTIONS
// ===========================================================================

/// A faction — a group with an agenda operating within or across
/// civilizations. Factions are the primary entities the player
/// interacts with at a personal level: joining, trading, doing
/// missions, building reputation.
///
/// Factions have influence within their parent civ(s) and an
/// independent relationship with the player. Their ethos may align
/// with or oppose their parent civ's governing philosophy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Faction {
    pub id: Uuid,
    pub name: String,
    /// What kind of group this is — political, military, economic, etc.
    pub category: FactionCategory,
    /// Whether this faction operates within one civ or across many.
    pub scope: FactionScope,
    /// Core values and approach — simpler than civ-level ethos.
    pub ethos: FactionEthos,
    /// Influence score within each civ (0.0-1.0), keyed by civ ID.
    /// For civ-internal factions: one entry.
    /// For transnational: one entry per civ they operate in.
    pub influence: HashMap<Uuid, f32>,
    /// How this faction regards the player.
    pub player_standing: FactionStanding,
    /// Free-text description for LLM context and encounter generation.
    pub description: String,
    /// Key NPCs, assets, or resources this faction controls.
    /// Used by the encounter pipeline to generate faction-specific content.
    pub notable_assets: Vec<String>,
}

/// Broad category of a faction. Drives what services they offer,
/// what missions they generate, and how the encounter pipeline uses them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum FactionCategory {
    /// Formal political parties, reform movements, governing councils.
    Political,
    /// Military organizations, defense forces, mercenary companies.
    Military,
    /// Trade guilds, merchant councils, industrial combines.
    Economic,
    /// Smuggler networks, pirate cartels, black markets.
    Criminal,
    /// Churches, cults, spiritual movements.
    Religious,
    /// Universities, research institutes, knowledge-seekers.
    Academic,
    /// Professional guilds — pilots, engineers, medics.
    /// Mechanically identical to transnational economic factions
    /// but with a service/craft orientation rather than political power.
    Guild,
}

/// Whether a faction is contained within one civilization or
/// operates across multiple.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FactionScope {
    /// Operates within a single civilization.
    CivInternal {
        civ_id: Uuid,
    },
    /// Spans multiple civilizations — merchant guilds, religious
    /// orders, criminal networks, intelligence agencies.
    Transnational {
        /// The civs this faction has a presence in.
        /// Each should have a corresponding entry in `influence`.
        civ_ids: Vec<Uuid>,
    },
    /// Not affiliated with any civilization — independent operators,
    /// frontier groups, entities in unclaimed space.
    Independent,
}

/// A faction's core values — simpler than civ-level ethos.
/// Three axes that determine behavior and compatibility.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FactionEthos {
    /// How aligned with established power.
    /// -1.0 = revolutionary, 0.0 = pragmatic, 1.0 = loyalist.
    pub alignment: f32,
    /// Willingness to work with outsiders including the player.
    /// 0.0 = insular, 1.0 = welcoming.
    pub openness: f32,
    /// Preferred methods of operation.
    /// 0.0 = subtle/diplomatic, 1.0 = direct/forceful.
    pub aggression: f32,
}

// ---------------------------------------------------------------------------
// Faction presence on systems
// ---------------------------------------------------------------------------

/// A faction's footprint in a specific star system.
/// Determines what the player can access and how visible the
/// faction is to casual observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactionPresence {
    pub faction_id: Uuid,
    /// How established the faction is here (0.0-1.0).
    /// Higher = more resources, more reliable services.
    pub strength: f32,
    /// How openly the faction operates (0.0-1.0).
    /// Low visibility means the player needs to know where to
    /// look (or have sufficient faction standing) to find them.
    pub visibility: f32,
    /// What the player can access through this faction here.
    pub services: Vec<FactionService>,
}

/// Services a faction can provide at a system where they're present.
/// Availability may depend on player standing with the faction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum FactionService {
    /// Faction-specific jobs and contracts.
    Missions,
    /// Buy/sell goods, possibly at faction-specific prices.
    Trade,
    /// Information about systems, civs, other factions, mission clues.
    Intelligence,
    /// Ship repair, possibly using faction-specific tech.
    Repair,
    /// Move contraband or the player across borders.
    Smuggling,
    /// Improve crew skills or player capabilities.
    Training,
    /// Safe harbor — reduced attention from hostile civs.
    Shelter,
}

// ===========================================================================
// PLAYER STANDING
// ===========================================================================

/// How a civilization regards the player.
/// Affects border access, legal status, trade prices, military response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CivStanding {
    /// Overall reputation: -1.0 (hostile) to 1.0 (allied).
    pub reputation: f32,
    /// Legal classification — determines how civ authorities treat the player.
    pub legal_status: LegalStatus,
    /// Price multiplier for trade in this civ's territory.
    /// < 1.0 = favorable, 1.0 = standard, > 1.0 = gouged.
    pub trade_modifier: f32,
}

impl CivStanding {
    /// Default standing for a newly encountered civilization.
    pub fn neutral() -> Self {
        Self {
            reputation: 0.0,
            legal_status: LegalStatus::Neutral,
            trade_modifier: 1.0,
        }
    }

    /// Starting standing with the player's home civilization.
    pub fn home_civ() -> Self {
        Self {
            reputation: 0.3,
            legal_status: LegalStatus::Licensed,
            trade_modifier: 0.95,
        }
    }
}

/// How the civilization's legal system classifies the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum LegalStatus {
    /// Full member of the civilization. Best prices, full access.
    Citizen,
    /// Authorized to operate in civ space. Good standing.
    Licensed,
    /// No special status. Standard treatment.
    Neutral,
    /// Under increased scrutiny. Random inspections, restricted areas.
    Suspect,
    /// Active warrant. Civ security will attempt arrest.
    Wanted,
    /// Declared hostile. Military engagement authorized.
    Enemy,
}

/// How a faction regards the player.
/// Affects mission availability, information access, services.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactionStanding {
    /// Overall reputation: -1.0 (hostile) to 1.0 (allied).
    pub reputation: f32,
    /// The player's rank within this faction's hierarchy.
    pub rank: FactionRank,
    /// How many jobs/missions the player has completed for this faction.
    pub missions_completed: u32,
}

impl FactionStanding {
    /// Default standing — the faction doesn't know the player exists.
    pub fn unknown() -> Self {
        Self {
            reputation: 0.0,
            rank: FactionRank::Unknown,
            missions_completed: 0,
        }
    }
}

/// The player's position within a faction's hierarchy.
/// Higher ranks unlock better missions, intelligence, and services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum FactionRank {
    /// The faction doesn't know the player.
    Unknown,
    /// The faction is aware of the player — first contact made.
    Contact,
    /// The player has done work for the faction. Basic trust.
    Associate,
    /// Full membership. Access to most faction resources.
    Member,
    /// Proven loyalty. Access to sensitive information and assets.
    Trusted,
    /// The highest rank. Part of the faction's leadership circle.
    InnerCircle,
}

// ===========================================================================
// SYSTEM ECONOMY
// ===========================================================================

/// Trade good categories available in the galaxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum TradeGood {
    Food,
    RawMaterials,
    ManufacturedGoods,
    MedicalSupplies,
    ConstructionMaterials,
    RefinedFuelCells,
}

impl TradeGood {
    /// Base price per unit when supply and demand are balanced.
    pub fn base_price(&self) -> f64 {
        match self {
            TradeGood::Food => 10.0,
            TradeGood::RawMaterials => 15.0,
            TradeGood::ManufacturedGoods => 30.0,
            TradeGood::MedicalSupplies => 25.0,
            TradeGood::ConstructionMaterials => 20.0,
            TradeGood::RefinedFuelCells => 18.0,
        }
    }

    /// All trade good variants, for iteration.
    pub fn all() -> &'static [TradeGood] {
        &[
            TradeGood::Food,
            TradeGood::RawMaterials,
            TradeGood::ManufacturedGoods,
            TradeGood::MedicalSupplies,
            TradeGood::ConstructionMaterials,
            TradeGood::RefinedFuelCells,
        ]
    }

    /// Display name for the trade screen.
    pub fn display_name(&self) -> &'static str {
        match self {
            TradeGood::Food => "Food",
            TradeGood::RawMaterials => "Raw materials",
            TradeGood::ManufacturedGoods => "Manufactured goods",
            TradeGood::MedicalSupplies => "Medical supplies",
            TradeGood::ConstructionMaterials => "Construction materials",
            TradeGood::RefinedFuelCells => "Refined fuel cells",
        }
    }
}

/// A system's economic profile — drives trade prices and availability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemEconomy {
    /// What this system produces (0.0 = imports everything, 1.0 = major exporter).
    pub production: HashMap<TradeGood, f32>,
    /// What this system needs (0.0 = no demand, 1.0 = desperate need).
    pub consumption: HashMap<TradeGood, f32>,
    /// How much prices swing. Frontier = high, capital = low.
    pub price_volatility: f32,
    /// Credits per fuel unit.
    pub fuel_price: f32,
    /// Credits per supply unit.
    pub supply_price: f32,
}

impl SystemEconomy {
    /// Calculate the buy price for a trade good at this system.
    /// Higher consumption and lower production = more expensive.
    pub fn buy_price(&self, good: TradeGood) -> f64 {
        let base = good.base_price();
        let prod = *self.production.get(&good).unwrap_or(&0.5);
        let cons = *self.consumption.get(&good).unwrap_or(&0.5);
        // Price goes up with consumption, down with production.
        let modifier = 1.0 + (cons - prod) as f64 * self.price_volatility as f64;
        (base * modifier.max(0.3)).round()
    }

    /// Sell price is always less than buy price (the spread).
    /// Systems pay more for goods they consume heavily.
    pub fn sell_price(&self, good: TradeGood) -> f64 {
        (self.buy_price(good) * 0.75).round()
    }

    /// How much of a good is available to buy.
    /// High production = plenty, low production = limited.
    pub fn availability(&self, good: TradeGood) -> Availability {
        let prod = *self.production.get(&good).unwrap_or(&0.0);
        if prod >= 0.7 {
            Availability::Plenty
        } else if prod >= 0.4 {
            Availability::Moderate
        } else if prod >= 0.15 {
            Availability::Limited
        } else {
            Availability::Unavailable
        }
    }
}

/// How much of a trade good is available for purchase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Plenty,
    Moderate,
    Limited,
    Unavailable,
}

impl std::fmt::Display for Availability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Availability::Plenty => write!(f, "plenty"),
            Availability::Moderate => write!(f, "moderate"),
            Availability::Limited => write!(f, "limited"),
            Availability::Unavailable => write!(f, "unavailable"),
        }
    }
}