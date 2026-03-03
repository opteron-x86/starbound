// file: crates/encounters/src/seed_event.rs
//! Seed event types — the data model for hand-crafted encounter content.
//!
//! Each seed event is a self-contained piece of narrative content with
//! context requirements that determine when it can fire. The encounter
//! pipeline selects events whose requirements match the current game state.

use serde::{Deserialize, Serialize};

/// A hand-crafted encounter. The gold standard the LLM will eventually
/// riff on, and the fallback when the LLM is unavailable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedEvent {
    /// Unique string identifier.
    pub id: String,
    /// What kind of encounter this is.
    pub encounter_type: String,
    /// The emotional register.
    pub tone: String,
    /// Conditions that must be true for this event to be eligible.
    #[serde(default)]
    pub context_requirements: ContextRequirements,
    /// The text the player reads. This is where tone lives or dies.
    pub text: String,
    /// The choices available to the player.
    pub choices: Vec<SeedChoice>,
}

/// Conditions for an event to fire. All fields are optional —
/// an empty requirements block means the event can fire anywhere.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextRequirements {
    /// Minimum infrastructure level at the current system.
    pub infrastructure_min: Option<String>,
    /// Maximum infrastructure level (for frontier/empty encounters).
    pub infrastructure_max: Option<String>,
    /// System must be controlled by a faction.
    pub faction_controlled: Option<bool>,
    /// System must NOT be controlled by a faction.
    pub unclaimed: Option<bool>,
    /// Minimum galactic years since player last visited this system.
    #[serde(default)]
    pub time_since_last_visit_galactic_years_min: Option<f64>,
    /// Player's fuel must be below this fraction (0.0–1.0) of capacity.
    pub fuel_below_fraction: Option<f32>,
    /// Player's hull must be below this threshold (0.0–1.0).
    pub hull_below: Option<f32>,
    /// Minimum number of crew members.
    pub crew_min: Option<usize>,
    /// Tags for additional filtering (e.g. "frontier", "trade", "ancient").
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A choice the player can make in response to an encounter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedChoice {
    /// What the player sees.
    pub label: String,
    /// What happens mechanically (interpreted by the game loop).
    pub mechanical_effect: String,
    /// Tone guidance for the LLM when narrating the outcome.
    #[serde(default)]
    pub tone_note: String,
}