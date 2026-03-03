use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

/// A crew member — the most important relationships in the game.
/// They share your subjective timeline while the galaxy ages around you.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewMember {
    pub id: Uuid,
    pub name: String,
    pub role: CrewRole,
    /// Core personality drives — invisible to the player, manifest
    /// through behavior. The player builds a mental model through
    /// observation, and that model might be wrong.
    pub drives: PersonalityDrives,
    /// Multidimensional, asymmetric trust toward the captain.
    pub trust: Trust,
    /// How this crew member relates to every other crew member.
    pub relationships: HashMap<Uuid, CrewRelationship>,
    /// Background text — LLM fuel for generating authentic dialogue.
    pub background: String,
    /// Current emotional and psychological state.
    pub state: CrewState,
    /// How this person joined the crew.
    pub origin: CrewOrigin,
}

/// Functional role on the ship. Determines duty station, expertise,
/// and which encounters they're most relevant to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum CrewRole {
    Navigator,
    Engineer,
    Comms,
    Medic,
    Science,
    Security,
    Pilot,
    Quartermaster,
    General,
}

/// The six core drives, each weighted 0.0–1.0.
/// These are simulation parameters — the player never sees numbers,
/// only behavior that emerges from them.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PersonalityDrives {
    /// Safety, stability, predictability.
    pub security: f32,
    /// Autonomy, independence, self-determination.
    pub freedom: f32,
    /// Meaning, contribution, being needed.
    pub purpose: f32,
    /// Relationships, belonging, loyalty.
    pub connection: f32,
    /// Understanding, discovery, truth.
    pub knowledge: f32,
    /// Fairness, principle, moral integrity.
    pub justice: f32,
}

/// Trust is multidimensional and asymmetric. Someone might trust you
/// professionally but not personally. Each dimension is -1.0 to 1.0.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Trust {
    /// Does this person believe you're competent?
    /// Built through demonstrated good judgment.
    /// Eroded by preventable disasters.
    pub professional: f32,
    /// Does this person feel safe being vulnerable with you?
    /// Built through time and honoring confidences.
    /// Slow to build, hard to repair.
    pub personal: f32,
    /// Does this person believe you share or respect their values?
    /// Most volatile — a single decision can shatter it.
    pub ideological: f32,
}

impl Trust {
    /// Starting trust for someone who already knows the captain.
    pub fn starting_crew() -> Self {
        Self {
            professional: 0.3,
            personal: 0.2,
            ideological: 0.1,
        }
    }

    /// Starting trust for a new recruit — blank slate.
    pub fn new_recruit() -> Self {
        Self {
            professional: 0.0,
            personal: 0.0,
            ideological: 0.0,
        }
    }
}

/// How two crew members relate to each other, independent of the captain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewRelationship {
    /// Overall warmth or friction (-1.0 to 1.0).
    pub rapport: f32,
    /// The nature of the relationship.
    pub dynamic: RelationshipDynamic,
    /// Freeform notes for LLM context.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum RelationshipDynamic {
    Strangers,
    Acquaintances,
    Colleagues,
    Friends,
    CloseFriends,
    Rivals,
    Romantic,
    Mentor,
    Protege,
    Estranged,
}

/// Current emotional and psychological state. Shifts constantly
/// based on events, crew dynamics, and the weight of the journey.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrewState {
    pub mood: Mood,
    /// 0.0 = calm, 1.0 = breaking point.
    pub stress: f32,
    /// What's on their mind right now — drives conversation and reactions.
    pub active_concerns: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Mood {
    Content,
    Anxious,
    Determined,
    Grieving,
    Restless,
    Hopeful,
    Withdrawn,
    Angry,
    Inspired,
}

/// How this crew member came to be on the ship.
/// Starting crew are anchors — losing one feels like losing identity.
/// Recruited crew carry the context of where and when you found them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CrewOrigin {
    /// Knew the captain before the journey.
    Starting,
    /// Joined at a specific place and time.
    Recruited {
        system_id: Uuid,
        galactic_day_joined: f64,
    },
}
