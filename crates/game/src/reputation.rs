// file: crates/game/src/reputation.rs
//! Reputation engine — derives who the player is from what they've done.
//!
//! The system works in three phases:
//! 1. **Record**: Player actions are logged as ActionRecords.
//! 2. **Derive**: Behavioral axes are recalculated from the history.
//! 3. **Label**: The world assigns reputation labels when patterns
//!    cross recognition thresholds.
//!
//! Design principle: early actions carry extra weight. The first
//! few choices define initial expectations; later actions can shift
//! the profile but the past resists. This means first impressions
//! matter — just like in real life.

use starbound_core::journey::Journey;
use starbound_core::reputation::{
    ActionContext, ActionRecord, ActionType, LabelKind, PlayerProfile, ReputationLabel,
};
use starbound_core::time::Timestamp;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Maximum number of actions retained in history.
/// Oldest actions are pruned when this limit is exceeded.
const MAX_HISTORY: usize = 200;

/// Minimum number of actions before labels start crystallizing.
/// Prevents snap judgments from a single choice.
const LABEL_MIN_ACTIONS: usize = 5;

/// Strength threshold for a label to become active.
const LABEL_ACTIVATION_THRESHOLD: f32 = 0.4;

/// Labels below this strength are pruned.
const LABEL_PRUNING_THRESHOLD: f32 = 0.15;

// ---------------------------------------------------------------------------
// Action recording
// ---------------------------------------------------------------------------

/// Record a player action and recalculate the profile.
///
/// This is the main entry point — call it whenever the player does
/// something that should shape their identity.
pub fn record_action(
    journey: &mut Journey,
    action_type: ActionType,
    context: ActionContext,
) {
    let record = ActionRecord {
        action_type,
        timestamp: journey.time,
        context,
    };

    journey.profile.action_history.push(record);

    // Prune if over limit.
    if journey.profile.action_history.len() > MAX_HISTORY {
        let excess = journey.profile.action_history.len() - MAX_HISTORY;
        journey.profile.action_history.drain(0..excess);
    }

    // Recalculate everything.
    recalculate_axes(&mut journey.profile);
    evaluate_labels(&mut journey.profile);
}

/// Record an action with just a type and note (convenience).
pub fn record_simple(journey: &mut Journey, action_type: ActionType, note: &str) {
    record_action(
        journey,
        action_type,
        ActionContext::with_note(note),
    );
}

/// Record an action with faction context.
pub fn record_faction_action(
    journey: &mut Journey,
    action_type: ActionType,
    faction_id: Uuid,
    note: &str,
) {
    let mut ctx = ActionContext::faction(faction_id);
    ctx.note = note.into();
    record_action(journey, action_type, ctx);
}

// ---------------------------------------------------------------------------
// Axis derivation
// ---------------------------------------------------------------------------

