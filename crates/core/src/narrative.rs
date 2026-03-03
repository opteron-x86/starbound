use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

use crate::time::Timestamp;

// ---------------------------------------------------------------------------
// Thread Ledger
// ---------------------------------------------------------------------------

/// A narrative thread — a dangling story element the encounter pipeline
/// can pick up and weave back into the player's journey.
///
/// The thread ledger is the game's narrative memory. High-tension threads
/// that haven't been addressed and connect to current context are prime
/// candidates for echo encounters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: Uuid,
    pub thread_type: ThreadType,
    /// People, factions, places, objects connected to this thread.
    pub associated_entities: Vec<Uuid>,
    /// How unresolved or charged this thread is (0.0–1.0).
    /// Some threads never fully decay.
    pub tension: f32,
    /// When this thread was created.
    pub created_at: Timestamp,
    /// When something last happened with this thread.
    pub last_touched: Timestamp,
    pub resolution: ResolutionState,
    /// Human/LLM-readable description of the thread.
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ThreadType {
    Relationship,
    Mystery,
    Debt,
    Grudge,
    Promise,
    Secret,
    Anomaly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ResolutionState {
    Open,
    Partial,
    Resolved,
    /// The thread didn't end — it became something else.
    Transformed,
}

// ---------------------------------------------------------------------------
// Game Events
// ---------------------------------------------------------------------------

/// A record of something that happened. Append-only log.
/// Recent events in full, older events summarized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameEvent {
    pub timestamp: Timestamp,
    pub category: EventCategory,
    pub description: String,
    /// Entities involved in this event.
    pub associated_entities: Vec<Uuid>,
    /// What changed as a result — brief mechanical notes.
    pub consequences: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    Encounter,
    Crew,
    Mission,
    Faction,
    Personal,
    Travel,
    Discovery,
}

// ---------------------------------------------------------------------------
// Encounter Briefs
// ---------------------------------------------------------------------------

/// The structured output of the encounter pipeline — a brief handed
/// to the LLM (or seed library) describing what should happen next.
///
/// The simulation decides WHAT happens. The LLM decides HOW it reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncounterBrief {
    pub encounter_type: EncounterType,
    /// Threads from the ledger this encounter should engage with.
    pub relevant_threads: Vec<Uuid>,
    /// The emotional register the encounter should aim for.
    pub tone: Tone,
    /// Key people, places, factions involved.
    pub key_entities: Vec<Uuid>,
    /// What this encounter needs to accomplish mechanically.
    /// e.g. "offer fuel at a cost", "surface mission clue node #12",
    /// "create tension between crew members A and B".
    pub mechanical_goals: Vec<String>,
    /// Additional context for the LLM — free text.
    pub context_notes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum EncounterType {
    /// Something from the past resurfacing.
    Echo,
    /// A new element entering the story.
    Novel,
    /// Driven by current faction/political state.
    Contextual,
    /// Crew-internal dynamics.
    CrewDynamic,
    /// Mission-related discovery.
    MissionClue,
    /// Routine — trade, resupply, minor interaction.
    Mundane,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Tense,
    Quiet,
    Wonder,
    Urgent,
    Melancholy,
    Mundane,
}
