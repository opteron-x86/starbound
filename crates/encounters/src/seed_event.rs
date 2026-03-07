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
//!
//! ## Redesign additions
//!
//! - **Category**: Purpose-driven classification (`ambient`, `exploration`,
//!   `faction`, `crew`, `main_quest`, `side_quest`, `contract`).
//! - **Priority**: 0–3 tier that affects silence override and score weighting.
//! - **Prerequisites**: Hard gates on threads, cargo, and visited systems.
//! - **New effects**: `faction_standing`, `discover_location`, `resolve_thread`,
//!   `time_cost`, `add_knowledge_node`, `reputation_shift`, `npc_disposition`.

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
    /// What kind of encounter this is (legacy field — use `event_kind` for
    /// the ambient/discovery split). Retained for tone/pacing scoring.
    pub encounter_type: String,
    /// The emotional register.
    pub tone: String,
    /// Purpose-driven classification.
    ///
    /// Determines which file the event lives in and how the pipeline
    /// treats it (e.g. main_quest events get priority overrides).
    ///
    /// Values: `ambient`, `exploration`, `faction`, `crew`,
    ///         `main_quest`, `side_quest`, `contract`
    #[serde(default = "default_category")]
    pub category: String,
    /// Priority tier (0–3). Higher priority events override the silence
    /// check and receive a scoring bonus in the pipeline.
    ///
    /// - 0 = ambient (can be skipped, silence is fine)
    /// - 1 = normal (standard encounter behavior)
    /// - 2 = important (convergence events, side quest progression)
    /// - 3 = critical (main quest events — always fire when eligible)
    #[serde(default)]
    pub priority: u8,
    /// Conditions that must be true for this event to be eligible.
    #[serde(default)]
    pub context_requirements: ContextRequirements,
    /// The text the player reads. Supports template placeholders like
    /// `{system.name}`, `{faction.name}`, `{ship.name}`, etc.
    pub text: String,
    /// The choices available to the player.
    pub choices: Vec<SeedChoice>,
    /// Which player intents this event can resolve (legacy field).
    /// Replaced by `trigger` for new events. Retained for backward
    /// compatibility — events with intents derive their trigger as
    /// `Action(intents[0])` when `trigger` is absent.
    #[serde(default)]
    pub intents: Vec<String>,

    // --- Encounter system redesign fields ---

    /// When this event can fire. Replaces the implicit arrival-vs-intent
    /// distinction with an explicit trigger type.
    ///
    /// Defaults: if absent in JSON, derived from `intents`:
    /// - intents non-empty → `action:{intents[0]}`
    /// - intents empty → `arrival`
    #[serde(default)]
    pub trigger: EventTrigger,
    /// Whether this is ambient (texture, small moments) or discovery
    /// (player-initiated investigation with meaningful stakes).
    ///
    /// Defaults: if absent, derived from `intents`:
    /// - intents non-empty → `discovery`
    /// - intents empty → `ambient`
    #[serde(default)]
    pub event_kind: EventKind,
}

fn default_category() -> String {
    "ambient".into()
}

// ---------------------------------------------------------------------------
// Event trigger — when an event can fire
// ---------------------------------------------------------------------------

/// When an event fires. Each event declares its trigger type.
/// The pipeline receives a trigger and only considers matching events.
///
/// Serialized as a string tag: "arrival", "transit", "docked", "linger",
/// or "action:tag" for player-initiated actions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EventTrigger {
    /// Fires on FTL arrival at a system or sublight arrival at a location.
    /// The classic trigger — most legacy events use this.
    Arrival,
    /// Fires during sublight transit between locations within a system.
    /// Ambient events only — small crew/environment moments.
    Transit,
    /// Fires when the player docks at a station or lands at a location.
    /// Ambient events — station life, colony atmosphere.
    Docked,
    /// Fires when the player spends time at a location.
    /// Slightly higher chance than transit/docked.
    Linger,
    /// Fires in response to a specific player action.
    /// The string tag matches against the action type:
    /// "scan", "investigate", "board", "explore", "recover", "follow_lead",
    /// "trade", "repair", "resupply", etc.
    Action(String),
}

