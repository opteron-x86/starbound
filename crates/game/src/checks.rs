// file: crates/game/src/checks.rs
//! Skill check resolution — the engine that makes ship state, crew
//! composition, and reputation matter for outcomes.
//!
//! When an action has uncertain results, the game runs a skill check.
//! The player never sees dice or percentages — they see narrative
//! framing that reflects the odds. Under the hood:
//!
//! 1. Compute a modifier pool from relevant game state.
//! 2. Add a small seeded random factor (±15%).
//! 3. Compare against difficulty.
//! 4. Resolve to one of five outcome tiers.
//!
//! Design principle: preparation and crew quality genuinely matter.
//! The random factor is small enough that a well-equipped ship with
//! a skilled crew in good shape will reliably succeed at moderate
//! challenges. But nothing is ever certain — and desperation makes
//! everything harder.

use rand::Rng;
use uuid::Uuid;

use starbound_core::crew::{CrewMember, CrewRole};
use starbound_core::journey::Journey;
use starbound_core::ship::ShipModules;

use crate::consequences::ModuleTarget;
use crate::reputation::{reputation_modifier, ReputationDomain};

// ---------------------------------------------------------------------------
// Check specification
// ---------------------------------------------------------------------------

/// Describes what's being checked — the inputs to the resolution engine.
///
/// Callers (encounter pipeline, player actions) build a SkillCheck
/// describing the situation, then pass it to `resolve_check()` with
/// the current journey state.
#[derive(Debug, Clone)]
pub struct SkillCheck {
    /// Which ship module is most relevant. Its condition contributes
    /// 0.0–0.4 to the modifier pool.
    pub relevant_module: Option<ModuleTarget>,
    /// Which crew role matters. The best-matching crew member's
    /// effective skill contributes 0.0–0.3.
    pub relevant_role: Option<CrewRole>,
    /// Base difficulty of the check: 0.0 (trivial) to 1.0 (impossible).
    pub difficulty: f32,
    /// Optional faction ID — if this check involves a faction interaction,
    /// standing with that faction contributes 0.0–0.15.
    pub faction_context: Option<Uuid>,
    /// Situational modifiers applied on top of computed pool.
    /// Positive values help, negative values hinder.
    pub situational_modifiers: Vec<SituationalModifier>,
    /// Which reputation domain this check falls into.
    /// Determines how player labels affect the outcome.
    pub reputation_domain: ReputationDomain,
}

/// An ad-hoc modifier from the encounter or situation context.
/// Examples: "element of surprise" (+0.1), "damaged cargo" (-0.05).
#[derive(Debug, Clone)]
pub struct SituationalModifier {
    pub label: String,
    pub value: f32,
}

// ---------------------------------------------------------------------------
// Check outcome
// ---------------------------------------------------------------------------

/// The five outcome tiers — a spectrum, not binary pass/fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OutcomeTier {
    CriticalFailure,
    Failure,
    Partial,
    Success,
    CriticalSuccess,
}

impl OutcomeTier {
    /// Short label for debug output and logging.
    pub fn label(self) -> &'static str {
        match self {
            OutcomeTier::CriticalSuccess => "critical success",
            OutcomeTier::Success => "success",
            OutcomeTier::Partial => "partial success",
            OutcomeTier::Failure => "failure",
            OutcomeTier::CriticalFailure => "critical failure",
        }
    }

    /// Is this a positive outcome (success or better)?
    pub fn is_success(self) -> bool {
        matches!(self, OutcomeTier::Success | OutcomeTier::CriticalSuccess)
    }

    /// Is this a negative outcome (failure or worse)?
    pub fn is_failure(self) -> bool {
        matches!(self, OutcomeTier::Failure | OutcomeTier::CriticalFailure)
    }
}

