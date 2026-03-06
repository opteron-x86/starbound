// file: crates/core/src/reputation.rs
//! Player behavioral profile types — the world's model of who you are.
//!
//! The player does not choose a class. Identity emerges from accumulated
//! actions. The game tracks behavioral axes derived from what the player
//! actually does, and the world assigns labels when patterns crystallize.
//!
//! These types are in core because they're serialized on Journey and
//! read by the encounter pipeline, skill checks, and eventually NPCs.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

use crate::time::Timestamp;

/// The player's emergent identity — derived from actions, not chosen.
///
/// Six behavioral axes, each 0.0–1.0, computed from the action history.
/// The player never sees the numbers; they manifest through NPC reactions,
/// faction offers, crew opinions, and available encounter options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerProfile {
    // -- Behavioral axes (derived, not set directly) --
    /// Frequency and severity of violent actions.
    /// 0.0 = pacifist, 1.0 = predator.
    pub aggression: f32,
    /// Contract completion rate, promise-keeping, consistency.
    /// 0.0 = flake, 1.0 = ironclad.
    pub reliability: f32,
    /// Per-faction commitment. Do they stick with allies or play all sides?
    /// This is the aggregate score; per-faction loyalty is in the map.
    /// 0.0 = mercenary, 1.0 = true believer.
    pub loyalty: f32,
    /// Response to vulnerability. Help, exploit, or ignore?
    /// 0.0 = ruthless, 1.0 = compassionate.
    pub mercy: f32,
    /// Engagement with the unknown. Anomalies, clues, distorted space.
    /// 0.0 = pragmatist, 1.0 = seeker.
    pub curiosity: f32,
    /// Information handling. Keep secrets or broadcast discoveries?
    /// 0.0 = loudmouth, 1.0 = vault.
    pub discretion: f32,

    /// Per-faction loyalty scores. Keys are faction IDs.
    pub faction_loyalty: HashMap<Uuid, f32>,

    /// The raw action history — what the player actually did.
    /// Used to derive the axes above. Capped to prevent unbounded growth.
    pub action_history: Vec<ActionRecord>,

    /// Currently active reputation labels — assigned by the world.
    pub labels: Vec<ReputationLabel>,
}

impl PlayerProfile {
    /// A blank profile — no history, no labels, all axes at 0.5 (neutral).
    pub fn new() -> Self {
        Self {
            aggression: 0.0,
            reliability: 0.5,
            loyalty: 0.5,
            mercy: 0.5,
            curiosity: 0.0,
            discretion: 0.5,
            faction_loyalty: HashMap::new(),
            action_history: Vec::new(),
            labels: Vec::new(),
        }
    }

    /// Whether the player has a specific label active.
    pub fn has_label(&self, label_kind: &LabelKind) -> bool {
        self.labels.iter().any(|l| l.kind == *label_kind)
    }

