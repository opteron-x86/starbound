// file: crates/core/src/effects.rs
//! Shared effect types — the vocabulary for encounter consequences.
//!
//! `EffectDef` is the JSON-authored data format. `ModuleTarget` identifies
//! ship modules across effect application and skill checks. `FollowUp`
//! and `FollowUpDelay` handle event chaining.
//!
//! These types live in core because they're consumed by encounters
//! (seed library), game (consequence engine), llm (response parsing),
//! and cli (display). They're pure data — no game logic.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Effect definitions (JSON-authored)
// ---------------------------------------------------------------------------

/// A single effect definition as authored in event JSON.
/// Converted to the game's `Effect` enum for application.
///
/// Tagged enum — JSON uses `{"type": "fuel", "delta": 20.0}` format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectDef {
    // --- Core effects ---

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

    // --- Extended effects ---

    /// Change the player's standing with a faction category.
    /// Positive delta improves relations, negative worsens.
    FactionStanding {
        /// Which faction category: "military", "economic", "guild",
        /// "religious", "criminal"
        faction_category: String,
        delta: f32,
    },
    /// Reveal a hidden location in the current system.
    /// If name matches an existing undiscovered location, it becomes
    /// discovered. Otherwise this is a narrative note.
    DiscoverLocation {
        name: String,
        #[serde(default)]
        description: Option<String>,
    },
    /// Close or transform an existing thread.
    ResolveThread {
        /// Thread type to match ("mystery", "anomaly", etc.)
        thread_type: String,
        /// Keyword from the thread description to identify which thread.
        keyword: String,
        /// Target state: "resolved" or "transformed"
        #[serde(default = "default_resolution")]
        to_state: String,
    },
    /// Advance the main quest by adding a knowledge node.
    AddKnowledgeNode {
        /// The narrative content of this discovery.
        content: String,
    },
    /// Some choices cost personal/galactic time.
    TimeCost {
        /// Hours of personal time consumed.
        hours: f64,
    },
    /// Shift the player's behavioral profile reputation.
    /// This adjusts emergent labels like "explorer", "trader", "pirate".
    ReputationShift {
        /// The label to shift: "explorer", "trader", "diplomat",
        /// "fighter", "pirate", "scholar"
        label: String,
        delta: f32,
    },
    /// Change an NPC's disposition toward the player.
    NpcDisposition {
        /// NPC identifier (matched by name at current system).
        npc_name: String,
        delta: f32,
    },
}

fn default_resolution() -> String {
    "resolved".into()
}

// ---------------------------------------------------------------------------
// Ship module targeting
// ---------------------------------------------------------------------------

/// Which ship module an effect targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleTarget {
    Engine,
    Sensors,
    Comms,
    Weapons,
    LifeSupport,
}

impl ModuleTarget {
    /// Human-readable display name for this module.
    pub fn name(self) -> &'static str {
        match self {
            ModuleTarget::Engine => "Engine",
            ModuleTarget::Sensors => "Sensors",
            ModuleTarget::Comms => "Comms",
            ModuleTarget::Weapons => "Weapons",
            ModuleTarget::LifeSupport => "Life support",
        }
    }
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