impl EventTrigger {
    /// Base silence rate for this trigger type. Higher = more likely to
    /// produce silence (no event). Action triggers never silence.
    pub fn base_silence_rate(&self) -> f64 {
        match self {
            EventTrigger::Arrival => 0.50,  // Half of arrivals are just arrivals.
            EventTrigger::Transit => 0.85,  // ~15% fire rate. Most transits are quiet.
            EventTrigger::Docked => 0.80,   // ~20% fire rate. Most docks go straight to menu.
            EventTrigger::Linger => 0.60,   // ~40% fire rate. Player chose to linger.
            EventTrigger::Action(_) => 0.0, // Player chose to act. Always respond.
        }
    }

    /// Whether the player chose this trigger (i.e. silence is not allowed).
    pub fn is_player_action(&self) -> bool {
        matches!(self, EventTrigger::Action(_))
    }

    /// The action tag, if this is an Action trigger.
    pub fn action_tag(&self) -> Option<&str> {
        match self {
            EventTrigger::Action(tag) => Some(tag.as_str()),
            _ => None,
        }
    }

    /// Short display label for logging.
    pub fn label(&self) -> String {
        match self {
            EventTrigger::Arrival => "arrival".into(),
            EventTrigger::Transit => "transit".into(),
            EventTrigger::Docked => "docked".into(),
            EventTrigger::Linger => "linger".into(),
            EventTrigger::Action(tag) => format!("action:{}", tag),
        }
    }
}

impl Default for EventTrigger {
    fn default() -> Self {
        EventTrigger::Arrival
    }
}

/// Custom serialization: "arrival", "transit", "docked", "linger", "action:scan"
impl Serialize for EventTrigger {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.label())
    }
}

impl<'de> Deserialize<'de> for EventTrigger {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "arrival" => EventTrigger::Arrival,
            "transit" => EventTrigger::Transit,
            "docked" => EventTrigger::Docked,
            "linger" => EventTrigger::Linger,
            other if other.starts_with("action:") => {
                EventTrigger::Action(other.strip_prefix("action:").unwrap().to_string())
            }
            // Legacy: bare action tags without prefix.
            other => EventTrigger::Action(other.to_string()),
        })
    }
}

// ---------------------------------------------------------------------------
// Event kind — ambient vs discovery
// ---------------------------------------------------------------------------

/// Whether an event is ambient (texture) or discovery (player-initiated,
/// meaningful stakes). Affects silence behavior and pipeline treatment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Texture events — small moments during transit, docking, lingering.
    /// Can be silenced. Low stakes, crew/relationship/environmental focus.
    Ambient,
    /// Player-initiated investigation with meaningful stakes — clues,
    /// resources, danger, threads. Should not be silenced.
    Discovery,
}

impl Default for EventKind {
    fn default() -> Self {
        EventKind::Ambient
    }
}

impl EventKind {
    pub fn label(&self) -> &'static str {
        match self {
            EventKind::Ambient => "ambient",
            EventKind::Discovery => "discovery",
        }
    }
}

// ---------------------------------------------------------------------------
// SeedEvent — derived accessors for backward compatibility
// ---------------------------------------------------------------------------

impl SeedEvent {
    /// The effective trigger for this event.
    ///
    /// If `trigger` was explicitly set in JSON, uses that.
    /// Otherwise derives from the legacy `intents` field:
    /// - intents non-empty → `Action(intents[0])`
    /// - intents empty → `Arrival`
    pub fn effective_trigger(&self) -> EventTrigger {
        // If trigger was explicitly set to something other than the default
        // Arrival, use it. Otherwise fall back to intents-based derivation.
        if self.trigger != EventTrigger::Arrival {
            return self.trigger.clone();
        }
        // Derive from legacy intents.
        if let Some(first) = self.intents.first() {
            EventTrigger::Action(first.clone())
        } else {
            EventTrigger::Arrival
        }
    }