    /// Get the strongest active label, if any.
    pub fn primary_label(&self) -> Option<&ReputationLabel> {
        self.labels.iter().max_by(|a, b| {
            a.strength.partial_cmp(&b.strength).unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Score how well the player's reputation fits an encounter based on tags.
    ///
    /// Called by the encounter pipeline to boost events that match the
    /// player's known identity. Returns a weight multiplier (1.0 = neutral).
    pub fn encounter_weight(&self, event_tags: &[String]) -> f64 {
        if self.labels.is_empty() || event_tags.is_empty() {
            return 1.0;
        }

        let mut weight = 1.0_f64;

        for label in &self.labels {
            let strength = label.strength as f64;

            for tag in event_tags {
                let tag_lower = tag.to_lowercase();
                let tag_match = match label.kind {
                    LabelKind::Pirate => matches!(
                        tag_lower.as_str(),
                        "pirate" | "raid" | "criminal" | "smuggling" | "combat"
                    ),
                    LabelKind::Privateer => matches!(
                        tag_lower.as_str(),
                        "military" | "combat" | "patrol" | "enforcement"
                    ),
                    LabelKind::Trader => matches!(
                        tag_lower.as_str(),
                        "trade" | "merchant" | "economic" | "negotiation"
                    ),
                    LabelKind::Seeker => matches!(
                        tag_lower.as_str(),
                        "anomaly" | "mystery" | "ancient" | "signal"
                            | "distortion" | "exploration"
                    ),
                    LabelKind::Mercenary => matches!(
                        tag_lower.as_str(),
                        "combat" | "contract" | "military" | "mercenary"
                    ),
                    LabelKind::Operative => matches!(
                        tag_lower.as_str(),
                        "intelligence" | "covert" | "espionage" | "stealth"
                    ),
                    LabelKind::Drifter => false,
                };

                if tag_match {
                    weight += 0.3 * strength;
                }
            }
        }

        weight
    }

    /// Shift a behavioral axis by label name. Used by encounter effects
    /// to adjust the player's profile based on choices.
    ///
    /// Label names map to axes:
    /// - "explorer" / "seeker" → curiosity
    /// - "trader" / "merchant" → reliability (via economic activity)
    /// - "diplomat" → mercy + discretion
    /// - "fighter" / "mercenary" → aggression
    /// - "pirate" → aggression + (negative mercy)
    /// - "scholar" → curiosity + discretion
    pub fn shift_label(&mut self, label: &str, delta: f32) {
        match label.to_lowercase().as_str() {
            "explorer" | "seeker" => {
                self.curiosity = (self.curiosity + delta).clamp(0.0, 1.0);
            }
            "trader" | "merchant" => {
                self.reliability = (self.reliability + delta * 0.5).clamp(0.0, 1.0);
            }
            "diplomat" => {
                self.mercy = (self.mercy + delta * 0.5).clamp(0.0, 1.0);
                self.discretion = (self.discretion + delta * 0.5).clamp(0.0, 1.0);
            }
            "fighter" | "mercenary" => {
                self.aggression = (self.aggression + delta).clamp(0.0, 1.0);
            }
            "pirate" => {
                self.aggression = (self.aggression + delta).clamp(0.0, 1.0);
                self.mercy = (self.mercy - delta * 0.5).clamp(0.0, 1.0);
            }
            "scholar" => {
                self.curiosity = (self.curiosity + delta * 0.7).clamp(0.0, 1.0);
                self.discretion = (self.discretion + delta * 0.3).clamp(0.0, 1.0);
            }
            _ => {} // Unknown labels are silently ignored.
        }
    }
}

impl Default for PlayerProfile {
    fn default() -> Self {
        Self::new()
    }
}

/// A record of a player action — the raw data that drives axis derivation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRecord {
    /// What category of action was taken.
    pub action_type: ActionType,
    /// When it happened (both timescales).
    pub timestamp: Timestamp,
    /// Where and in what faction context.
    pub context: ActionContext,
}

/// Categories of trackable player action.
/// Each maps to one or more behavioral axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    // -- Aggression axis --
    /// Attacked another vessel or entity.
    Attack,
    /// Raided or pirated a target.
    Raid,
    /// Threatened or intimidated.
    Threaten,

    // -- Mercy axis --
    /// Helped someone in distress.
    Rescue,
    /// Shared resources with those in need.
    ShareResources,
    /// Exploited someone vulnerable.
    Exploit,
    /// Ignored a distress call or plea.
    Ignore,

    // -- Reliability axis --
    /// Completed a contract or promise.
    ContractComplete,
    /// Abandoned a contract.
    ContractAbandon,
    /// Betrayed a contract (delivered to wrong party, etc.).
    ContractBetray,

    // -- Curiosity axis --
    /// Investigated an anomaly or unknown signal.
    Investigate,
    /// Entered distorted or dangerous space voluntarily.
    EnterDistortion,
    /// Followed a mission clue.
    PursueMission,
    /// Avoided the unknown — chose safety over discovery.
    AvoidUnknown,

    // -- Discretion axis --
    /// Kept a secret or withheld information.
    KeepSecret,
    /// Sold or shared intelligence.
    SellIntel,
    /// Broadcast a discovery publicly.
    Broadcast,

    // -- Loyalty axis (always paired with faction context) --
    /// Acted in a faction's interest.
    FactionService,
    /// Acted against a faction's interest.
    FactionBetrayal,

    // -- Economic --
    /// Completed a trade.
    Trade,
    /// Smuggled contraband.
    Smuggle,
}

/// Where and in what context an action was taken.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionContext {
    /// The system where the action occurred.
    pub system_id: Option<Uuid>,
    /// The faction most relevant to this action (if any).
    pub faction_id: Option<Uuid>,
    /// Free-text note for narrative context.
    pub note: String,
}

impl ActionContext {
    pub fn empty() -> Self {
        Self {
            system_id: None,
            faction_id: None,
            note: String::new(),
        }
    }

    pub fn with_note(note: &str) -> Self {
        Self {
            system_id: None,
            faction_id: None,
            note: note.into(),
        }
    }

    pub fn faction(faction_id: Uuid) -> Self {
        Self {
            system_id: None,
            faction_id: Some(faction_id),
            note: String::new(),
        }
    }
}

/// A reputation label assigned by the world when behavioral patterns
/// cross recognition thresholds.
///
/// Labels are not permanent — they shift as behavior changes, but
/// history has weight. Early decisions matter more because they set
/// expectations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationLabel {
    /// What kind of label.
    pub kind: LabelKind,
    /// How strongly the world associates this label with the player.
    /// 0.0 = barely recognized, 1.0 = defining trait.
    pub strength: f32,
    /// Which factions recognize this label.
    /// Empty = universally recognized.
    pub recognized_by: Vec<Uuid>,
}

/// The categories of reputation the world can assign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Display, EnumString)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum LabelKind {
    /// High aggression + low mercy + economic activity.
    /// Military factions hunt you. Criminal factions respect you.
    Pirate,
    /// High reliability + military faction loyalty.
    /// Formal/informal military contracts. Rivals treat you as combatant.
    Privateer,
    /// High reliability + economic activity + low aggression.
    /// Best prices, preferred docking, but you're a target.
    Trader,
    /// High curiosity + engagement with distorted space.
    /// The religious order takes interest. Strange encounters find you.
    Seeker,
    /// High aggression + contract work + moderate reliability.
    /// Military factions offer wet work. Dangerous jobs find you.
    Mercenary,
    /// High discretion + covert contract work.
    /// Intelligence networks value you. You hear things others don't.
    Operative,
    /// Balanced profile — no strong signals.
    /// No special treatment, no closed doors.
    Drifter,
}

impl LabelKind {
    /// Human-readable description of the label.
    pub fn description(self) -> &'static str {
        match self {
            LabelKind::Pirate => "Known raider and predator",
            LabelKind::Privateer => "Military-aligned enforcer",
            LabelKind::Trader => "Reliable merchant captain",
            LabelKind::Seeker => "Drawn to the unknown",
            LabelKind::Mercenary => "Gun for hire",
            LabelKind::Operative => "Shadow operator",
            LabelKind::Drifter => "No fixed allegiance",
        }
    }
}