/// Recalculate all six behavioral axes from the action history.
///
/// Uses weighted counting with recency bias — recent actions count
/// more than old ones, but old actions never fully disappear.
fn recalculate_axes(profile: &mut PlayerProfile) {
    let history = &profile.action_history;

    if history.is_empty() {
        return;
    }

    let len = history.len() as f32;

    // --- Aggression ---
    // Driven by: Attack, Raid, Threaten (increase)
    // Reduced by absence of violent actions over time.
    let aggressive_actions = weighted_count(history, |a| {
        matches!(
            a.action_type,
            ActionType::Attack | ActionType::Raid | ActionType::Threaten
        )
    });
    // Normalize: if half your actions are aggressive, aggression = 1.0.
    profile.aggression = (aggressive_actions / (len * 0.5)).min(1.0);

    // --- Mercy ---
    // Driven by: Rescue, ShareResources (increase)
    // Reduced by: Exploit, Ignore (decrease)
    let merciful = weighted_count(history, |a| {
        matches!(
            a.action_type,
            ActionType::Rescue | ActionType::ShareResources
        )
    });
    let cruel = weighted_count(history, |a| {
        matches!(a.action_type, ActionType::Exploit | ActionType::Ignore)
    });
    let mercy_total = merciful + cruel;
    if mercy_total > 0.0 {
        profile.mercy = merciful / mercy_total;
    }
    // If no mercy-relevant actions, stays at default 0.5.

    // --- Reliability ---
    // Driven by contract outcomes: complete vs abandon/betray.
    let completed = weighted_count(history, |a| {
        a.action_type == ActionType::ContractComplete
    });
    let broken = weighted_count(history, |a| {
        matches!(
            a.action_type,
            ActionType::ContractAbandon | ActionType::ContractBetray
        )
    });
    let contract_total = completed + broken;
    if contract_total > 0.0 {
        profile.reliability = completed / contract_total;
    }

    // --- Curiosity ---
    // Driven by: Investigate, EnterDistortion, PursueMission (increase)
    // Reduced by: AvoidUnknown
    let curious = weighted_count(history, |a| {
        matches!(
            a.action_type,
            ActionType::Investigate | ActionType::EnterDistortion | ActionType::PursueMission
        )
    });
    let cautious = weighted_count(history, |a| {
        a.action_type == ActionType::AvoidUnknown
    });
    let curiosity_total = curious + cautious;
    if curiosity_total > 0.0 {
        profile.curiosity = curious / curiosity_total;
    } else if curious > 0.0 {
        profile.curiosity = (curious / (len * 0.3)).min(1.0);
    }

    // --- Discretion ---
    // Driven by: KeepSecret (increase)
    // Reduced by: SellIntel, Broadcast (decrease)
    let secret = weighted_count(history, |a| a.action_type == ActionType::KeepSecret);
    let loud = weighted_count(history, |a| {
        matches!(
            a.action_type,
            ActionType::SellIntel | ActionType::Broadcast
        )
    });
    let discretion_total = secret + loud;
    if discretion_total > 0.0 {
        profile.discretion = secret / discretion_total;
    }

    // --- Loyalty (aggregate) ---
    // Driven by: FactionService vs FactionBetrayal across all factions.
    let loyal = weighted_count(history, |a| a.action_type == ActionType::FactionService);
    let disloyal = weighted_count(history, |a| {
        a.action_type == ActionType::FactionBetrayal
    });
    let loyalty_total = loyal + disloyal;
    if loyalty_total > 0.0 {
        profile.loyalty = loyal / loyalty_total;
    }

    // --- Per-faction loyalty ---
    recalculate_faction_loyalty(profile);
}

/// Recalculate per-faction loyalty scores.
fn recalculate_faction_loyalty(profile: &mut PlayerProfile) {
    // Collect all faction-relevant actions.
    let mut faction_service: std::collections::HashMap<Uuid, f32> =
        std::collections::HashMap::new();
    let mut faction_betrayal: std::collections::HashMap<Uuid, f32> =
        std::collections::HashMap::new();

    for (i, record) in profile.action_history.iter().enumerate() {
        if let Some(fid) = record.context.faction_id {
            let weight = recency_weight(i, profile.action_history.len());
            match record.action_type {
                ActionType::FactionService => {
                    *faction_service.entry(fid).or_default() += weight;
                }
                ActionType::FactionBetrayal => {
                    *faction_betrayal.entry(fid).or_default() += weight;
                }
                _ => {}
            }
        }
    }

    // Merge into loyalty scores.
    profile.faction_loyalty.clear();
    let all_factions: std::collections::HashSet<Uuid> = faction_service
        .keys()
        .chain(faction_betrayal.keys())
        .copied()
        .collect();

    for fid in all_factions {
        let service = faction_service.get(&fid).copied().unwrap_or(0.0);
        let betrayal = faction_betrayal.get(&fid).copied().unwrap_or(0.0);
        let total = service + betrayal;
        if total > 0.0 {
            profile.faction_loyalty.insert(fid, service / total);
        }
    }
}

/// Weighted count of actions matching a predicate.
/// Recent actions count more (recency bias).
fn weighted_count<F>(history: &[ActionRecord], predicate: F) -> f32
where
    F: Fn(&ActionRecord) -> bool,
{
    let len = history.len();
    history
        .iter()
        .enumerate()
        .filter(|(_, a)| predicate(a))
        .map(|(i, _)| recency_weight(i, len))
        .sum()
}