impl std::fmt::Display for OutcomeTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// The result of resolving a skill check.
#[derive(Debug, Clone)]
pub struct CheckOutcome {
    /// Which tier the check landed in.
    pub tier: OutcomeTier,
    /// How far above or below the threshold: positive = exceeded,
    /// negative = fell short. Useful for scaling consequences.
    pub margin: f32,
    /// The total modifier pool (before random factor).
    pub base_modifier: f32,
    /// The random factor that was applied (for debugging/logging).
    pub random_factor: f32,
    /// The final effective score (base_modifier + random_factor).
    pub effective_score: f32,
    /// The difficulty that was checked against.
    pub difficulty: f32,
    /// Breakdown of what contributed to the modifier pool.
    /// Useful for narrative framing ("your navigator's skill
    /// made the difference" vs. "damaged sensors left you blind").
    pub modifier_breakdown: Vec<ModifierContribution>,
}

/// A single contribution to the modifier pool, with its source.
#[derive(Debug, Clone)]
pub struct ModifierContribution {
    pub source: String,
    pub value: f32,
}

// ---------------------------------------------------------------------------
// Modifier weights (from the design doc)
// ---------------------------------------------------------------------------

/// Maximum contribution from the primary ship module's condition.
const MODULE_WEIGHT: f32 = 0.40;

/// Maximum contribution from the relevant crew member's skill.
const CREW_WEIGHT: f32 = 0.30;

/// Maximum contribution from faction standing.
const FACTION_WEIGHT: f32 = 0.15;

/// Maximum contribution from reputation profile.
/// Stubbed at 0.0 until the behavioral profile system is built.
const _REPUTATION_WEIGHT: f32 = 0.10;

/// Maximum contribution from ship upgrade bonuses.
/// Stubbed at a small flat value; will be driven by module variants later.
const UPGRADE_WEIGHT: f32 = 0.05;

/// The random factor range: ±15% of the 0.0–1.0 scale.
const RANDOM_RANGE: f32 = 0.15;

// ---------------------------------------------------------------------------
// Tier thresholds (margin = effective_score - difficulty)
// ---------------------------------------------------------------------------

const CRITICAL_SUCCESS_THRESHOLD: f32 = 0.30;
const SUCCESS_THRESHOLD: f32 = 0.05;
const PARTIAL_THRESHOLD: f32 = -0.10;
const FAILURE_THRESHOLD: f32 = -0.30;
// Below FAILURE_THRESHOLD = CriticalFailure.

fn tier_from_margin(margin: f32) -> OutcomeTier {
    if margin >= CRITICAL_SUCCESS_THRESHOLD {
        OutcomeTier::CriticalSuccess
    } else if margin >= SUCCESS_THRESHOLD {
        OutcomeTier::Success
    } else if margin >= PARTIAL_THRESHOLD {
        OutcomeTier::Partial
    } else if margin >= FAILURE_THRESHOLD {
        OutcomeTier::Failure
    } else {
        OutcomeTier::CriticalFailure
    }
}

// ---------------------------------------------------------------------------
// Core resolution
// ---------------------------------------------------------------------------

