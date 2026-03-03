// file: crates/encounters/src/matcher.rs
//! Event matching — filters the seed library against current game state.
//!
//! This is the Phase 1 encounter pipeline: no LLM, no thread weaving,
//! just context-appropriate event selection from the seed library.
//! The full pipeline (Day 5) will layer pressure, echo, and novelty
//! filtering on top of this foundation.

use starbound_core::galaxy::{InfrastructureLevel, StarSystem};
use starbound_core::journey::Journey;

use super::seed_event::{ContextRequirements, SeedEvent};

/// Game state distilled into what the matcher needs.
/// Keeps the matcher decoupled from the full game state.
pub struct MatchContext<'a> {
    pub system: &'a StarSystem,
    pub journey: &'a Journey,
    /// Galactic years since the player last visited this system (None if first visit).
    pub galactic_years_since_last_visit: Option<f64>,
}

/// Filter seed events to those whose requirements match the current context.
/// Returns events sorted by specificity (most specific requirements first),
/// so the caller can pick the best match or randomize among top candidates.
pub fn match_events<'a>(
    events: &'a [SeedEvent],
    ctx: &MatchContext,
) -> Vec<&'a SeedEvent> {
    let mut matched: Vec<(&SeedEvent, usize)> = events
        .iter()
        .filter(|e| requirements_met(&e.context_requirements, ctx))
        .map(|e| (e, specificity(&e.context_requirements)))
        .collect();

    // Sort by specificity descending — most specific events first.
    matched.sort_by(|a, b| b.1.cmp(&a.1));

    matched.into_iter().map(|(e, _)| e).collect()
}

/// Check whether all requirements of an event are satisfied.
fn requirements_met(req: &ContextRequirements, ctx: &MatchContext) -> bool {
    // Infrastructure minimum.
    if let Some(ref min) = req.infrastructure_min {
        if let Some(min_level) = parse_infrastructure(min) {
            if infra_rank(ctx.system.infrastructure_level) < infra_rank(min_level) {
                return false;
            }
        }
    }

    // Infrastructure maximum.
    if let Some(ref max) = req.infrastructure_max {
        if let Some(max_level) = parse_infrastructure(max) {
            if infra_rank(ctx.system.infrastructure_level) > infra_rank(max_level) {
                return false;
            }
        }
    }

    // Faction controlled.
    if let Some(true) = req.faction_controlled {
        if ctx.system.controlling_faction.is_none() {
            return false;
        }
    }

    // Unclaimed.
    if let Some(true) = req.unclaimed {
        if ctx.system.controlling_faction.is_some() {
            return false;
        }
    }

    // Time since last visit.
    if let Some(min_years) = req.time_since_last_visit_galactic_years_min {
        match ctx.galactic_years_since_last_visit {
            Some(years) if years >= min_years => {}
            None => {} // First visit — always satisfies "time since last visit"
            _ => return false,
        }
    }

    // Fuel below threshold.
    if let Some(threshold) = req.fuel_below_fraction {
        let fuel_fraction = ctx.journey.ship.fuel / ctx.journey.ship.fuel_capacity;
        if fuel_fraction >= threshold {
            return false;
        }
    }

    // Hull below threshold.
    if let Some(threshold) = req.hull_below {
        if ctx.journey.ship.hull_condition >= threshold {
            return false;
        }
    }

    // Crew minimum.
    if let Some(min) = req.crew_min {
        if ctx.journey.crew.len() < min {
            return false;
        }
    }

    true
}

/// How specific are these requirements? More conditions = more specific.
/// Used to prefer events tailored to the current situation over generic ones.
fn specificity(req: &ContextRequirements) -> usize {
    let mut score = 0;
    if req.infrastructure_min.is_some() { score += 1; }
    if req.infrastructure_max.is_some() { score += 1; }
    if req.faction_controlled.is_some() { score += 1; }
    if req.unclaimed.is_some() { score += 1; }
    if req.time_since_last_visit_galactic_years_min.is_some() { score += 2; } // Extra weight
    if req.fuel_below_fraction.is_some() { score += 2; }
    if req.hull_below.is_some() { score += 2; }
    if req.crew_min.is_some() { score += 1; }
    if !req.tags.is_empty() { score += 1; }
    score
}

fn parse_infrastructure(s: &str) -> Option<InfrastructureLevel> {
    match s.to_lowercase().as_str() {
        "none" => Some(InfrastructureLevel::None),
        "outpost" => Some(InfrastructureLevel::Outpost),
        "colony" => Some(InfrastructureLevel::Colony),
        "established" => Some(InfrastructureLevel::Established),
        "hub" => Some(InfrastructureLevel::Hub),
        "capital" => Some(InfrastructureLevel::Capital),
        _ => None,
    }
}

