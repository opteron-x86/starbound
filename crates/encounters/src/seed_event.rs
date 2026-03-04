// file: crates/encounters/src/seed_event.rs
//! Seed event types — the data model for hand-crafted encounter content.
//!
//! Each seed event is a self-contained piece of narrative content with
//! context requirements that determine when it can fire. The encounter
//! pipeline selects events whose requirements match the current game state.
//!
//! Effects are defined inline on each choice as structured data. The game
//! engine converts these `EffectDef` values into `Effect` enums and applies
//! them to the journey state. This keeps all content in JSON — no Rust
//! changes needed to add new events.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Seed events
// ---------------------------------------------------------------------------

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
    /// The text the player reads. Supports template placeholders like
    /// `{system.name}`, `{faction.name}`, `{ship.name}`, etc.
    pub text: String,
    /// The choices available to the player.
    pub choices: Vec<SeedChoice>,
    /// Which player intents this event can resolve.
    /// Empty = arrival-only (traditional pipeline behavior).
    /// Non-empty = this event fires when the player initiates a matching action.
    #[serde(default)]
    pub intents: Vec<String>,
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

    // -------------------------------------------------------------------
    // Faction presence requirements (Phase C)
    // -------------------------------------------------------------------

    /// A faction of this category must be present at the current system.
    /// Uses the FactionCategory string representation: "military",
    /// "economic", "guild", "religious", "criminal".
    pub faction_category_present: Option<String>,
    /// The matching faction presence must have at least this strength.
    pub faction_min_strength: Option<f32>,
    /// The matching faction presence must have at least this visibility.
    /// Events gated on low visibility create "hidden world" encounters
    /// that players discover by exploring the margins.
    pub faction_max_visibility: Option<f32>,
    /// The system's time distortion factor must be at least this value.
    /// Used for encounters tied to anomalous spacetime.
    pub time_factor_min: Option<f64>,
}

// ---------------------------------------------------------------------------
// Choices and effects
// ---------------------------------------------------------------------------

/// A choice the player can make in response to an encounter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedChoice {
    /// What the player sees.
    pub label: String,
    /// The effects this choice produces — inline, structured, data-driven.
    pub effects: Vec<EffectDef>,
    /// Tone guidance for the LLM when narrating the outcome.
    #[serde(default)]
    pub tone_note: String,
    /// Optional follow-up event triggered after this choice resolves.
    #[serde(default)]
    pub follows: Option<FollowUp>,
}

/// A single effect definition as authored in event JSON.
/// Converted to the game's `Effect` enum for application.
///
/// Tagged enum — JSON uses `{"type": "fuel", "delta": 20.0}` format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectDef {
    /// Add or remove fuel. Clamped to [0, capacity].
    Fuel { delta: f32 },
    /// Add or remove supplies. Clamped to [0, capacity].
    Supplies { delta: f32 },
    /// Add or remove generic resources (credits/trade goods).
    Resources { delta: f64 },
    /// Add or remove hull condition. Clamped to [0.0, 1.0].
    Hull { delta: f32 },
    /// Adjust stress for all crew. Clamped to [0.0, 1.0].
    CrewStress { delta: f32 },
    /// Set mood for a crew member (most stressed) or all crew.
    CrewMood {
        mood: String,
        #[serde(default)]
        all: bool,
    },
    /// Adjust professional trust for all crew toward the captain.
    TrustProfessional { delta: f32 },
    /// Adjust personal trust for all crew toward the captain.
    TrustPersonal { delta: f32 },
    /// Adjust ideological trust for all crew toward the captain.
    TrustIdeological { delta: f32 },
    /// Spawn a new narrative thread.
    SpawnThread {
        thread_type: String,
        description: String,
    },
    /// Add a cargo item.
    AddCargo { item: String, quantity: u32 },
    /// Remove all cargo (jettison).
    JettisonCargo {},
    /// Damage a specific ship module. Amount subtracted from condition.
    DamageModule { module: String, amount: f32 },
    /// Repair a specific ship module. Amount added to condition.
    RepairModule { module: String, amount: f32 },
    /// Add a concern to a random crew member's active concerns.
    AddConcern { text: String },
    /// Log a narrative note (no mechanical change, but appears in the log).
    Narrative { text: String },
    /// No mechanical effect — the choice was about tone, not state.
    Pass {},
}

// ---------------------------------------------------------------------------
// Event chaining
// ---------------------------------------------------------------------------

/// A follow-up event triggered by a choice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FollowUp {
    /// ID of the next event to trigger.
    pub event_id: String,
    /// When does the follow-up fire?
    #[serde(default)]
    pub delay: FollowUpDelay,
}

/// When a follow-up event fires.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowUpDelay {
    /// Show immediately after this choice resolves.
    #[default]
    Immediate,
    /// Fire on next arrival at any system.
    NextArrival,
}