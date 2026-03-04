// file: crates/game/src/supplies.rs
//! Supply consumption — the clock that forces the player back to civilization.
//!
//! Supplies (food, air, water, maintenance materials) are consumed
//! per personal-day. The rate depends on crew size and life support
//! condition. Running low creates mounting pressure; running out
//! triggers a death spiral of hull damage, crew stress, and system
//! degradation.
//!
//! Design principle: supplies create a planning horizon. The player
//! can't wander indefinitely — they need to return to civilization
//! or find frontier sources. This makes route planning meaningful
//! and creates tension between exploration and survival.

use starbound_core::crew::Mood;
use starbound_core::journey::Journey;

// ---------------------------------------------------------------------------
// Supply status thresholds
// ---------------------------------------------------------------------------

/// How the ship's supply situation reads at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupplyStatus {
    /// Above 50% — no concerns.
    Comfortable,
    /// 20–50% — the quartermaster is watching the numbers.
    Adequate,
    /// 5–20% — crew stress rising, morale dropping, urgency building.
    Low,
    /// 0–5% — rationing. Hull damage from jury-rigging, systems degrading.
    Critical,
    /// 0% — the death spiral. Everything breaks.
    Depleted,
}

impl SupplyStatus {
    /// Assess the current supply situation.
    pub fn assess(supplies: f32, capacity: f32) -> Self {
        if capacity <= 0.0 {
            return SupplyStatus::Depleted;
        }
        let fraction = supplies / capacity;
        if fraction <= 0.0 {
            SupplyStatus::Depleted
        } else if fraction <= 0.05 {
            SupplyStatus::Critical
        } else if fraction <= 0.20 {
            SupplyStatus::Low
        } else if fraction <= 0.50 {
            SupplyStatus::Adequate
        } else {
            SupplyStatus::Comfortable
        }
    }

    /// Human-readable label for the CLI.
    pub fn label(self) -> &'static str {
        match self {
            SupplyStatus::Comfortable => "comfortable",
            SupplyStatus::Adequate => "adequate",
            SupplyStatus::Low => "LOW",
            SupplyStatus::Critical => "CRITICAL",
            SupplyStatus::Depleted => "DEPLETED",
        }
    }

    /// Whether the situation warrants a warning in the UI.
    pub fn is_warning(self) -> bool {
        matches!(self, SupplyStatus::Low | SupplyStatus::Critical | SupplyStatus::Depleted)
    }
}

// ---------------------------------------------------------------------------
// Consumption rate
// ---------------------------------------------------------------------------

/// Base supply consumption per crew member per personal-day.
const BASE_RATE_PER_CREW: f32 = 0.1;

/// Life support efficiency thresholds.
/// Good life support wastes less. Damaged life support wastes more.
fn life_support_multiplier(life_support_condition: f32) -> f32 {
    if life_support_condition >= 0.7 {
        // Good condition — efficient recycling.
        1.0
    } else if life_support_condition >= 0.4 {
        // Degraded — some waste, higher consumption.
        1.3
    } else if life_support_condition >= 0.1 {
        // Badly damaged — significant waste.
        1.8
    } else {
        // Non-functional — no recycling at all.
        2.5
    }
}

/// Calculate supply consumption rate (units per personal-day).
///
/// Formula: crew_count × BASE_RATE × life_support_multiplier
///
/// A standard 3-person crew with good life support consumes 0.3/day,
/// meaning 100 units lasts ~333 days (~11 months). Damage the life
/// support and that horizon shrinks fast.
pub fn consumption_rate(crew_count: usize, life_support_condition: f32) -> f32 {
    let crew = crew_count.max(1) as f32; // Minimum 1 (the captain)
    crew * BASE_RATE_PER_CREW * life_support_multiplier(life_support_condition)
}

/// Estimate how many personal-days of supplies remain at current rate.
pub fn days_remaining(journey: &Journey) -> f64 {
    let rate = consumption_rate(
        journey.crew.len(),
        journey.ship.modules.life_support.condition,
    );
    if rate <= 0.0 {
        return f64::INFINITY;
    }
    journey.ship.supplies as f64 / rate as f64
}

// ---------------------------------------------------------------------------
// Supply consumption report
// ---------------------------------------------------------------------------

/// What happened when supplies were consumed during a time span.
#[derive(Debug, Clone)]
pub struct SupplyReport {
    /// Units of supplies consumed.
    pub consumed: f32,
    /// Supply level after consumption.
    pub remaining: f32,
    /// Status after consumption.
    pub status: SupplyStatus,
    /// Warnings or narrative notes to display.
    pub warnings: Vec<String>,
    /// Whether depletion effects were applied (hull damage, stress, etc.).
    pub depletion_effects_applied: bool,
    /// Days spent in depleted state (for severity calculation).
    pub depleted_days: f32,
}

// ---------------------------------------------------------------------------
// Core consumption function
// ---------------------------------------------------------------------------