/// Weight for an action based on its position in history.
/// Index 0 = oldest, len-1 = most recent.
/// Most recent action gets weight 1.0, oldest gets 0.3.
fn recency_weight(index: usize, total: usize) -> f32 {
    if total <= 1 {
        return 1.0;
    }
    let position = index as f32 / (total - 1) as f32; // 0.0 = oldest, 1.0 = newest
    0.3 + 0.7 * position
}

// ---------------------------------------------------------------------------
// Label evaluation
// ---------------------------------------------------------------------------

/// Evaluate whether the player's behavioral pattern matches any
/// reputation labels, and update the active labels accordingly.
fn evaluate_labels(profile: &mut PlayerProfile) {
    if profile.action_history.len() < LABEL_MIN_ACTIONS {
        return;
    }

    let candidates = vec![
        evaluate_pirate(profile),
        evaluate_privateer(profile),
        evaluate_trader(profile),
        evaluate_seeker(profile),
        evaluate_mercenary(profile),
        evaluate_operative(profile),
        evaluate_drifter(profile),
    ];

    // Update existing labels and add new ones.
    for (kind, strength) in candidates {
        if strength >= LABEL_ACTIVATION_THRESHOLD {
            if let Some(existing) = profile.labels.iter_mut().find(|l| l.kind == kind) {
                // Blend toward new strength (don't snap).
                existing.strength = existing.strength * 0.6 + strength * 0.4;
            } else {
                profile.labels.push(ReputationLabel {
                    kind,
                    strength,
                    recognized_by: vec![],
                });
            }
        } else if let Some(existing) = profile.labels.iter_mut().find(|l| l.kind == kind) {
            // Decay toward zero.
            existing.strength = existing.strength * 0.8 + strength * 0.2;
        }
    }

    // Prune labels that have decayed below threshold.
    profile
        .labels
        .retain(|l| l.strength >= LABEL_PRUNING_THRESHOLD);
}

fn evaluate_pirate(p: &PlayerProfile) -> (LabelKind, f32) {
    // High aggression + low mercy + some economic activity.
    let score = p.aggression * 0.5 + (1.0 - p.mercy) * 0.3 + (1.0 - p.reliability) * 0.2;
    (LabelKind::Pirate, score)
}

fn evaluate_privateer(p: &PlayerProfile) -> (LabelKind, f32) {
    // High reliability + some aggression + loyalty to at least one faction.
    let max_faction_loyalty = p
        .faction_loyalty
        .values()
        .copied()
        .fold(0.0_f32, f32::max);
    let score =
        p.reliability * 0.3 + p.aggression * 0.2 + max_faction_loyalty * 0.3 + p.loyalty * 0.2;
    (LabelKind::Privateer, score)
}

fn evaluate_trader(p: &PlayerProfile) -> (LabelKind, f32) {
    // High reliability + low aggression.
    let score = p.reliability * 0.4 + (1.0 - p.aggression) * 0.4 + p.mercy * 0.2;
    (LabelKind::Trader, score)
}

fn evaluate_seeker(p: &PlayerProfile) -> (LabelKind, f32) {
    // High curiosity is the primary driver.
    let score = p.curiosity * 0.6 + (1.0 - p.aggression) * 0.2 + p.discretion * 0.2;
    (LabelKind::Seeker, score)
}

fn evaluate_mercenary(p: &PlayerProfile) -> (LabelKind, f32) {
    // Moderate aggression + contract work + reliability.
    let score = p.aggression * 0.3 + p.reliability * 0.4 + (1.0 - p.loyalty) * 0.3;
    (LabelKind::Mercenary, score)
}

fn evaluate_operative(p: &PlayerProfile) -> (LabelKind, f32) {
    // High discretion + reliability.
    let score = p.discretion * 0.5 + p.reliability * 0.3 + (1.0 - p.aggression) * 0.2;
    (LabelKind::Operative, score)
}

fn evaluate_drifter(p: &PlayerProfile) -> (LabelKind, f32) {
    // No strong signals in any direction — the anti-pattern.
    // Score is higher when all axes are near 0.5.
    let centrality = 1.0
        - ((p.aggression - 0.5).abs()
            + (p.reliability - 0.5).abs()
            + (p.loyalty - 0.5).abs()
            + (p.mercy - 0.5).abs()
            + (p.curiosity - 0.5).abs()
            + (p.discretion - 0.5).abs())
            / 3.0; // Normalize to 0–1 range.
    (LabelKind::Drifter, centrality.max(0.0))
}

