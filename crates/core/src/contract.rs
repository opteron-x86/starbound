// file: crates/core/src/contract.rs
//! Contract data types — structured jobs the player can accept.
//!
//! Contracts are offered by NPCs, tracked on the Journey, and
//! resolved through gameplay. They have objectives, rewards,
//! and consequences for completion or failure.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

/// A contract — a job offered by an NPC with clear terms.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contract {
    pub id: Uuid,
    /// The NPC who offered this contract.
    pub issuer_npc_id: Uuid,
    /// The faction behind the contract (if any).
    pub issuer_faction_id: Option<Uuid>,
    /// Short name — "Deliver medical supplies to Acheron".
    pub title: String,
    /// A couple sentences of context.
    pub description: String,
    /// What kind of job this is.
    pub contract_type: ContractType,
    /// Where the work takes you.
    pub destination_system_id: Uuid,
    /// Specific location at the destination (None = any location in system).
    #[serde(default)]
    pub destination_location_id: Option<Uuid>,
    /// Where to return for payment (usually the issuer's home system).
    pub origin_system_id: Uuid,
    /// Credits paid on completion.
    pub reward_credits: f64,
    /// Cargo placed in the player's hold on acceptance.
    /// (item name, quantity)
    pub cargo_given: Option<(String, u32)>,
    /// Cargo the player must deliver (for delivery contracts).
    /// May be the same as cargo_given, or something else for retrieval.
    pub cargo_required: Option<(String, u32)>,
    /// Current state of this contract.
    pub state: ContractState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ContractType {
    /// Carry cargo from origin to destination.
    Delivery,
    /// Go to destination, find something, bring it back.
    Retrieval,
    /// Go to destination, learn something, report back.
    Investigation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ContractState {
    /// NPC is offering this, player hasn't accepted yet.
    Offered,
    /// Player has accepted. Working on it.
    Active,
    /// Objective complete, ready to turn in.
    ReadyToComplete,
    /// Turned in. Rewards received.
    Completed,
    /// Player failed or abandoned the contract.
    Failed,
}

impl Contract {
    /// Create a simple delivery contract.
    pub fn delivery(
        issuer_npc_id: Uuid,
        issuer_faction_id: Option<Uuid>,
        title: impl Into<String>,
        description: impl Into<String>,
        origin_system_id: Uuid,
        destination_system_id: Uuid,
        cargo_name: impl Into<String>,
        cargo_quantity: u32,
        reward_credits: f64,
    ) -> Self {
        let cargo = cargo_name.into();
        Self {
            id: Uuid::new_v4(),
            issuer_npc_id,
            issuer_faction_id,
            title: title.into(),
            description: description.into(),
            contract_type: ContractType::Delivery,
            destination_system_id,
            destination_location_id: None,
            origin_system_id,
            reward_credits,
            cargo_given: Some((cargo.clone(), cargo_quantity)),
            cargo_required: Some((cargo, cargo_quantity)),
            state: ContractState::Offered,
        }
    }
}