/// Consume supplies for a span of personal-days.
///
/// This is called during travel execution and during local time actions.
/// It deducts supplies and applies consequences when levels drop:
///
/// - **Low** (below 20%): crew stress +0.02 per day of low supplies.
/// - **Critical** (below 5%): crew stress +0.05 per day, morale drops.
/// - **Depleted** (0%): hull damage, life support degradation, heavy stress.
///   This is a death spiral — the ship eats itself to keep the crew alive.
pub fn consume_supplies(journey: &mut Journey, personal_days: f64) -> SupplyReport {
    let rate = consumption_rate(
        journey.crew.len(),
        journey.ship.modules.life_support.condition,
    );

    let total_consumption = rate * personal_days as f32;
    let supplies_before = journey.ship.supplies;

    // How many days were spent in each status zone.
    // We simulate day-by-day to get the right proportions.
    let days_low;
    let days_critical;
    let days_depleted;

    if rate <= 0.0 || total_consumption <= 0.0 {
        // Edge case: no consumption (somehow).
        return SupplyReport {
            consumed: 0.0,
            remaining: journey.ship.supplies,
            status: SupplyStatus::assess(journey.ship.supplies, journey.ship.supply_capacity),
            warnings: vec![],
            depletion_effects_applied: false,
            depleted_days: 0.0,
        };
    }

    // Calculate thresholds in absolute supply units.
    let low_threshold = journey.ship.supply_capacity * 0.20;
    let critical_threshold = journey.ship.supply_capacity * 0.05;

    // Figure out how many days were spent below each threshold.
    // Instead of simulating tick-by-tick, compute analytically.
    let supplies_after_raw = supplies_before - total_consumption;
    let supplies_after = supplies_after_raw.max(0.0);
    let actual_consumed = supplies_before - supplies_after;

    // Days in each zone (proportional to supply units consumed in that zone).
    let days_per_unit = personal_days as f32 / total_consumption;

    // Supplies consumed while above low threshold.
    let consumed_above_low = (supplies_before - low_threshold).max(0.0)
        .min(actual_consumed);
    // Supplies consumed between low and critical.
    let consumed_in_low = (supplies_before.min(low_threshold) - critical_threshold).max(0.0)
        .min((actual_consumed - consumed_above_low).max(0.0));
    // Supplies consumed between critical and zero.
    let consumed_in_critical = (supplies_before.min(critical_threshold)).max(0.0)
        .min((actual_consumed - consumed_above_low - consumed_in_low).max(0.0));
    // Days past zero (if consumption exceeded remaining).
    let overconsumption = (-supplies_after_raw).max(0.0);
    days_depleted = overconsumption * days_per_unit;

    days_low = consumed_in_low * days_per_unit;
    days_critical = consumed_in_critical * days_per_unit;

    // Apply the consumption.
    journey.ship.supplies = supplies_after;

    // --- Apply consequences based on time spent in each zone ---

    let mut warnings = Vec::new();
    let mut depletion_effects_applied = false;

    // Low supplies: gradual stress accumulation.
    if days_low > 0.0 {
        let stress_delta = 0.02 * (days_low / 30.0); // Slow buildup.
        for member in &mut journey.crew {
            member.state.stress = (member.state.stress + stress_delta).min(1.0);
        }
        warnings.push("Supplies running low. Crew is uneasy.".into());
    }

    // Critical supplies: faster stress, morale impact.
    if days_critical > 0.0 {
        let stress_delta = 0.05 * (days_critical / 30.0);
        for member in &mut journey.crew {
            member.state.stress = (member.state.stress + stress_delta).min(1.0);
            // High-security crew members become anxious.
            if member.drives.security > 0.5 && member.state.mood != Mood::Anxious {
                member.state.mood = Mood::Anxious;
            }
        }
        warnings.push("Supplies critical. Rationing in effect.".into());
    }

    // Depleted: the death spiral.
    if days_depleted > 0.0 {
        depletion_effects_applied = true;

        // Hull damage — jury-rigging consumes ship structure.
        let hull_damage = 0.01 * (days_depleted / 10.0);
        journey.ship.hull_condition = (journey.ship.hull_condition - hull_damage).max(0.0);

        // Life support degrades without maintenance materials.
        let ls_damage = 0.02 * (days_depleted / 10.0);
        journey.ship.modules.life_support.condition =
            (journey.ship.modules.life_support.condition - ls_damage).max(0.0);

        // Heavy stress.
        let stress_delta = 0.10 * (days_depleted / 10.0);
        for member in &mut journey.crew {
            member.state.stress = (member.state.stress + stress_delta).min(1.0);
            member.state.mood = Mood::Anxious;
        }

        warnings.push(format!(
            "SUPPLIES DEPLETED for {:.0} days. Hull and life support taking damage.",
            days_depleted,
        ));
    }

    let status = SupplyStatus::assess(journey.ship.supplies, journey.ship.supply_capacity);

    // Contextual warning when crossing below 20% for the first time in this span.
    if supplies_before > low_threshold && supplies_after <= low_threshold && days_depleted == 0.0 {
        warnings.push("Supplies have dropped below 20%. Find a port soon.".into());
    }

    SupplyReport {
        consumed: actual_consumed,
        remaining: supplies_after,
        status,
        warnings,
        depletion_effects_applied,
        depleted_days: days_depleted,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use starbound_core::crew::*;
    use starbound_core::mission::*;
    use starbound_core::ship::*;
    use starbound_core::reputation::PlayerProfile;
    use starbound_core::time::Timestamp;
    use uuid::Uuid;

    fn test_journey_with_supplies(supplies: f32, crew_count: usize) -> Journey {
        let crew: Vec<CrewMember> = (0..crew_count)
            .map(|i| CrewMember {
                id: Uuid::new_v4(),
                name: format!("Crew {}", i),
                role: CrewRole::General,
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
                    stress: 0.1,
                    active_concerns: vec![],
                },
                origin: CrewOrigin::Starting,
            })
            .collect();

        Journey {
            ship: Ship {
                name: "Test Ship".into(),
                hull_condition: 1.0,
                fuel: 80.0,
                fuel_capacity: 100.0,
                supplies,
                supply_capacity: 100.0,
                cargo: HashMap::new(),
                cargo_capacity: 50,
                modules: ShipModules {
                    engine: Module::standard("Test Engine"),
                    sensors: Module::standard("Sensors"),
                    comms: Module::standard("Comms"),
                    weapons: Module::standard("Weapons"),
                    life_support: Module::standard("Life Support"),
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
        }
    }

    #[test]
    fn consumption_rate_scales_with_crew_size() {
        // 3 crew, good life support = 0.3/day.
        let rate = consumption_rate(3, 1.0);
        assert!((rate - 0.3).abs() < 0.001);

        // 5 crew, good life support = 0.5/day.
        let rate = consumption_rate(5, 1.0);
        assert!((rate - 0.5).abs() < 0.001);
    }

    #[test]
    fn damaged_life_support_increases_consumption() {
        let good = consumption_rate(3, 1.0);
        let degraded = consumption_rate(3, 0.5);
        let broken = consumption_rate(3, 0.05);

        assert!(degraded > good);
        assert!(broken > degraded);
    }

    #[test]
    fn comfortable_supplies_no_side_effects() {
        let mut journey = test_journey_with_supplies(80.0, 3);
        let report = consume_supplies(&mut journey, 10.0);

        // 3 crew × 0.1 × 10 days = 3.0 consumed.
        assert!((report.consumed - 3.0).abs() < 0.1);
        assert_eq!(report.status, SupplyStatus::Comfortable);
        assert!(report.warnings.is_empty());
        assert!(!report.depletion_effects_applied);

        // Crew stress unchanged.
        for member in &journey.crew {
            assert!((member.state.stress - 0.1).abs() < 0.01);
        }
    }

    #[test]
    fn low_supplies_increase_crew_stress() {
        // Start at 15% (15 out of 100) — already in "Low" zone.
        let mut journey = test_journey_with_supplies(15.0, 3);
        let initial_stress = journey.crew[0].state.stress;

        let report = consume_supplies(&mut journey, 30.0);

        assert!(report.status == SupplyStatus::Low || report.status == SupplyStatus::Critical);
        assert!(!report.warnings.is_empty());

        // Stress should have increased.
        assert!(journey.crew[0].state.stress > initial_stress);
    }

    #[test]
    fn depleted_supplies_damage_ship() {
        // Start at 1.0 supplies — will deplete quickly.
        let mut journey = test_journey_with_supplies(1.0, 3);
        let hull_before = journey.ship.hull_condition;
        let ls_before = journey.ship.modules.life_support.condition;

        let report = consume_supplies(&mut journey, 100.0);

        assert_eq!(report.status, SupplyStatus::Depleted);
        assert!(report.depletion_effects_applied);
        assert!(report.depleted_days > 0.0);

        // Hull took damage.
        assert!(journey.ship.hull_condition < hull_before);
        // Life support took damage.
        assert!(journey.ship.modules.life_support.condition < ls_before);
    }

    #[test]
    fn supplies_cannot_go_negative() {
        let mut journey = test_journey_with_supplies(5.0, 3);
        consume_supplies(&mut journey, 1000.0);

        assert!(journey.ship.supplies >= 0.0);
    }

    #[test]
    fn days_remaining_estimate() {
        let journey = test_journey_with_supplies(100.0, 3);
        let days = days_remaining(&journey);

        // 100 / 0.3 ≈ 333 days.
        assert!(days > 300.0 && days < 350.0);
    }

    #[test]
    fn supply_status_thresholds() {
        assert_eq!(SupplyStatus::assess(80.0, 100.0), SupplyStatus::Comfortable);
        assert_eq!(SupplyStatus::assess(35.0, 100.0), SupplyStatus::Adequate);
        assert_eq!(SupplyStatus::assess(15.0, 100.0), SupplyStatus::Low);
        assert_eq!(SupplyStatus::assess(3.0, 100.0), SupplyStatus::Critical);
        assert_eq!(SupplyStatus::assess(0.0, 100.0), SupplyStatus::Depleted);
    }

    #[test]
    fn zero_crew_still_consumes_minimum() {
        // Even with 0 crew listed, the captain (minimum 1) consumes supplies.
        let rate = consumption_rate(0, 1.0);
        assert!(rate > 0.0);
    }
}