/// Resolve a skill check against the current journey state.
///
/// The RNG is passed in to maintain determinism — the caller controls
/// the seed, same pattern as the rest of the codebase.
pub fn resolve_check<R: Rng>(
    check: &SkillCheck,
    journey: &Journey,
    rng: &mut R,
) -> CheckOutcome {
    let mut breakdown = Vec::new();
    let mut total = 0.0_f32;

    // --- Module condition ---
    if let Some(module_target) = check.relevant_module {
        let condition = get_module_condition(&journey.ship.modules, module_target);
        let contribution = condition * MODULE_WEIGHT;
        breakdown.push(ModifierContribution {
            source: format!("{} condition ({:.0}%)", module_target.name(), condition * 100.0),
            value: contribution,
        });
        total += contribution;
    }

    // --- Crew skill ---
    if let Some(role) = check.relevant_role {
        let (contribution, source) = crew_contribution(&journey.crew, role);
        breakdown.push(ModifierContribution { source, value: contribution });
        total += contribution;
    }

    // --- Faction standing ---
    if let Some(faction_id) = check.faction_context {
        let contribution = faction_contribution(journey, faction_id);
        if contribution.abs() > 0.001 {
            breakdown.push(ModifierContribution {
                source: "faction standing".into(),
                value: contribution,
            });
            total += contribution;
        }
    }

    // --- Reputation modifier ---
    let rep_contribution = reputation_modifier(&journey.profile, check.reputation_domain);
    if rep_contribution.abs() > 0.001 {
        breakdown.push(ModifierContribution {
            source: "reputation".into(),
            value: rep_contribution,
        });
        total += rep_contribution;
    }

    // --- Ship upgrade bonus (simple version) ---
    // Non-standard module variants get a small bonus.
    if let Some(module_target) = check.relevant_module {
        let bonus = upgrade_bonus(&journey.ship.modules, module_target);
        if bonus > 0.0 {
            breakdown.push(ModifierContribution {
                source: "module upgrade".into(),
                value: bonus,
            });
            total += bonus;
        }
    }

    // --- Situational modifiers ---
    for sm in &check.situational_modifiers {
        breakdown.push(ModifierContribution {
            source: sm.label.clone(),
            value: sm.value,
        });
        total += sm.value;
    }

    // Clamp base modifier to a reasonable range.
    let base_modifier = total.clamp(0.0, 1.0);

    // --- Random factor ---
    let random_factor = rng.gen_range(-RANDOM_RANGE..=RANDOM_RANGE);
    let effective_score = (base_modifier + random_factor).clamp(0.0, 1.0);

    // --- Resolve tier ---
    let margin = effective_score - check.difficulty;
    let tier = tier_from_margin(margin);

    CheckOutcome {
        tier,
        margin,
        base_modifier,
        random_factor,
        effective_score,
        difficulty: check.difficulty,
        modifier_breakdown: breakdown,
    }
}

// ---------------------------------------------------------------------------
// Narrative framing helpers
// ---------------------------------------------------------------------------

/// Generate a pre-check narrative hint based on the computed odds.
/// This is what the player sees before committing to the action:
/// "Your engineer thinks she can reroute the power, but it's risky."
///
/// Returns a general difficulty impression, not specific numbers.
pub fn difficulty_impression(check: &SkillCheck, journey: &Journey) -> DifficultyImpression {
    // Compute the modifier pool without random factor.
    let mut total = 0.0_f32;

    if let Some(module_target) = check.relevant_module {
        let condition = get_module_condition(&journey.ship.modules, module_target);
        total += condition * MODULE_WEIGHT;
    }

    if let Some(role) = check.relevant_role {
        let (contribution, _) = crew_contribution(&journey.crew, role);
        total += contribution;
    }

    if let Some(faction_id) = check.faction_context {
        total += faction_contribution(journey, faction_id);
    }

    for sm in &check.situational_modifiers {
        total += sm.value;
    }

    // Reputation modifier.
    total += reputation_modifier(&journey.profile, check.reputation_domain);

    let base = total.clamp(0.0, 1.0);
    let expected_margin = base - check.difficulty;

    if expected_margin > 0.30 {
        DifficultyImpression::NearCertain
    } else if expected_margin > 0.10 {
        DifficultyImpression::Favorable
    } else if expected_margin > -0.05 {
        DifficultyImpression::EvenOdds
    } else if expected_margin > -0.20 {
        DifficultyImpression::Risky
    } else {
        DifficultyImpression::Desperate
    }
}

/// How the odds feel to the player — drives narrative framing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DifficultyImpression {
    /// "Straightforward. Your engineer barely looks up from her coffee."
    NearCertain,
    /// "Your navigator is confident. This should work."
    Favorable,
    /// "Could go either way. Your crew exchanges glances."
    EvenOdds,
    /// "Your engineer thinks she can do it, but it's risky."
    Risky,
    /// "Your navigator says this is suicide. You'd need incredible luck."
    Desperate,
}

