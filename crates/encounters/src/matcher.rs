// file: crates/encounters/src/matcher.rs
//! Event matching — filters the seed library against current game state.
//!
//! This is the Phase 1 encounter pipeline: no LLM, no thread weaving,
//! just context-appropriate event selection from the seed library.
//! The full pipeline (Day 5) will layer pressure, echo, and novelty
//! filtering on top of this foundation.
//!
//! Phase C additions: faction presence matching. Events can require
//! a specific faction category to be present, with optional strength
//! and visibility gates. Also supports time_factor_min for encounters
//! tied to anomalous spacetime.

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
        if ctx.system.controlling_civ.is_none() {
            return false;
        }
    }

    // Unclaimed.
    if let Some(true) = req.unclaimed {
        if ctx.system.controlling_civ.is_some() {
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

    // -------------------------------------------------------------------
    // Faction presence requirements (Phase C)
    // -------------------------------------------------------------------

    // Faction category present — find a matching presence at this system.
    if let Some(ref category) = req.faction_category_present {
        let category_lower = category.to_lowercase();

        // Find any faction presence whose category matches.
        // We check the category string against the faction_presence entries.
        // Since FactionPresence only stores faction_id (not category), and
        // we don't have access to the full Faction list here, we use a
        // convention: tags include the category for faction-gated events,
        // OR we match against the presence's services as a proxy.
        //
        // Better approach: match against system.faction_presence directly.
        // The matcher receives the StarSystem which has faction_presence.
        // We need to check if any presence matches the required category.
        //
        // Since we can't resolve faction_id → category without the faction
        // list, we match using a service-based heuristic:
        //   military → has Intelligence + Missions
        //   economic → has Trade
        //   guild → has Repair
        //   religious → has Shelter
        //   criminal → has Smuggling
        //
        // This is intentionally loose — the pipeline stage that selects
        // from matched events will do the fine-grained faction check.
        let matching_presence = ctx.system.faction_presence.iter().find(|fp| {
            let strength_ok = req.faction_min_strength
                .map(|min| fp.strength >= min)
                .unwrap_or(true);
            let vis_ok = req.faction_max_visibility
                .map(|max| fp.visibility <= max)
                .unwrap_or(true);

            if !strength_ok || !vis_ok {
                return false;
            }

            // Category matching via service heuristic.
            match category_lower.as_str() {
                "military" => fp.services.iter().any(|s| {
                    matches!(s, starbound_core::galaxy::FactionService::Intelligence)
                }) && fp.services.iter().any(|s| {
                    matches!(s, starbound_core::galaxy::FactionService::Missions)
                }),
                "economic" => fp.services.iter().any(|s| {
                    matches!(s, starbound_core::galaxy::FactionService::Trade)
                }),
                "guild" => fp.services.iter().any(|s| {
                    matches!(s, starbound_core::galaxy::FactionService::Repair)
                }) && fp.services.iter().any(|s| {
                    matches!(s, starbound_core::galaxy::FactionService::Trade)
                }),
                "religious" => fp.services.iter().any(|s| {
                    matches!(s, starbound_core::galaxy::FactionService::Shelter)
                }),
                "criminal" => fp.services.iter().any(|s| {
                    matches!(s, starbound_core::galaxy::FactionService::Smuggling)
                }),
                _ => false,
            }
        });

        if matching_presence.is_none() {
            return false;
        }
    }

    // Time factor minimum.
    if let Some(min_factor) = req.time_factor_min {
        if ctx.system.time_factor < min_factor {
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
    // Faction presence requirements are highly specific.
    if req.faction_category_present.is_some() { score += 2; }
    if req.faction_min_strength.is_some() { score += 1; }
    if req.faction_max_visibility.is_some() { score += 1; }
    if req.time_factor_min.is_some() { score += 2; }
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
            controlling_civ: faction,
            infrastructure_level: infra,
            history: vec![],
            active_threads: vec![],
            time_factor: 1.0,
            faction_presence: vec![],
        }
    }

    fn test_system_with_faction_presence(
        infra: InfrastructureLevel,
        civ: Option<Uuid>,
        presences: Vec<FactionPresence>,
    ) -> StarSystem {
        StarSystem {
            faction_presence: presences,
            ..test_system(infra, civ)
        }
    }

    fn test_journey(fuel: f32, hull: f32, crew_count: usize) -> Journey {
        use starbound_core::crew::*;

        let crew: Vec<CrewMember> = (0..crew_count)
            .map(|i| CrewMember {
                id: Uuid::new_v4(),
                name: format!("Crew {}", i),
                role: CrewRole::Engineer,
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
                background: "Test crew member.".into(),
                state: CrewState {
                    mood: Mood::Determined,
                    stress: 0.3,
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
                    engine: Module::standard("Test Engine"),
                    sensors: Module::standard("Test Sensors"),
                    comms: Module::standard("Test Comms"),
                    weapons: Module::standard("Test Weapons"),
                    life_support: Module::standard("Test Life Support"),
                },
            },
            current_system: Uuid::new_v4(),
            time: Timestamp::zero(),
            resources: 1000.0,
            mission: MissionState {
                mission_type: MissionType::Search,
                core_truth: "Test mission.".into(),
                knowledge_nodes: vec![],
            },
            crew,
            threads: vec![],
            event_log: vec![],
            civ_standings: HashMap::new(),
        }
    }

    // -------------------------------------------------------------------
    // Existing tests (unchanged)
    // -------------------------------------------------------------------

    #[test]
    fn colony_with_faction_matches_expected() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"faction_checkpoint"),
            "Colony with faction should match faction_checkpoint, got: {:?}", ids);
    }

    #[test]
    fn empty_space_matches_deep_space() {
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
            galactic_years_since_last_visit: Some(80.0),
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"colony_generations"),
            "Returning after 80 years should match colony_generations, got: {:?}", ids);
    }

    #[test]
    fn specificity_ordering() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(20.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: Some(60.0),
        };

        let matched = match_events(&events, &ctx);
        assert!(matched.len() >= 3, "Should match multiple events");

        if matched.len() >= 2 {
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
        let journey = test_journey(80.0, 0.9, 0);
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

    // -------------------------------------------------------------------
    // Faction presence matching tests (Phase C)
    // -------------------------------------------------------------------

    #[test]
    fn faction_category_present_matches_when_present() {
        let events = all_seed_events();
        let civ_id = Uuid::new_v4();
        let system = test_system_with_faction_presence(
            InfrastructureLevel::Hub,
            Some(civ_id),
            vec![FactionPresence {
                faction_id: Uuid::new_v4(),
                strength: 0.6,
                visibility: 0.8,
                services: vec![FactionService::Trade, FactionService::Missions],
            }],
        );
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"guild_price_war"),
            "Hub with economic presence should match guild_price_war, got: {:?}", ids);
    }

    #[test]
    fn faction_category_present_excludes_when_absent() {
        let events = all_seed_events();
        // System with NO faction presence at all.
        let system = test_system(InfrastructureLevel::Hub, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        // No faction events should match.
        assert!(!ids.contains(&"guild_price_war"),
            "No faction presence should not match guild_price_war");
        assert!(!ids.contains(&"lattice_dead_drop"),
            "No faction presence should not match lattice_dead_drop");
    }

    #[test]
    fn criminal_faction_matches_smuggling_events() {
        let events = all_seed_events();
        let system = test_system_with_faction_presence(
            InfrastructureLevel::Colony,
            Some(Uuid::new_v4()),
            vec![FactionPresence {
                faction_id: Uuid::new_v4(),
                strength: 0.3,
                visibility: 0.1,
                services: vec![FactionService::Intelligence, FactionService::Smuggling],
            }],
        );
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"lattice_dead_drop"),
            "Criminal presence should match lattice_dead_drop, got: {:?}", ids);
    }

    #[test]
    fn time_factor_min_gates_correctly() {
        let events = all_seed_events();

        // Normal system — should NOT match time-gated events.
        let normal = test_system_with_faction_presence(
            InfrastructureLevel::Colony,
            Some(Uuid::new_v4()),
            vec![FactionPresence {
                faction_id: Uuid::new_v4(),
                strength: 0.5,
                visibility: 0.6,
                services: vec![FactionService::Shelter, FactionService::Intelligence],
            }],
        );
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = MatchContext {
            system: &normal,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(!ids.contains(&"quiet_star_vigil"),
            "Normal-time system should not match quiet_star_vigil");

        // Distorted system — should match.
        let mut distorted = test_system_with_faction_presence(
            InfrastructureLevel::Colony,
            None,
            vec![FactionPresence {
                faction_id: Uuid::new_v4(),
                strength: 0.5,
                visibility: 0.6,
                services: vec![FactionService::Shelter, FactionService::Intelligence],
            }],
        );
        distorted.time_factor = 2.5;
        let ctx2 = MatchContext {
            system: &distorted,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched2 = match_events(&events, &ctx2);
        let ids2: Vec<&str> = matched2.iter().map(|e| e.id.as_str()).collect();
        assert!(ids2.contains(&"quiet_star_vigil"),
            "Distorted system with religious presence should match quiet_star_vigil, got: {:?}", ids2);
    }

    #[test]
    fn strength_gate_excludes_weak_presence() {
        let events = all_seed_events();
        // Military presence too weak to trigger military_inspection.
        let system = test_system_with_faction_presence(
            InfrastructureLevel::Colony,
            Some(Uuid::new_v4()),
            vec![FactionPresence {
                faction_id: Uuid::new_v4(),
                strength: 0.1, // Below the 0.5 gate
                visibility: 0.9,
                services: vec![FactionService::Intelligence, FactionService::Missions],
            }],
        );
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(!ids.contains(&"military_inspection"),
            "Weak military presence should not trigger military_inspection");
    }

    #[test]
    fn faction_events_have_higher_specificity_than_generic() {
        let events = all_seed_events();
        let system = test_system_with_faction_presence(
            InfrastructureLevel::Hub,
            Some(Uuid::new_v4()),
            vec![
                FactionPresence {
                    faction_id: Uuid::new_v4(),
                    strength: 0.7,
                    visibility: 0.9,
                    services: vec![FactionService::Trade, FactionService::Missions],
                },
                FactionPresence {
                    faction_id: Uuid::new_v4(),
                    strength: 0.3,
                    visibility: 0.1,
                    services: vec![FactionService::Intelligence, FactionService::Smuggling],
                },
            ],
        );
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = MatchContext {
            system: &system,
            journey: &journey,
            galactic_years_since_last_visit: None,
        };

        let matched = match_events(&events, &ctx);
        if matched.len() >= 2 {
            // Faction-specific events should rank above generic events.
            let faction_event_pos = matched.iter()
                .position(|e| e.id == "guild_price_war" || e.id == "lattice_dead_drop");
            let generic_event_pos = matched.iter()
                .position(|e| e.context_requirements.faction_category_present.is_none()
                    && e.context_requirements.time_factor_min.is_none());

            if let (Some(faction_pos), Some(generic_pos)) = (faction_event_pos, generic_event_pos) {
                assert!(faction_pos < generic_pos,
                    "Faction events should rank above generic events in specificity");
            }
        }
    }
}