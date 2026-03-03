use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

/// The mission gives the journey shape. It's gravity — you can escape it,
/// orbit it, fight it, but it's always there bending your trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissionState {
    pub mission_type: MissionType,
    /// The truth baked into the galaxy. Structured enough for the simulation,
    /// evocative enough for the LLM.
    pub core_truth: String,
    /// Knowledge nodes — pieces of understanding the player accumulates.
    pub knowledge_nodes: Vec<KnowledgeNode>,
}

impl MissionState {
    /// How many nodes the player has discovered (in any state beyond Unknown).
    pub fn discovered_count(&self) -> usize {
        self.knowledge_nodes
            .iter()
            .filter(|n| n.discovery_state != DiscoveryState::Unknown)
            .count()
    }

    /// Rough sense of mission progress as a fraction.
    pub fn progress(&self) -> f32 {
        if self.knowledge_nodes.is_empty() {
            return 0.0;
        }
        let connected = self
            .knowledge_nodes
            .iter()
            .filter(|n| n.discovery_state == DiscoveryState::Connected)
            .count();
        connected as f32 / self.knowledge_nodes.len() as f32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MissionType {
    /// Find a thing. The thing is more than expected.
    Search,
    /// Reach someone or something. The truth is relational.
    Contact,
    /// Time-sensitive. Every detour costs lives. Time dilation makes this painful.
    Rescue,
    /// Most open-ended. Explore and report back (if "back" still means anything).
    Survey,
}

/// A piece of understanding about the mission's truth.
/// Multiple access points mean missing one doesn't block progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeNode {
    pub id: Uuid,
    pub node_type: KnowledgeNodeType,
    pub description: String,
    pub discovery_state: DiscoveryState,
    /// Other nodes that must be understood before this one makes sense.
    pub dependencies: Vec<Uuid>,
    /// Different ways to encounter this knowledge — an ancient database,
    /// faction oral history, crew member expertise, a trader's anecdote.
    pub access_points: Vec<String>,
    /// How central this node is to the core truth.
    pub relevance: Relevance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeNodeType {
    /// A physical location, piece of technology, historical record.
    Concrete,
    /// A principle, pattern, linguistic key.
    Conceptual,
    /// Understanding that two things are connected.
    Relational,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryState {
    Unknown,
    Discovered,
    Understood,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Relevance {
    /// Directly addresses the core truth.
    Central,
    /// Important supporting evidence.
    Supporting,
    /// Tangential — interesting but not essential.
    Peripheral,
    /// Looks relevant but misleads (red herring with its own narrative value).
    Misleading,
}