impl DifficultyImpression {
    pub fn label(self) -> &'static str {
        match self {
            DifficultyImpression::NearCertain => "near certain",
            DifficultyImpression::Favorable => "favorable",
            DifficultyImpression::EvenOdds => "even odds",
            DifficultyImpression::Risky => "risky",
            DifficultyImpression::Desperate => "desperate",
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience builders
// ---------------------------------------------------------------------------

impl SkillCheck {
    /// A simple check against a single module and role.
    pub fn simple(
        module: ModuleTarget,
        role: CrewRole,
        difficulty: f32,
    ) -> Self {
        Self {
            relevant_module: Some(module),
            relevant_role: Some(role),
            difficulty,
            faction_context: None,
            situational_modifiers: vec![],
            reputation_domain: ReputationDomain::General,
        }
    }

    /// A check with no specific module or role — pure situational.
    pub fn unassisted(difficulty: f32) -> Self {
        Self {
            relevant_module: None,
            relevant_role: None,
            difficulty,
            faction_context: None,
            situational_modifiers: vec![],
            reputation_domain: ReputationDomain::General,
        }
    }

    /// Add a situational modifier.
    pub fn with_modifier(mut self, label: &str, value: f32) -> Self {
        self.situational_modifiers.push(SituationalModifier {
            label: label.into(),
            value,
        });
        self
    }

    /// Set the faction context.
    pub fn with_faction(mut self, faction_id: Uuid) -> Self {
        self.faction_context = Some(faction_id);
        self
    }

    /// Set the reputation domain.
    pub fn with_domain(mut self, domain: ReputationDomain) -> Self {
        self.reputation_domain = domain;
        self
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn get_module_condition(modules: &ShipModules, target: ModuleTarget) -> f32 {
    match target {
        ModuleTarget::Engine => modules.engine.condition,
        ModuleTarget::Sensors => modules.sensors.condition,
        ModuleTarget::Comms => modules.comms.condition,
        ModuleTarget::Weapons => modules.weapons.condition,
        ModuleTarget::LifeSupport => modules.life_support.condition,
    }
}

/// Find the best crew member for a role and compute their contribution.
///
/// Exact role match gets full weight. Adjacent roles (e.g., Engineer
/// for a Science check) get partial credit. Stress reduces effectiveness.
///
/// Returns (contribution, description).
fn crew_contribution(crew: &[CrewMember], target_role: CrewRole) -> (f32, String) {
    if crew.is_empty() {
        return (0.0, "no crew".into());
    }

    // Find the best match: exact role first, then adjacent roles.
    let mut best_score = 0.0_f32;
    let mut best_name = String::new();
    let mut match_type = "no match";

    for member in crew {
        let role_factor = role_match(member.role, target_role);
        if role_factor <= 0.0 {
            continue;
        }

        // Stress penalty: at 0.0 stress = full effectiveness,
        // at 1.0 stress = 40% effectiveness.
        let stress_factor = 1.0 - (member.state.stress * 0.6);
        let effective = role_factor * stress_factor;

        if effective > best_score {
            best_score = effective;
            best_name = member.name.clone();
            match_type = if (role_factor - 1.0).abs() < 0.01 {
                "specialist"
            } else {
                "assisting"
            };
        }
    }

    if best_score <= 0.0 {
        return (0.0, format!("no crew for {} role", target_role));
    }

    let contribution = best_score * CREW_WEIGHT;
    let desc = format!("{} ({}, {})", best_name, target_role, match_type);
    (contribution, desc)
}

/// How well a crew member's role matches the check's required role.
/// 1.0 = exact match, 0.5 = adjacent/transferable, 0.0 = no match.
fn role_match(crew_role: CrewRole, check_role: CrewRole) -> f32 {
    if crew_role == check_role {
        return 1.0;
    }

    // Adjacent skill transfers — an engineer has some pilot skill, etc.
    let transfer = match (crew_role, check_role) {
        // Engineer transfers.
        (CrewRole::Engineer, CrewRole::Science) => 0.5,
        (CrewRole::Engineer, CrewRole::Pilot) => 0.3,
        // Navigator transfers.
        (CrewRole::Navigator, CrewRole::Pilot) => 0.6,
        (CrewRole::Navigator, CrewRole::Science) => 0.4,
        // Pilot transfers.
        (CrewRole::Pilot, CrewRole::Navigator) => 0.5,
        (CrewRole::Pilot, CrewRole::Security) => 0.3,
        // Comms transfers.
        (CrewRole::Comms, CrewRole::Quartermaster) => 0.5,
        (CrewRole::Comms, CrewRole::Science) => 0.3,
        // Science transfers.
        (CrewRole::Science, CrewRole::Engineer) => 0.4,
        (CrewRole::Science, CrewRole::Medic) => 0.5,
        (CrewRole::Science, CrewRole::Navigator) => 0.3,
        // Medic transfers.
        (CrewRole::Medic, CrewRole::Science) => 0.4,
        // Security transfers.
        (CrewRole::Security, CrewRole::Pilot) => 0.3,
        // Quartermaster transfers.
        (CrewRole::Quartermaster, CrewRole::Comms) => 0.4,
        // General: small bonus to everything.
        (CrewRole::General, _) => 0.3,
        // No transfer.
        _ => 0.0,
    };

    transfer
}

/// Compute faction standing contribution for a check.
///
/// Positive standing helps (up to +0.15), negative standing
/// actively hinders (down to -0.10).
fn faction_contribution(journey: &Journey, faction_id: Uuid) -> f32 {
    // Check faction standings stored on the galaxy's faction objects.
    // For now, we'd need faction data passed in or stored on journey.
    // The journey has civ_standings but not per-faction standings yet.
    //
    // Stub: check civ_standings as a proxy. The controlling civ's
    // reputation gives a partial modifier.
    //
    // TODO: Wire to per-faction standing when the faction system
    // tracks player reputation directly on the journey.

    // For now, look through civ_standings for any positive/negative signal.
    // This is a simplification — faction_id is being used as a civ_id proxy.
    if let Some(standing) = journey.civ_standings.get(&faction_id) {
        // Reputation ranges -1.0 to 1.0. Scale to ±FACTION_WEIGHT.
        standing.reputation * FACTION_WEIGHT
    } else {
        0.0
    }
}

/// Check if a module has a non-standard variant that grants a bonus.
///
/// Standard variants get 0.0. Named/upgraded variants get a small
/// flat bonus. This will be expanded when the upgrade system is built.
fn upgrade_bonus(modules: &ShipModules, target: ModuleTarget) -> f32 {
    let variant = match target {
        ModuleTarget::Engine => &modules.engine.variant,
        ModuleTarget::Sensors => &modules.sensors.variant,
        ModuleTarget::Comms => &modules.comms.variant,
        ModuleTarget::Weapons => &modules.weapons.variant,
        ModuleTarget::LifeSupport => &modules.life_support.variant,
    };

    // Simple heuristic: if the variant name contains upgrade-y keywords,
    // grant the bonus. This is a placeholder for a proper upgrade system.
    let name_lower = variant.to_lowercase();
    if name_lower.contains("military")
        || name_lower.contains("mk.ii")
        || name_lower.contains("mk.iii")
        || name_lower.contains("advanced")
        || name_lower.contains("alien")
    {
        UPGRADE_WEIGHT
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use starbound_core::crew::*;
    use starbound_core::mission::*;
    use starbound_core::reputation::PlayerProfile;
    use starbound_core::ship::*;
    use starbound_core::time::Timestamp;

    fn test_journey_with_crew(crew: Vec<CrewMember>) -> Journey {
        Journey {
            ship: Ship {
                name: "Test Ship".into(),
                hull_condition: 1.0,
                fuel: 80.0,
                fuel_capacity: 100.0,
                supplies: 80.0,
                supply_capacity: 100.0,
                cargo: HashMap::new(),
                cargo_capacity: 50,
                modules: ShipModules {
                    engine: Module::standard("Cascade Drive Mk.II"),
                    sensors: Module::standard("Broadband Array"),
                    comms: Module::standard("Tightbeam Transceiver"),
                    weapons: Module::standard("Point Defense Grid"),
                    life_support: Module::standard("Closed-Loop Recycler"),
                },
            },
            current_system: Uuid::new_v4(),
            time: Timestamp::zero(),
            resources: 1000.0,
            mission: MissionState {
                mission_type: MissionType::Search,
                core_truth: "Test".into(),
                knowledge_nodes: vec![],
            },
            crew,
            threads: vec![],
            event_log: vec![],
            civ_standings: HashMap::new(),
            profile: PlayerProfile::new(),
            active_contracts: vec![],
            discovered_rumors: vec![],
            current_location: None,
        }
    }

    fn make_crew_member(name: &str, role: CrewRole, stress: f32) -> CrewMember {
        CrewMember {
            id: Uuid::new_v4(),
            name: name.into(),
            role,
            drives: PersonalityDrives {
                security: 0.5,
                freedom: 0.5,
                purpose: 0.5,
                connection: 0.5,
                knowledge: 0.5,
                justice: 0.5,
            },
            trust: Trust::starting_crew(),
            relationships: HashMap::new(),
            background: "Test crew.".into(),
            state: CrewState {
                mood: Mood::Content,
                stress,
                active_concerns: vec![],
            },
            origin: CrewOrigin::Starting,
        }
    }

    fn seeded_rng(seed: u64) -> StdRng {
        StdRng::seed_from_u64(seed)
    }

    // --- Determinism ---

    #[test]
    fn same_seed_same_outcome() {
        let crew = vec![make_crew_member("Nav", CrewRole::Navigator, 0.2)];
        let journey = test_journey_with_crew(crew);
        let check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.5);

        let result1 = resolve_check(&check, &journey, &mut seeded_rng(42));
        let result2 = resolve_check(&check, &journey, &mut seeded_rng(42));

        assert_eq!(result1.tier, result2.tier);
        assert!((result1.margin - result2.margin).abs() < f32::EPSILON);
        assert!((result1.random_factor - result2.random_factor).abs() < f32::EPSILON);
    }

    #[test]
    fn different_seeds_can_differ() {
        let crew = vec![make_crew_member("Nav", CrewRole::Navigator, 0.2)];
        let journey = test_journey_with_crew(crew);
        let check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.5);

        // Run many seeds and check we get at least 2 different outcomes.
        let mut tiers = std::collections::HashSet::new();
        for seed in 0..100 {
            let result = resolve_check(&check, &journey, &mut seeded_rng(seed));
            tiers.insert(result.tier);
        }
        assert!(tiers.len() >= 2, "Random factor should produce variety");
    }

    // --- Module condition matters ---

    #[test]
    fn damaged_module_reduces_chance() {
        let crew = vec![make_crew_member("Nav", CrewRole::Navigator, 0.1)];

        // Good engine.
        let good_journey = test_journey_with_crew(crew.clone());

        // Bad engine.
        let mut bad_journey = test_journey_with_crew(crew);
        bad_journey.ship.modules.engine.condition = 0.2;

        let check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.4);

        let good_result = resolve_check(&check, &good_journey, &mut seeded_rng(42));
        let bad_result = resolve_check(&check, &bad_journey, &mut seeded_rng(42));

        // Same seed, so random factor identical. Base modifier should differ.
        assert!(good_result.base_modifier > bad_result.base_modifier,
            "Good engine ({:.3}) should give higher modifier than damaged ({:.3})",
            good_result.base_modifier, bad_result.base_modifier);
    }

    // --- Crew skill matters ---

    #[test]
    fn matching_crew_role_helps() {
        // Navigator for a navigator check.
        let matched = vec![make_crew_member("Nav", CrewRole::Navigator, 0.1)];
        let matched_journey = test_journey_with_crew(matched);

        // Security for a navigator check — poor match.
        let mismatched = vec![make_crew_member("Guard", CrewRole::Security, 0.1)];
        let mismatched_journey = test_journey_with_crew(mismatched);

        let check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.4);

        let matched_result = resolve_check(&check, &matched_journey, &mut seeded_rng(42));
        let mismatched_result = resolve_check(&check, &mismatched_journey, &mut seeded_rng(42));

        assert!(matched_result.base_modifier > mismatched_result.base_modifier,
            "Matching role ({:.3}) should beat mismatched ({:.3})",
            matched_result.base_modifier, mismatched_result.base_modifier);
    }

    #[test]
    fn no_crew_still_resolves() {
        let journey = test_journey_with_crew(vec![]);
        let check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.3);

        let result = resolve_check(&check, &journey, &mut seeded_rng(42));

        // Should still resolve — just with lower modifier.
        assert!(result.base_modifier < 0.5, "No crew should mean lower odds");
    }

