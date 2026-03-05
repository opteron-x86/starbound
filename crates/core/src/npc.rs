// file: crates/core/src/npc.rs
//! NPC data types — named people who persist in the galaxy.
//!
//! NPCs live at systems, have faction ties, and remember the player.
//! They age in galactic time and can die between visits.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    /// How this NPC feels about the player (-1.0 to 1.0).
    /// Starts at 0.0 (neutral). Shifts through interaction.
    pub disposition: f32,
    /// A short description — 2-3 sentences of context.
    pub bio: String,
    /// What drives this NPC — short phrases like "expand guild influence".
    pub motivations: Vec<String>,
    /// Accumulated interaction history notes.
    pub notes: Vec<String>,
    /// Is this NPC still alive?
    pub alive: bool,
}

impl Npc {
    /// Create a new NPC with neutral disposition.
    pub fn new(
        name: impl Into<String>,
        title: impl Into<String>,
        faction_id: Option<Uuid>,
        home_system_id: Uuid,
        bio: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            title: title.into(),
            faction_id,
            home_system_id,
            home_location_id: None,
            disposition: 0.0,
            bio: bio.into(),
            motivations: Vec::new(),
            notes: Vec::new(),
            alive: true,
        }
    }
}