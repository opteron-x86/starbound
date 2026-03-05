use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::contract::Contract;
use crate::crew::CrewMember;
use crate::galaxy::CivStanding;
use crate::mission::MissionState;
use crate::narrative::{GameEvent, Thread};
use crate::reputation::PlayerProfile;
use crate::ship::Ship;
use crate::time::Timestamp;

/// The player's complete state — ship, crew, position, timeline, mission.
///
/// This is the in-memory object that gets persisted to SQLite.
/// Everything the game needs to know about the player's journey.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journey {
    pub ship: Ship,
    /// Which system the player is currently in.
    pub current_system: Uuid,
    /// Which location within the system the player is at.
    /// None means at the system edge (just arrived, or in sublight transit).
    #[serde(default)]
    pub current_location: Option<Uuid>,
    /// The dual timeline — personal and galactic time elapsed.
    pub time: Timestamp,
    /// Generic resource units (credits, trade goods, etc.).
    pub resources: f64,
    pub mission: MissionState,
    pub crew: Vec<CrewMember>,
    /// The narrative thread ledger — every dangling story element.
    pub threads: Vec<Thread>,
    /// The event log — what has happened, in order.
    pub event_log: Vec<GameEvent>,
    /// Player's standing with each civilization, keyed by civ ID.
    /// Initialized when the player first enters a civ's territory.
    pub civ_standings: HashMap<Uuid, CivStanding>,
    /// The player's emergent behavioral profile — derived from actions.
    #[serde(default)]
    pub profile: PlayerProfile,
    /// Active contracts the player has accepted.
    #[serde(default)]
    pub active_contracts: Vec<Contract>,
}