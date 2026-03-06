// file: crates/core/src/npc.rs
//! NPC data types — named people who persist in the galaxy.
//!
//! NPCs live at systems, have faction ties, and remember the player.
//! They age in galactic time and can die between visits.
//!
//! ## Social layer
//!
//! Each NPC has:
//! - **Identity**: species, pronouns, cultural origin, background
//! - **Personality**: warmth/boldness/idealism axes that shape behavior
//! - **Social state**: disposition toward player, connections to other
//!   NPCs, knowledge of local events, interaction memory

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Core NPC struct
// ---------------------------------------------------------------------------

/// A persistent NPC who exists at a specific system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Npc {
    pub id: Uuid,
    pub name: String,
    /// Role or position — "Guild Factor", "Station Master", etc.
    pub title: String,
    /// Which faction this NPC belongs to, if any.
    pub faction_id: Option<Uuid>,
    /// The system where this NPC lives.
    pub home_system_id: Uuid,
    /// The specific location within the system (station, planet, etc.).
    pub home_location_id: Option<Uuid>,
    /// Is this NPC still alive?
    pub alive: bool,

    // --- Identity (generated once, mostly static) ---

    /// Biological or constructed category. Drives portrait selection,
    /// pronoun usage, and species-specific interaction flavor.
    pub species: Species,
    /// Pronoun set for text generation. Derived from species + sex
    /// at generation time, stored flat for easy template access.
    pub pronouns: Pronouns,
    /// A short description — 2-3 sentences of context.
    pub bio: String,
    /// Which civilization this NPC comes from culturally.
    /// Affects name style, worldview, and cultural references.
    pub origin_civ_id: Option<Uuid>,
    /// Background tags — "frontier_born", "career_military", "ex_academic".
    /// Used for encounter matching and dialogue flavor.
    #[serde(default)]
    pub background_tags: Vec<String>,

    // --- Personality (generated once, shapes all interactions) ---

    /// Three-axis personality model. Simpler than crew PersonalityDrives
    /// because the player doesn't live with NPCs.
    pub personality: NpcPersonality,

    // --- Social state (changes over time) ---

    /// How this NPC feels about the player (-1.0 to 1.0).
    /// Starts at 0.0 (neutral). Shifts through interaction.
    pub disposition: f32,
    /// What drives this NPC — 2-3 short phrases.
    pub motivations: Vec<String>,
    /// Relationships with other NPCs.
    #[serde(default)]
    pub connections: Vec<NpcConnection>,
    /// What this NPC knows — local intel, faction info, thread awareness.
    /// Each entry is a short description the NPC can share with the player.
    #[serde(default)]
    pub knowledge: Vec<String>,
    /// Accumulated interaction history with the player.
    /// Capped at 5 entries — oldest pruned.
    #[serde(default)]
    pub interaction_history: Vec<InteractionRecord>,
}

impl Npc {
    /// Create a new NPC with neutral disposition and default social state.
    pub fn new(
        name: impl Into<String>,
        title: impl Into<String>,
        species: Species,
        faction_id: Option<Uuid>,
        home_system_id: Uuid,
        bio: impl Into<String>,
    ) -> Self {
        let pronouns = species.default_pronouns();
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            title: title.into(),
            species,
            pronouns,
            faction_id,
            home_system_id,
            home_location_id: None,
            alive: true,
            bio: bio.into(),
            origin_civ_id: None,
            background_tags: Vec::new(),
            personality: NpcPersonality::default(),
            disposition: 0.0,
            motivations: Vec::new(),
            connections: Vec::new(),
            knowledge: Vec::new(),
            interaction_history: Vec::new(),
        }
    }

    /// Record an interaction with the player. Caps history at 5.
    pub fn record_interaction(&mut self, summary: impl Into<String>, galactic_day: f64, disposition_delta: f32) {
        self.disposition = (self.disposition + disposition_delta).clamp(-1.0, 1.0);
        self.interaction_history.push(InteractionRecord {
            galactic_day,
            summary: summary.into(),
            disposition_delta,
        });
        if self.interaction_history.len() > 5 {
            self.interaction_history.remove(0);
        }
    }

    /// The most recent interaction with the player, if any.
    pub fn last_interaction(&self) -> Option<&InteractionRecord> {
        self.interaction_history.last()
    }
}

// ---------------------------------------------------------------------------
// Species and sex
// ---------------------------------------------------------------------------

/// What kind of being this NPC is. Drives portrait selection,
/// pronoun derivation, and species-specific interaction flavor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Species {
    /// Standard human — the vast majority in the Near Reach.
    Human { sex: BiologicalSex },
    /// Synthetic intelligence in a humanoid or non-humanoid shell.
    /// Faction-built, station-embedded, or autonomous.
    Synthetic {
        /// Description of physical form — "humanoid frame",
        /// "station-embedded terminal", "maintenance unit".
        chassis: String,
    },
    /// Alien species encountered at the edges of known space.
    /// The `kind` string is the species name.
    Alien { kind: String, sex: AlienSex },
}

impl Species {
    /// Derive the default pronoun set from species and sex.
    pub fn default_pronouns(&self) -> Pronouns {
        match self {
            Species::Human { sex } => match sex {
                BiologicalSex::Male => Pronouns::he(),
                BiologicalSex::Female => Pronouns::she(),
            },
            Species::Synthetic { .. } => Pronouns::it(),
            Species::Alien { sex, .. } => match sex {
                AlienSex::Male => Pronouns::he(),
                AlienSex::Female => Pronouns::she(),
                AlienSex::Neuter | AlienSex::Other => Pronouns::they(),
            },
        }
    }

