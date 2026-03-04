//! Starbound Core — the shared vocabulary of the game.
//!
//! All data types live here. No game logic. These types are the design
//! document made concrete: galaxy structures, crew personalities, the
//! thread ledger, mission state, and the dual-timeline system that
//! makes everything tick.

pub mod crew;
pub mod galaxy;
pub mod journey;
pub mod mission;
pub mod narrative;
pub mod reputation;
pub mod ship;
pub mod time;

// Re-export the most commonly used types at crate root.
pub use crew::CrewMember;
pub use galaxy::{Civilization, Connection, Faction, Sector, StarSystem};
pub use journey::Journey;
pub use mission::{KnowledgeNode, MissionState};
pub use narrative::{EncounterBrief, GameEvent, Thread};
pub use reputation::PlayerProfile;
pub use ship::Ship;
pub use time::Timestamp;