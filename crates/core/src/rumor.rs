// file: crates/core/src/rumor.rs
//! Rumors — actionable information the player discovers while docked.
//!
//! Rumors are mechanically generated from live game state, then optionally
//! flavored by the LLM. The content is real — real prices, real faction
//! tensions, real thread hooks. The LLM shapes delivery, not substance.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Core rumor types
// ---------------------------------------------------------------------------

/// A piece of actionable information the player can discover.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rumor {
    pub id: Uuid,
    pub category: RumorCategory,
    /// The core fact — what the player learns.
    pub content: RumorContent,
    /// Where this rumor was heard (system ID).
    pub source_system: Uuid,
    /// Specific location within the system (location ID).
    pub source_location: Uuid,
    /// When it was generated (galactic days).
    pub generated_at: f64,
    /// How many galactic days until this info is stale.
    pub expires_in: f64,
    /// Probability the info is still accurate (0.0–1.0).
    pub reliability: f64,
    /// Has the player acted on this rumor?
    #[serde(default)]
    pub acted_on: bool,
    /// Was the rumor accurate when the player checked?
    #[serde(default)]
    pub outcome: Option<RumorOutcome>,
    /// Display text — the templated or LLM-generated prose the player reads.
    pub display_text: String,
    /// Short mechanical summary for the rumor log.
    pub summary: String,
}

/// What kind of information this rumor provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RumorCategory {
    /// A profitable trade route between locations.
    TradeTip,
    /// A faction looking for someone to do a job.
    ContractLead,
    /// Political or military developments between factions.
    FactionIntel,
    /// A hook that can spawn a narrative thread.
    ThreadSeed,
    /// A clue advancing the central mystery.
    MissionClue,
    /// Atmospheric detail — world texture, no mechanical payload.
    LocalColor,
}

impl std::fmt::Display for RumorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RumorCategory::TradeTip => write!(f, "Trade tip"),
            RumorCategory::ContractLead => write!(f, "Contract lead"),
            RumorCategory::FactionIntel => write!(f, "Faction intel"),
            RumorCategory::ThreadSeed => write!(f, "Thread seed"),
            RumorCategory::MissionClue => write!(f, "Mission clue"),
            RumorCategory::LocalColor => write!(f, "Local color"),
        }
    }
}

/// The mechanical content of a rumor — what it actually tells the player.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RumorContent {
    TradeTip {
        good: String,
        buy_system: Uuid,
        buy_location: Option<Uuid>,
        sell_system: Uuid,
        sell_location: Option<Uuid>,
        /// Expected profit per unit (sell price − buy price).
        estimated_spread: f64,
    },
    ContractLead {
        faction_id: Uuid,
        contract_type: String,
        destination_system: Option<Uuid>,
        estimated_reward: f64,
        /// The specific NPC offering this work (if known).
        npc_id: Option<Uuid>,
        /// Display name of the NPC (for rumor text).
        npc_name: Option<String>,
    },
    FactionIntel {
        summary: String,
        factions_involved: Vec<Uuid>,
        /// What this means for the player.
        implication: String,
    },
    ThreadSeed {
        description: String,
        related_system: Option<Uuid>,
        thread_type: String,
    },
    MissionClue {
        clue_text: String,
        knowledge_node_hint: Option<String>,
        /// 0.0 = very specific, 1.0 = very vague.
        vagueness: f32,
    },
    LocalColor {
        description: String,
    },
}

/// What happened when the player checked if a rumor was accurate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RumorOutcome {
    /// Info was correct when the player checked.
    Accurate,
    /// Was true but changed before the player arrived.
    Stale,
    /// Was wrong from the start (low reliability).
    Inaccurate,
}

// ---------------------------------------------------------------------------
// Reliability helpers
// ---------------------------------------------------------------------------

impl RumorCategory {
    /// How quickly this type of rumor goes stale (in galactic days).
    /// Trade tips expire fast. Mysteries are patient.
    pub fn default_expiry(&self) -> f64 {
        match self {
            RumorCategory::TradeTip => 180.0,       // ~6 months
            RumorCategory::ContractLead => 365.0,    // ~1 year
            RumorCategory::FactionIntel => 730.0,    // ~2 years
            RumorCategory::ThreadSeed => 3650.0,     // ~10 years
            RumorCategory::MissionClue => 36500.0,   // effectively permanent
            RumorCategory::LocalColor => 365.0,      // ~1 year
        }
    }
}

/// Infrastructure-based reliability for rumor generation.
/// Better infrastructure = more reliable information.
pub fn base_reliability(infrastructure_label: &str) -> f64 {
    match infrastructure_label {
        "capital" => 0.90,
        "hub" => 0.80,
        "established" => 0.70,
        "colony" => 0.55,
        "outpost" => 0.40,
        _ => 0.50,
    }
}

/// How many rumors a location can generate based on infrastructure.
pub fn rumor_count_range(infrastructure_label: &str) -> (usize, usize) {
    match infrastructure_label {
        "capital" => (3, 4),
        "hub" => (2, 4),
        "established" => (2, 3),
        "colony" => (1, 3),
        "outpost" => (1, 2),
        _ => (1, 2),
    }
}