    /// The effective event kind.
    ///
    /// If `event_kind` was explicitly set, uses that.
    /// Otherwise derives: events with intents are Discovery, others Ambient.
    pub fn effective_kind(&self) -> EventKind {
        // If explicitly set to Discovery, honor it.
        if self.event_kind == EventKind::Discovery {
            return EventKind::Discovery;
        }
        // Derive from intents.
        if !self.intents.is_empty() {
            EventKind::Discovery
        } else {
            EventKind::Ambient
        }
    }

    /// Whether this event matches a given trigger.
    ///
    /// Action triggers match on the tag string.
    /// Other triggers match exactly.
    pub fn matches_trigger(&self, trigger: &EventTrigger) -> bool {
        let effective = self.effective_trigger();
        match (trigger, &effective) {
            // Action triggers match on tag.
            (EventTrigger::Action(want), EventTrigger::Action(have)) => want == have,
            // Non-action triggers match exactly.
            _ => *trigger == effective,
        }
    }
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

    // -------------------------------------------------------------------
    // Location type requirements (intra-system navigation)
    // -------------------------------------------------------------------

    /// Location types where this event can fire.
    /// Empty = fires anywhere. Non-empty = only at these location types.
    /// Uses LocationType category strings: "station", "planet_surface",
    /// "moon", "asteroid_belt", "deep_space", "megastructure".
    #[serde(default)]
    pub location_types: Vec<String>,

    // -------------------------------------------------------------------
    // Prerequisites — hard gates for quest progression
    // -------------------------------------------------------------------

    /// Hard prerequisites that must ALL be met for this event to be
    /// eligible. Unlike other context requirements which affect scoring,
    /// unmet prerequisites eliminate the event entirely.
    ///
    /// This is the mechanism for quest progression: a main quest event
    /// that requires three signal threads won't fire until the player
    /// has accumulated them through any combination of encounters.
    #[serde(default)]
    pub prerequisites: Option<Prerequisites>,
}

/// Hard prerequisite gates for event eligibility.
///
/// All specified conditions must be met. Unspecified conditions are
/// ignored (vacuously true). The matcher checks these before any
/// scoring happens — failed prerequisites mean the event never
/// enters the candidate pool.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Prerequisites {
    /// Require N open threads of a specific type.
    /// e.g. `{"thread_type": "anomaly", "min_count": 2}`
    #[serde(default)]
    pub threads_with_type: Option<ThreadCountReq>,
    /// Require N open threads whose description contains a keyword.
    /// e.g. `{"tag": "signal", "min_count": 3}`
    #[serde(default)]
    pub threads_with_tag: Option<ThreadTagReq>,
    /// A specific thread description substring must be active (Open or Partial).
    /// Matched against thread descriptions case-insensitively.
    #[serde(default)]
    pub thread_active: Option<String>,
    /// Player must have this item in cargo.
    #[serde(default)]
    pub cargo_contains: Option<String>,
    /// Player must have visited a system whose name contains this string.
    #[serde(default)]
    pub has_visited_system: Option<String>,
    /// Player must have at least one active contract.
    #[serde(default)]
    pub contract_active: Option<bool>,
    /// Player standing with a faction category must be at least this value.
    #[serde(default)]
    pub faction_standing_min: Option<FactionStandingReq>,
}

/// Require a minimum count of open threads with a specific type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadCountReq {
    pub thread_type: String,
    pub min_count: usize,
}

/// Require a minimum count of open threads tagged with a keyword.
/// "Tag" here means the thread description contains the keyword.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadTagReq {
    pub tag: String,
    pub min_count: usize,
}

/// Require minimum standing with a faction category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactionStandingReq {
    pub faction_category: String,
    pub min_reputation: f32,
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
    // --- Existing effects (unchanged) ---

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

    // --- New effects (encounter system redesign) ---

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