// ---------------------------------------------------------------------------
// Reputation modifier for skill checks
// ---------------------------------------------------------------------------

/// Compute the reputation modifier for a skill check.
///
/// This is the function that `checks.rs` calls to fill the
/// reputation slot in the modifier pool.
///
/// The modifier depends on which labels are active and what
/// kind of check is being made. Labels help in their domain
/// and can hinder outside it.
pub fn reputation_modifier(
    profile: &PlayerProfile,
    check_domain: ReputationDomain,
) -> f32 {
    let mut modifier = 0.0_f32;

    for label in &profile.labels {
        let contribution = domain_label_contribution(label.kind, check_domain);
        modifier += contribution * label.strength;
    }

    // Clamp to the design doc range.
    modifier.clamp(-0.10, 0.10)
}

/// What domain a skill check falls into, for reputation matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReputationDomain {
    /// Navigation, scanning, exploration.
    Exploration,
    /// Combat, intimidation, tactical.
    Combat,
    /// Trade, negotiation, diplomacy.
    Social,
    /// Engineering, repair, technical.
    Technical,
    /// Stealth, subterfuge, information.
    Covert,
    /// General — no specific domain.
    General,
}

/// How a reputation label affects checks in a given domain.
/// Positive = helps, negative = hinders.
fn domain_label_contribution(label: LabelKind, domain: ReputationDomain) -> f32 {
    match (label, domain) {
        // Pirates are feared — helps combat/intimidation, hinders social.
        (LabelKind::Pirate, ReputationDomain::Combat) => 0.08,
        (LabelKind::Pirate, ReputationDomain::Social) => -0.06,
        (LabelKind::Pirate, ReputationDomain::Covert) => -0.04,

        // Privateers are respected in combat, tolerated socially.
        (LabelKind::Privateer, ReputationDomain::Combat) => 0.06,
        (LabelKind::Privateer, ReputationDomain::Social) => 0.03,

        // Traders get the best social outcomes.
        (LabelKind::Trader, ReputationDomain::Social) => 0.08,
        (LabelKind::Trader, ReputationDomain::Combat) => -0.04,

        // Seekers excel at exploration.
        (LabelKind::Seeker, ReputationDomain::Exploration) => 0.08,
        (LabelKind::Seeker, ReputationDomain::Technical) => 0.04,

        // Mercenaries are combat-effective but socially suspect.
        (LabelKind::Mercenary, ReputationDomain::Combat) => 0.07,
        (LabelKind::Mercenary, ReputationDomain::Social) => -0.03,

        // Operatives excel at covert work.
        (LabelKind::Operative, ReputationDomain::Covert) => 0.08,
        (LabelKind::Operative, ReputationDomain::Social) => 0.03,

        // Drifters — no strong effect anywhere.
        (LabelKind::Drifter, _) => 0.0,

        // Default — no effect.
        _ => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Pipeline integration helpers
// ---------------------------------------------------------------------------

/// Score how well the player's reputation fits an encounter.
///
/// Convenience wrapper — delegates to PlayerProfile::encounter_weight().
pub fn reputation_encounter_weight(profile: &PlayerProfile, event_tags: &[String]) -> f64 {
    profile.encounter_weight(event_tags)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use starbound_core::reputation::*;
    use starbound_core::time::Timestamp;

    fn empty_profile() -> PlayerProfile {
        PlayerProfile::new()
    }

    fn profile_with_actions(actions: &[(ActionType, &str)]) -> PlayerProfile {
        let mut profile = empty_profile();
        for (i, (action_type, note)) in actions.iter().enumerate() {
            profile.action_history.push(ActionRecord {
                action_type: *action_type,
                timestamp: Timestamp {
                    personal_days: i as f64,
                    galactic_days: i as f64 * 10.0,
                },
                context: ActionContext::with_note(note),
            });
        }
        recalculate_axes(&mut profile);
        evaluate_labels(&mut profile);
        profile
    }

    // --- Axis derivation ---

    #[test]
    fn aggressive_actions_raise_aggression() {
        let profile = profile_with_actions(&[
            (ActionType::Attack, "raided a freighter"),
            (ActionType::Raid, "pirated a convoy"),
            (ActionType::Attack, "ambushed patrol"),
            (ActionType::Threaten, "intimidated dock workers"),
            (ActionType::Trade, "sold loot"),
            (ActionType::Trade, "traded at port"),
        ]);

        assert!(
            profile.aggression > 0.5,
            "Expected high aggression, got {:.2}",
            profile.aggression
        );
    }

    #[test]
    fn merciful_actions_raise_mercy() {
        let profile = profile_with_actions(&[
            (ActionType::Rescue, "saved stranded crew"),
            (ActionType::ShareResources, "gave food to colony"),
            (ActionType::Rescue, "responded to distress"),
            (ActionType::Rescue, "pulled survivors from wreck"),
            (ActionType::Trade, "resupplied"),
            (ActionType::Investigate, "checked anomaly"),
        ]);

        assert!(
            profile.mercy > 0.7,
            "Expected high mercy, got {:.2}",
            profile.mercy
        );
    }

    #[test]
    fn reliability_tracks_contracts() {
        let profile = profile_with_actions(&[
            (ActionType::ContractComplete, "delivered cargo"),
            (ActionType::ContractComplete, "finished escort"),
            (ActionType::ContractComplete, "patrol complete"),
            (ActionType::ContractAbandon, "abandoned salvage job"),
            (ActionType::Trade, "resupplied"),
            (ActionType::Trade, "refueled"),
        ]);

        assert!(
            profile.reliability > 0.6,
            "Expected moderate-high reliability, got {:.2}",
            profile.reliability
        );
    }

    #[test]
    fn curiosity_tracks_exploration() {
        let profile = profile_with_actions(&[
            (ActionType::Investigate, "scanned anomaly"),
            (ActionType::EnterDistortion, "entered weird zone"),
            (ActionType::PursueMission, "followed signal"),
            (ActionType::Investigate, "explored ruin"),
            (ActionType::Investigate, "analyzed artifact"),
            (ActionType::Trade, "resupplied"),
        ]);

        assert!(
            profile.curiosity > 0.6,
            "Expected high curiosity, got {:.2}",
            profile.curiosity
        );
    }

    // --- Label evaluation ---

    #[test]
    fn pirate_label_from_aggressive_unreliable_play() {
        let profile = profile_with_actions(&[
            (ActionType::Attack, "raided freighter"),
            (ActionType::Raid, "pirated convoy"),
            (ActionType::Exploit, "looted survivors"),
            (ActionType::Attack, "ambushed patrol"),
            (ActionType::ContractBetray, "double-crossed employer"),
            (ActionType::Threaten, "extorted docking fee"),
            (ActionType::Raid, "hit another target"),
        ]);

        assert!(
            profile.has_label(&LabelKind::Pirate),
            "Expected Pirate label. Labels: {:?}",
            profile.labels.iter().map(|l| (&l.kind, l.strength)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn seeker_label_from_curious_play() {
        let profile = profile_with_actions(&[
            (ActionType::Investigate, "scanned anomaly"),
            (ActionType::EnterDistortion, "entered weird zone"),
            (ActionType::PursueMission, "followed signal"),
            (ActionType::Investigate, "explored ruin"),
            (ActionType::Investigate, "analyzed artifact"),
            (ActionType::KeepSecret, "kept findings private"),
            (ActionType::PursueMission, "continued pursuit"),
        ]);

        assert!(
            profile.has_label(&LabelKind::Seeker),
            "Expected Seeker label. Labels: {:?}",
            profile.labels.iter().map(|l| (&l.kind, l.strength)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn trader_label_from_peaceful_reliable_play() {
        let profile = profile_with_actions(&[
            (ActionType::ContractComplete, "delivered cargo"),
            (ActionType::ContractComplete, "finished escort"),
            (ActionType::Trade, "traded at hub"),
            (ActionType::ContractComplete, "completed supply run"),
            (ActionType::Rescue, "helped stranded ship"),
            (ActionType::Trade, "sold goods"),
            (ActionType::ContractComplete, "another delivery"),
        ]);

        assert!(
            profile.has_label(&LabelKind::Trader),
            "Expected Trader label. Labels: {:?}",
            profile.labels.iter().map(|l| (&l.kind, l.strength)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn no_labels_with_too_few_actions() {
        let profile = profile_with_actions(&[
            (ActionType::Attack, "single attack"),
            (ActionType::Attack, "another attack"),
        ]);

        assert!(
            profile.labels.is_empty(),
            "Expected no labels with only 2 actions"
        );
    }

    // --- Recency bias ---

    #[test]
    fn recent_actions_weigh_more() {
        // Old actions: peaceful. Recent actions: aggressive.
        let mut actions: Vec<(ActionType, &str)> = Vec::new();
        for _ in 0..5 {
            actions.push((ActionType::Trade, "traded peacefully"));
        }
        for _ in 0..5 {
            actions.push((ActionType::Attack, "attacked aggressively"));
        }

        let recent_aggro = profile_with_actions(&actions);

        // Reverse: old actions aggressive, recent peaceful.
        let mut reversed: Vec<(ActionType, &str)> = Vec::new();
        for _ in 0..5 {
            reversed.push((ActionType::Attack, "attacked aggressively"));
        }
        for _ in 0..5 {
            reversed.push((ActionType::Trade, "traded peacefully"));
        }

        let recent_peaceful = profile_with_actions(&reversed);

        assert!(
            recent_aggro.aggression > recent_peaceful.aggression,
            "Recent aggression ({:.2}) should exceed recent peaceful ({:.2})",
            recent_aggro.aggression,
            recent_peaceful.aggression
        );
    }

    // --- Reputation modifier ---

    #[test]
    fn pirate_helps_combat_hinders_social() {
        let mut profile = empty_profile();
        profile.labels.push(ReputationLabel {
            kind: LabelKind::Pirate,
            strength: 0.8,
            recognized_by: vec![],
        });

        let combat_mod = reputation_modifier(&profile, ReputationDomain::Combat);
        let social_mod = reputation_modifier(&profile, ReputationDomain::Social);

        assert!(combat_mod > 0.0, "Pirate should help combat");
        assert!(social_mod < 0.0, "Pirate should hinder social");
    }

    #[test]
    fn seeker_helps_exploration() {
        let mut profile = empty_profile();
        profile.labels.push(ReputationLabel {
            kind: LabelKind::Seeker,
            strength: 0.7,
            recognized_by: vec![],
        });

        let exploration_mod = reputation_modifier(&profile, ReputationDomain::Exploration);
        assert!(
            exploration_mod > 0.0,
            "Seeker should help exploration"
        );
    }

    #[test]
    fn empty_profile_no_modifier() {
        let profile = empty_profile();
        let modifier = reputation_modifier(&profile, ReputationDomain::General);
        assert!(
            modifier.abs() < 0.001,
            "Empty profile should have no modifier"
        );
    }

    // --- Encounter weight ---

    #[test]
    fn pirate_label_boosts_pirate_encounters() {
        let mut profile = empty_profile();
        profile.labels.push(ReputationLabel {
            kind: LabelKind::Pirate,
            strength: 0.8,
            recognized_by: vec![],
        });

        let weight =
            reputation_encounter_weight(&profile, &["combat".into(), "pirate".into()]);
        assert!(
            weight > 1.0,
            "Pirate label should boost pirate encounters"
        );
    }

    #[test]
    fn no_labels_neutral_encounter_weight() {
        let profile = empty_profile();
        let weight = reputation_encounter_weight(&profile, &["trade".into()]);
        assert!(
            (weight - 1.0).abs() < 0.001,
            "No labels should give neutral weight"
        );
    }

    // --- Per-faction loyalty ---

    #[test]
    fn faction_loyalty_tracks_service() {
        let faction_a = Uuid::new_v4();

        let mut profile = empty_profile();
        for i in 0..6 {
            profile.action_history.push(ActionRecord {
                action_type: ActionType::FactionService,
                timestamp: Timestamp {
                    personal_days: i as f64,
                    galactic_days: i as f64 * 10.0,
                },
                context: ActionContext::faction(faction_a),
            });
        }

        recalculate_axes(&mut profile);

        let loyalty = profile.faction_loyalty.get(&faction_a).copied().unwrap_or(0.0);
        assert!(
            loyalty > 0.8,
            "Pure service should give high loyalty, got {:.2}",
            loyalty
        );
    }
}