    /// Short label for display — "human", "synthetic", "alien (kind)".
    pub fn display_label(&self) -> String {
        match self {
            Species::Human { .. } => "human".into(),
            Species::Synthetic { chassis } => format!("synthetic ({})", chassis),
            Species::Alien { kind, .. } => kind.clone(),
        }
    }

    /// Whether this is a human NPC.
    pub fn is_human(&self) -> bool {
        matches!(self, Species::Human { .. })
    }

    /// Whether this is a synthetic NPC.
    pub fn is_synthetic(&self) -> bool {
        matches!(self, Species::Synthetic { .. })
    }
}

/// Biological sex for human NPCs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BiologicalSex {
    Male,
    Female,
}

/// Sex categories for alien species — biology may not map to human norms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlienSex {
    Male,
    Female,
    Neuter,
    /// Biology doesn't map to any familiar category.
    Other,
}

// ---------------------------------------------------------------------------
// Pronouns
// ---------------------------------------------------------------------------

/// Pronoun set for text generation. Stored flat on each NPC so
/// templates can use {pronoun.subject}/{pronoun.object}/{pronoun.possessive}
/// without branching logic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pronouns {
    /// "he", "she", "they", "it"
    pub subject: String,
    /// "him", "her", "them", "it"
    pub object: String,
    /// "his", "her", "their", "its"
    pub possessive: String,
    /// "He", "She", "They", "It" — capitalized for sentence starts.
    pub subject_cap: String,
}

impl Pronouns {
    pub fn he() -> Self {
        Self {
            subject: "he".into(),
            object: "him".into(),
            possessive: "his".into(),
            subject_cap: "He".into(),
        }
    }

    pub fn she() -> Self {
        Self {
            subject: "she".into(),
            object: "her".into(),
            possessive: "her".into(),
            subject_cap: "She".into(),
        }
    }

    pub fn they() -> Self {
        Self {
            subject: "they".into(),
            object: "them".into(),
            possessive: "their".into(),
            subject_cap: "They".into(),
        }
    }

    pub fn it() -> Self {
        Self {
            subject: "it".into(),
            object: "it".into(),
            possessive: "its".into(),
            subject_cap: "It".into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Personality
// ---------------------------------------------------------------------------

/// Three-axis personality model for NPCs.
///
/// Simpler than crew's six-axis PersonalityDrives because the player
/// doesn't live with NPCs. Three axes create 8 archetype corners
/// that are immediately legible through behavior.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct NpcPersonality {
    /// How readily they engage with others.
    /// 0.0 = guarded, transactional. 1.0 = open, generous with trust.
    pub warmth: f32,
    /// How they handle risk and conflict.
    /// 0.0 = cautious, hedging. 1.0 = direct, confrontational.
    pub boldness: f32,
    /// What motivates them.
    /// 0.0 = pragmatic, self-interested. 1.0 = principled, mission-driven.
    pub idealism: f32,
}

impl Default for NpcPersonality {
    fn default() -> Self {
        Self {
            warmth: 0.5,
            boldness: 0.5,
            idealism: 0.5,
        }
    }
}

impl NpcPersonality {
    /// Describe the dominant trait for bio generation.
    /// Returns a short phrase like "guarded and pragmatic".
    pub fn dominant_description(&self) -> &'static str {
        // Find the axis furthest from 0.5 in either direction.
        let axes = [
            ("warmth", self.warmth),
            ("boldness", self.boldness),
            ("idealism", self.idealism),
        ];

        let warmth_high = self.warmth > 0.6;
        let boldness_high = self.boldness > 0.6;
        let idealism_high = self.idealism > 0.6;

        match (warmth_high, boldness_high, idealism_high) {
            (true, true, true) => "passionate and open",
            (true, true, false) => "decisive and approachable",
            (true, false, true) => "principled and kind",
            (true, false, false) => "friendly but careful",
            (false, true, true) => "intense and driven",
            (false, true, false) => "direct and calculating",
            (false, false, true) => "reserved but principled",
            (false, false, false) => "guarded and pragmatic",
        }
    }
}

// ---------------------------------------------------------------------------
// Social connections
// ---------------------------------------------------------------------------

/// A relationship between two NPCs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NpcConnection {
    /// The other NPC in this relationship.
    pub npc_id: Uuid,
    /// What kind of relationship.
    pub relationship: NpcRelationType,
    /// Free text context — "mentored them when they first arrived",
    /// "competes for the same trade routes", etc.
    pub context: String,
}

/// Types of NPC-to-NPC relationships.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NpcRelationType {
    /// Same faction, professional respect.
    Colleague,
    /// Friendly despite different factions.
    Acquaintance,
    /// Active friction or competition.
    Rival,
    /// One depends on the other (for supplies, information, etc.).
    Dependent,
    /// Old friends from before current postings.
    OldFriend,
    /// Knows of them by reputation only.
    KnowsOf,
}

impl std::fmt::Display for NpcRelationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NpcRelationType::Colleague => write!(f, "colleague"),
            NpcRelationType::Acquaintance => write!(f, "acquaintance"),
            NpcRelationType::Rival => write!(f, "rival"),
            NpcRelationType::Dependent => write!(f, "dependent"),
            NpcRelationType::OldFriend => write!(f, "old friend"),
            NpcRelationType::KnowsOf => write!(f, "knows of"),
        }
    }
}

// ---------------------------------------------------------------------------
// Interaction history
// ---------------------------------------------------------------------------

/// A record of one interaction between the NPC and the player.
/// Minimal — just enough for "you helped me before" or
/// "last time you stiffed me on payment."
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractionRecord {
    /// When (galactic days).
    pub galactic_day: f64,
    /// What happened — short phrase.
    pub summary: String,
    /// How it shifted disposition.
    pub disposition_delta: f32,
}