fn infra_rank(level: InfrastructureLevel) -> u8 {
    match level {
        InfrastructureLevel::None => 0,
        InfrastructureLevel::Outpost => 1,
        InfrastructureLevel::Colony => 2,
        InfrastructureLevel::Established => 3,
        InfrastructureLevel::Hub => 4,
        InfrastructureLevel::Capital => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::all_seed_events;
    use std::collections::HashMap;
    use starbound_core::galaxy::*;
    use starbound_core::mission::*;
    use starbound_core::ship::*;
    use starbound_core::time::Timestamp;
    use uuid::Uuid;

    fn test_system(infra: InfrastructureLevel, faction: Option<Uuid>) -> StarSystem {
        StarSystem {
            id: Uuid::new_v4(),
            name: "Test System".into(),
            position: (0.0, 0.0),
            star_type: StarType::YellowDwarf,
            planetary_bodies: vec![],
            controlling_faction: faction,
            infrastructure_level: infra,
            history: vec![],
            active_threads: vec![],
                time_factor: 1.0,
        }
    }

    fn test_journey(fuel: f32, hull: f32, crew_count: usize) -> Journey {
        use starbound_core::crew::*;

        let crew: Vec<CrewMember> = (0..crew_count)
            .map(|i| CrewMember {
                id: Uuid::new_v4(),
                name: format!("Crew {}", i),
                role: CrewRole::Navigator,
                drives: PersonalityDrives {
                    security: 0.5, freedom: 0.5, purpose: 0.5,
                    connection: 0.5, knowledge: 0.5, justice: 0.5,
                },
                trust: Trust::starting_crew(),
                relationships: HashMap::new(),
                background: String::new(),
                state: CrewState {
                    mood: Mood::Content,
                    stress: 0.2,
                    active_concerns: vec![],
                },
                origin: CrewOrigin::Starting,
            })
            .collect();

        Journey {
            ship: Ship {
                name: "Test Ship".into(),
                hull_condition: hull,
                fuel,
                fuel_capacity: 100.0,
                cargo: HashMap::new(),
                cargo_capacity: 50,
                modules: ShipModules {
                    engine: Module::standard("Engine"),
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
        }
    }

    #[test]
    fn empty_system_matches_empty_system_event() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::None, None);
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"empty_system_arrival"),
            "Empty system should match empty_system_arrival, got: {:?}", ids);
    }

    #[test]
    fn hub_matches_trade_hub_bustle() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Hub, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"trade_hub_bustle"),
            "Hub should match trade_hub_bustle, got: {:?}", ids);
    }

    #[test]
    fn low_fuel_matches_fuel_event() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(20.0, 0.9, 3); // 20% fuel
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"fuel_merchant_desperate"),
            "Low fuel at colony should match fuel_merchant_desperate, got: {:?}", ids);
    }

    #[test]
    fn damaged_hull_matches_damage_event() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Outpost, None);
        let journey = test_journey(50.0, 0.3, 3); // Hull at 30%
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"damaged_and_limping"),
            "Damaged ship should match damaged_and_limping, got: {:?}", ids);
    }

    #[test]
    fn unclaimed_space_matches_pirate_warning() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::None, None);
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"pirate_warning"),
            "Unclaimed space should match pirate_warning, got: {:?}", ids);
    }

    #[test]
    fn time_dilation_return_matches_colony_generations() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: Some(80.0), // 80 years
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"colony_generations"),
            "Returning after 80 years should match colony_generations, got: {:?}", ids);
    }

    #[test]
    fn specificity_ordering() {
        let events = all_seed_events();
        // Colony with faction, low fuel, long absence — multiple events match
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(20.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: Some(60.0),
        };

        let matched = match_events(&events, &ctx);
        assert!(matched.len() >= 3, "Should match multiple events");

        // Most specific events should come first
        if matched.len() >= 2 {
            // The first event should have specificity >= the last
            let first_spec = specificity(&matched[0].context_requirements);
            let last_spec = specificity(&matched[matched.len() - 1].context_requirements);
            assert!(first_spec >= last_spec,
                "Events should be ordered by specificity: first={}, last={}", first_spec, last_spec);
        }
    }

    #[test]
    fn no_crew_excludes_crew_events() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 0); // No crew
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(!ids.contains(&"crew_quiet_moment"),
            "No crew should not match crew_quiet_moment");
    }
}