    // --- Stress reduces effectiveness ---

    #[test]
    fn stressed_crew_performs_worse() {
        let calm = vec![make_crew_member("Nav", CrewRole::Navigator, 0.0)];
        let calm_journey = test_journey_with_crew(calm);

        let stressed = vec![make_crew_member("Nav", CrewRole::Navigator, 0.9)];
        let stressed_journey = test_journey_with_crew(stressed);

        let check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.4);

        let calm_result = resolve_check(&check, &calm_journey, &mut seeded_rng(42));
        let stressed_result = resolve_check(&check, &stressed_journey, &mut seeded_rng(42));

        assert!(calm_result.base_modifier > stressed_result.base_modifier,
            "Calm crew ({:.3}) should outperform stressed ({:.3})",
            calm_result.base_modifier, stressed_result.base_modifier);
    }

    // --- Tier boundaries ---

    #[test]
    fn tier_thresholds_are_correct() {
        assert_eq!(tier_from_margin(0.35), OutcomeTier::CriticalSuccess);
        assert_eq!(tier_from_margin(0.30), OutcomeTier::CriticalSuccess);
        assert_eq!(tier_from_margin(0.15), OutcomeTier::Success);
        assert_eq!(tier_from_margin(0.05), OutcomeTier::Success);
        assert_eq!(tier_from_margin(0.0), OutcomeTier::Partial);
        assert_eq!(tier_from_margin(-0.10), OutcomeTier::Partial);
        assert_eq!(tier_from_margin(-0.15), OutcomeTier::Failure);
        assert_eq!(tier_from_margin(-0.30), OutcomeTier::Failure);
        assert_eq!(tier_from_margin(-0.31), OutcomeTier::CriticalFailure);
        assert_eq!(tier_from_margin(-0.50), OutcomeTier::CriticalFailure);
    }

    // --- Outcome tier API ---

    #[test]
    fn outcome_tier_classification() {
        assert!(OutcomeTier::CriticalSuccess.is_success());
        assert!(OutcomeTier::Success.is_success());
        assert!(!OutcomeTier::Partial.is_success());
        assert!(!OutcomeTier::Partial.is_failure());
        assert!(OutcomeTier::Failure.is_failure());
        assert!(OutcomeTier::CriticalFailure.is_failure());
    }

    // --- Difficulty impression ---

    #[test]
    fn easy_check_reads_as_near_certain() {
        let crew = vec![make_crew_member("Nav", CrewRole::Navigator, 0.1)];
        let journey = test_journey_with_crew(crew);

        let easy = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.1);
        assert_eq!(difficulty_impression(&easy, &journey), DifficultyImpression::NearCertain);
    }

    #[test]
    fn hard_check_reads_as_desperate() {
        let crew = vec![make_crew_member("Nav", CrewRole::Navigator, 0.5)];
        let mut journey = test_journey_with_crew(crew);
        journey.ship.modules.engine.condition = 0.3;

        let hard = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.9);
        let impression = difficulty_impression(&hard, &journey);
        assert!(
            impression == DifficultyImpression::Desperate
                || impression == DifficultyImpression::Risky,
            "Hard check with damaged ship should be desperate or risky, got {:?}",
            impression,
        );
    }

    // --- Situational modifiers ---

    #[test]
    fn situational_modifiers_apply() {
        let crew = vec![make_crew_member("Nav", CrewRole::Navigator, 0.1)];
        let journey = test_journey_with_crew(crew);

        let base_check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.5);
        let boosted_check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.5)
            .with_modifier("element of surprise", 0.15);

        let base_result = resolve_check(&base_check, &journey, &mut seeded_rng(42));
        let boosted_result = resolve_check(&boosted_check, &journey, &mut seeded_rng(42));

        assert!(boosted_result.base_modifier > base_result.base_modifier);
    }

    // --- Upgrade bonus ---

    #[test]
    fn mk_ii_engine_gives_upgrade_bonus() {
        // The default test ship has "Cascade Drive Mk.II" — should trigger.
        let crew = vec![make_crew_member("Nav", CrewRole::Navigator, 0.1)];
        let journey = test_journey_with_crew(crew);

        let check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.5);
        let result = resolve_check(&check, &journey, &mut seeded_rng(42));

        // Check that the breakdown includes an upgrade bonus.
        let has_upgrade = result.modifier_breakdown.iter()
            .any(|m| m.source.contains("upgrade") && m.value > 0.0);
        assert!(has_upgrade, "Mk.II engine should provide upgrade bonus");
    }

    // --- Modifier breakdown is populated ---

    #[test]
    fn breakdown_includes_all_sources() {
        let crew = vec![make_crew_member("Nav", CrewRole::Navigator, 0.1)];
        let journey = test_journey_with_crew(crew);

        let check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.5)
            .with_modifier("test bonus", 0.05);

        let result = resolve_check(&check, &journey, &mut seeded_rng(42));

        // Should have at least: module, crew, situational, possibly upgrade.
        assert!(result.modifier_breakdown.len() >= 3,
            "Expected at least 3 modifier sources, got {}",
            result.modifier_breakdown.len());
    }

    // --- Edge cases ---

    #[test]
    fn unassisted_check_still_works() {
        let journey = test_journey_with_crew(vec![]);
        let check = SkillCheck::unassisted(0.3);

        let result = resolve_check(&check, &journey, &mut seeded_rng(42));

        // With no module and no crew, base modifier should be 0.
        assert!((result.base_modifier - 0.0).abs() < 0.01);
    }

    #[test]
    fn trivial_difficulty_usually_succeeds() {
        let crew = vec![make_crew_member("Nav", CrewRole::Navigator, 0.1)];
        let journey = test_journey_with_crew(crew);

        let check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 0.0);

        let mut successes = 0;
        for seed in 0..100 {
            let result = resolve_check(&check, &journey, &mut seeded_rng(seed));
            if result.tier.is_success() || result.tier == OutcomeTier::CriticalSuccess {
                successes += 1;
            }
        }
        // With a good ship and crew vs difficulty 0.0, should succeed almost always.
        assert!(successes >= 90, "Expected 90+ successes, got {}", successes);
    }

    #[test]
    fn impossible_difficulty_usually_fails() {
        let crew = vec![make_crew_member("Nav", CrewRole::Navigator, 0.5)];
        let mut journey = test_journey_with_crew(crew);
        journey.ship.modules.engine.condition = 0.3;

        let check = SkillCheck::simple(ModuleTarget::Engine, CrewRole::Navigator, 1.0);

        let mut failures = 0;
        for seed in 0..100 {
            let result = resolve_check(&check, &journey, &mut seeded_rng(seed));
            if result.tier.is_failure() {
                failures += 1;
            }
        }
        assert!(failures >= 80, "Expected 80+ failures vs impossible, got {}", failures);
    }
}