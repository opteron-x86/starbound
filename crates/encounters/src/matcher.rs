// file: crates/encounters/src/matcher.rs
//! Event matching — filters the seed library against current game state.
//!
//! ## Location-aware infrastructure
//!
//! Infrastructure checks now use the **location's** infrastructure level
//! when available, falling back to the system level only when the player
//! is at the system edge (no specific location). This prevents station
//! events from firing at barren planets in developed systems.

use starbound_core::galaxy::{InfrastructureLevel, StarSystem};
use starbound_core::journey::Journey;
use starbound_core::narrative::ResolutionState;

use super::seed_event::{ContextRequirements, Prerequisites, SeedEvent};

/// Game state distilled into what the matcher needs.
/// Keeps the matcher decoupled from the full game state.
pub struct MatchContext<'a> {
    pub system: &'a StarSystem,
    pub journey: &'a Journey,
    /// Galactic years since the player last visited this system (None if first visit).
    pub galactic_years_since_last_visit: Option<f64>,
    /// The type of location the player is currently at (None if at system edge).
    pub location_type: Option<String>,
    /// Infrastructure level at the player's current location.
    /// When `Some`, infrastructure checks use this instead of the system level.
    /// When `None` (system edge), falls back to system.infrastructure_level.
    pub location_infrastructure: Option<InfrastructureLevel>,
    /// Names of systems the player has previously visited.
    #[allow(dead_code)]
    pub visited_system_names: Vec<String>,
}

/// Filter seed events to those whose requirements match the current context.
/// Returns events sorted by specificity (most specific requirements first).
///
/// Prerequisites are checked as hard gates — events with unmet
/// prerequisites are excluded regardless of context match.
pub fn match_events<'a>(
    events: &'a [SeedEvent],
    ctx: &MatchContext,
) -> Vec<&'a SeedEvent> {
    let mut matched: Vec<(&SeedEvent, usize)> = events
        .iter()
        .filter(|e| prerequisites_met(&e.context_requirements, ctx))
        .filter(|e| requirements_met(&e.context_requirements, ctx))
        .map(|e| (e, specificity(&e.context_requirements)))
        .collect();

    // Sort by specificity descending — most specific events first.
    matched.sort_by(|a, b| b.1.cmp(&a.1));

    matched.into_iter().map(|(e, _)| e).collect()
}

// ---------------------------------------------------------------------------
// Prerequisite checking — hard gates
// ---------------------------------------------------------------------------

fn prerequisites_met(req: &ContextRequirements, ctx: &MatchContext) -> bool {
    let prereqs = match &req.prerequisites {
        Some(p) => p,
        None => return true,
    };
    check_prerequisites(prereqs, ctx)
}

fn check_prerequisites(prereqs: &Prerequisites, ctx: &MatchContext) -> bool {
    if let Some(ref req) = prereqs.threads_with_type {
        let count = ctx
            .journey
            .threads
            .iter()
            .filter(|t| {
                (t.resolution == ResolutionState::Open
                    || t.resolution == ResolutionState::Partial)
                    && format!("{}", t.thread_type).to_lowercase() == req.thread_type.to_lowercase()
            })
            .count();
        if count < req.min_count {
            return false;
        }
    }

    if let Some(ref req) = prereqs.threads_with_tag {
        let tag_lower = req.tag.to_lowercase();
        let count = ctx
            .journey
            .threads
            .iter()
            .filter(|t| {
                (t.resolution == ResolutionState::Open
                    || t.resolution == ResolutionState::Partial)
                    && t.description.to_lowercase().contains(&tag_lower)
            })
            .count();
        if count < req.min_count {
            return false;
        }
    }

    if let Some(ref desc_substr) = prereqs.thread_active {
        let substr_lower = desc_substr.to_lowercase();
        let found = ctx.journey.threads.iter().any(|t| {
            (t.resolution == ResolutionState::Open
                || t.resolution == ResolutionState::Partial)
                && t.description.to_lowercase().contains(&substr_lower)
        });
        if !found {
            return false;
        }
    }

    if let Some(ref item) = prereqs.cargo_contains {
        let item_lower = item.to_lowercase();
        let found = ctx
            .journey
            .ship
            .cargo
            .keys()
            .any(|k| k.to_lowercase().contains(&item_lower));
        if !found {
            return false;
        }
    }

    if let Some(ref system_name) = prereqs.has_visited_system {
        let name_lower = system_name.to_lowercase();
        let found = ctx
            .visited_system_names
            .iter()
            .any(|n| n.to_lowercase().contains(&name_lower));
        if !found {
            return false;
        }
    }

    if let Some(true) = prereqs.contract_active {
        if ctx.journey.active_contracts.is_empty() {
            return false;
        }
    }

    if let Some(ref _faction_req) = prereqs.faction_standing_min {
        // TODO: Implement when faction standing is tracked per-category.
    }

    true
}

// ---------------------------------------------------------------------------
// Context requirement checking
// ---------------------------------------------------------------------------

/// Check whether all requirements of an event are satisfied.
///
/// Infrastructure checks use `location_infrastructure` when available,
/// falling back to `system.infrastructure_level` when the player is at
/// the system edge (no specific location).
fn requirements_met(req: &ContextRequirements, ctx: &MatchContext) -> bool {
    // Effective infrastructure: location-level when at a location,
    // system-level when at the system edge.
    let effective_infra = ctx
        .location_infrastructure
        .unwrap_or(ctx.system.infrastructure_level);

    // Infrastructure minimum.
    if let Some(ref min) = req.infrastructure_min {
        if let Some(min_level) = parse_infrastructure(min) {
            if infra_rank(effective_infra) < infra_rank(min_level) {
                return false;
            }
        }
    }

    // Infrastructure maximum.
    if let Some(ref max) = req.infrastructure_max {
        if let Some(max_level) = parse_infrastructure(max) {
            if infra_rank(effective_infra) > infra_rank(max_level) {
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
            None => {}
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

    // Faction category present.
    if let Some(ref category) = req.faction_category_present {
        let category_lower = category.to_lowercase();

        let matching_presence = ctx.system.faction_presence.iter().find(|fp| {
            let strength_ok = req
                .faction_min_strength
                .map(|min| fp.strength >= min)
                .unwrap_or(true);
            let vis_ok = req
                .faction_max_visibility
                .map(|max| fp.visibility <= max)
                .unwrap_or(true);

            if !strength_ok || !vis_ok {
                return false;
            }

            match category_lower.as_str() {
                "military" => {
                    fp.services.iter().any(|s| {
                        matches!(s, starbound_core::galaxy::FactionService::Intelligence)
                    }) && fp.services.iter().any(|s| {
                        matches!(s, starbound_core::galaxy::FactionService::Missions)
                    })
                }
                "economic" => fp
                    .services
                    .iter()
                    .any(|s| matches!(s, starbound_core::galaxy::FactionService::Trade)),
                "guild" => {
                    fp.services.iter().any(|s| {
                        matches!(s, starbound_core::galaxy::FactionService::Repair)
                    }) && fp.services.iter().any(|s| {
                        matches!(s, starbound_core::galaxy::FactionService::Trade)
                    })
                }
                "religious" => fp
                    .services
                    .iter()
                    .any(|s| matches!(s, starbound_core::galaxy::FactionService::Shelter)),
                "criminal" => fp
                    .services
                    .iter()
                    .any(|s| matches!(s, starbound_core::galaxy::FactionService::Smuggling)),
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

    // Location type filter.
    if !req.location_types.is_empty() {
        match &ctx.location_type {
            Some(loc_type) => {
                if !req.location_types.iter().any(|lt| lt == loc_type) {
                    return false;
                }
            }
            None => {
                return false;
            }
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Specificity scoring
// ---------------------------------------------------------------------------

fn specificity(req: &ContextRequirements) -> usize {
    let mut score = 0;
    if req.infrastructure_min.is_some() { score += 1; }
    if req.infrastructure_max.is_some() { score += 1; }
    if req.faction_controlled.is_some() { score += 1; }
    if req.unclaimed.is_some() { score += 1; }
    if req.time_since_last_visit_galactic_years_min.is_some() { score += 2; }
    if req.fuel_below_fraction.is_some() { score += 2; }
    if req.hull_below.is_some() { score += 2; }
    if req.crew_min.is_some() { score += 1; }
    if !req.tags.is_empty() { score += 1; }
    if req.faction_category_present.is_some() { score += 2; }
    if req.faction_min_strength.is_some() { score += 1; }
    if req.faction_max_visibility.is_some() { score += 1; }
    if req.time_factor_min.is_some() { score += 2; }
    if !req.location_types.is_empty() { score += 2; }
    if req.prerequisites.is_some() { score += 3; }
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
    use starbound_core::crew::*;
    use starbound_core::galaxy::*;
    use starbound_core::mission::*;
    use starbound_core::narrative::*;
    use starbound_core::reputation::PlayerProfile;
    use starbound_core::ship::*;
    use starbound_core::time::Timestamp;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn test_system(infra: InfrastructureLevel, faction: Option<Uuid>) -> StarSystem {
        StarSystem {
            id: Uuid::new_v4(),
            name: "Test System".into(),
            position: (0.0, 0.0),
            star_type: StarType::YellowDwarf,
            locations: vec![],
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
        let crew: Vec<CrewMember> = (0..crew_count)
            .map(|i| CrewMember {
                id: Uuid::new_v4(),
                name: format!("Crew {}", i),
                role: CrewRole::Engineer,
                drives: PersonalityDrives {
                    security: 0.5, freedom: 0.5, purpose: 0.5,
                    connection: 0.5, knowledge: 0.5, justice: 0.5,
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
                supplies: 80.0,
                supply_capacity: 100.0,
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
            profile: PlayerProfile::new(),
            active_contracts: vec![],
            discovered_rumors: vec![],
            current_location: None,
        }
    }

    /// Build a MatchContext for tests.
    fn make_ctx<'a>(
        system: &'a StarSystem,
        journey: &'a Journey,
        galactic_years: Option<f64>,
        location_type: Option<String>,
    ) -> MatchContext<'a> {
        MatchContext {
            system,
            journey,
            galactic_years_since_last_visit: galactic_years,
            location_type,
            location_infrastructure: None, // Tests use system-level by default
            visited_system_names: Vec::new(),
        }
    }

    /// Build a MatchContext with explicit location infrastructure.
    fn make_ctx_with_infra<'a>(
        system: &'a StarSystem,
        journey: &'a Journey,
        location_type: Option<String>,
        location_infra: Option<InfrastructureLevel>,
    ) -> MatchContext<'a> {
        MatchContext {
            system,
            journey,
            galactic_years_since_last_visit: None,
            location_type,
            location_infrastructure: location_infra,
            visited_system_names: Vec::new(),
        }
    }

    // -------------------------------------------------------------------
    // Content tests (against minimal event set)
    // -------------------------------------------------------------------

    #[test]
    fn station_arrival_matches_at_colony() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = make_ctx_with_infra(
            &system, &journey,
            Some("station".into()),
            Some(InfrastructureLevel::Colony),
        );

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"arrival_station_routine"),
            "Colony station should match arrival_station_routine, got: {:?}", ids);
    }

    #[test]
    fn empty_space_matches_deep_space() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::None, None);
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = make_ctx(&system, &journey, None, Some("deep_space".into()));

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"arrival_empty_space"),
            "Unclaimed deep space should match arrival_empty_space, got: {:?}", ids);
    }

    #[test]
    fn no_crew_excludes_crew_events() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 0);
        let ctx = make_ctx(&system, &journey, None, Some("deep_space".into()));

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(!ids.contains(&"crew_quiet_moment"),
            "No crew should not match crew_quiet_moment");
    }

    #[test]
    fn crew_event_matches_with_crew() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = make_ctx(&system, &journey, None, Some("deep_space".into()));

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"crew_quiet_moment"),
            "Deep space with crew should match crew_quiet_moment, got: {:?}", ids);
    }

    #[test]
    fn investigate_events_match_correct_locations() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::None, None);
        let journey = test_journey(80.0, 0.9, 3);

        // Deep space should match the anomaly investigation
        let ctx_deep = make_ctx(&system, &journey, None, Some("deep_space".into()));
        let matched = match_events(&events, &ctx_deep);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"investigate_anomaly_deep_space"),
            "Deep space should match investigate_anomaly_deep_space, got: {:?}", ids);

        // Planet surface should match the ruins investigation
        let ctx_planet = make_ctx(&system, &journey, None, Some("planet_surface".into()));
        let matched = match_events(&events, &ctx_planet);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"investigate_surface_ruins"),
            "Planet surface should match investigate_surface_ruins, got: {:?}", ids);
    }

    #[test]
    fn infrastructure_max_blocks_developed_locations() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Hub, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = make_ctx_with_infra(
            &system, &journey,
            Some("deep_space".into()),
            Some(InfrastructureLevel::Hub),
        );

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(!ids.contains(&"arrival_empty_space"),
            "Hub-level location should NOT match arrival_empty_space, got: {:?}", ids);
    }

    #[test]
    fn faction_checkpoint_requires_military() {
        let events = all_seed_events();
        let system_with_military = test_system_with_faction_presence(
            InfrastructureLevel::Colony, Some(Uuid::new_v4()),
            vec![FactionPresence {
                faction_id: Uuid::new_v4(),
                strength: 0.6, visibility: 0.8,
                services: vec![FactionService::Missions, FactionService::Intelligence],
            }],
        );
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = make_ctx_with_infra(
            &system_with_military, &journey,
            Some("station".into()),
            Some(InfrastructureLevel::Colony),
        );

        let matched = match_events(&events, &ctx);
        let ids: Vec<&str> = matched.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"faction_checkpoint"),
            "Station with military presence should match faction_checkpoint, got: {:?}", ids);

        // Without military — should NOT match
        let system_none = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let ctx2 = make_ctx_with_infra(
            &system_none, &journey,
            Some("station".into()),
            Some(InfrastructureLevel::Colony),
        );
        let matched2 = match_events(&events, &ctx2);
        let ids2: Vec<&str> = matched2.iter().map(|e| e.id.as_str()).collect();
        assert!(!ids2.contains(&"faction_checkpoint"),
            "Station without military should NOT match faction_checkpoint, got: {:?}", ids2);
    }

    #[test]
    fn specificity_ordering() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = make_ctx_with_infra(
            &system, &journey,
            Some("station".into()),
            Some(InfrastructureLevel::Colony),
        );

        let matched = match_events(&events, &ctx);
        if matched.len() >= 2 {
            let first_spec = specificity(&matched[0].context_requirements);
            let last_spec = specificity(&matched[matched.len() - 1].context_requirements);
            assert!(first_spec >= last_spec,
                "Events should be ordered by specificity: first={}, last={}", first_spec, last_spec);
        }
    }

    // -------------------------------------------------------------------
    // Prerequisite tests
    // -------------------------------------------------------------------

    #[test]
    fn prerequisite_threads_blocks_when_insufficient() {
        use super::super::seed_event::*;
        let event = SeedEvent {
            id: "test_gated".into(),
            encounter_type: "contextual".into(),
            tone: "wonder".into(),
            category: "main_quest".into(),
            priority: 3,
            context_requirements: ContextRequirements {
                prerequisites: Some(Prerequisites {
                    threads_with_type: Some(ThreadCountReq {
                        thread_type: "anomaly".into(),
                        min_count: 2,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            text: "Test event.".repeat(20),
            choices: vec![SeedChoice {
                label: "OK".into(),
                effects: vec![EffectDef::Pass {}],
                tone_note: String::new(),
                follows: None,
            }],
            intents: vec![],
            trigger: EventTrigger::default(),
            event_kind: EventKind::default(),
        };
        let events = vec![event];
        let system = test_system(InfrastructureLevel::Colony, None);
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = make_ctx(&system, &journey, None, None);
        assert!(match_events(&events, &ctx).is_empty());
    }

    #[test]
    fn prerequisite_threads_passes_when_sufficient() {
        use super::super::seed_event::*;
        let event = SeedEvent {
            id: "test_gated".into(),
            encounter_type: "contextual".into(),
            tone: "wonder".into(),
            category: "main_quest".into(),
            priority: 3,
            context_requirements: ContextRequirements {
                prerequisites: Some(Prerequisites {
                    threads_with_type: Some(ThreadCountReq {
                        thread_type: "anomaly".into(),
                        min_count: 2,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            text: "Test event.".repeat(20),
            choices: vec![SeedChoice {
                label: "OK".into(),
                effects: vec![EffectDef::Pass {}],
                tone_note: String::new(),
                follows: None,
            }],
            intents: vec![],
            trigger: EventTrigger::default(),
            event_kind: EventKind::default(),
        };
        let events = vec![event];
        let system = test_system(InfrastructureLevel::Colony, None);
        let mut journey = test_journey(80.0, 0.9, 3);
        for i in 0..2 {
            journey.threads.push(Thread {
                id: Uuid::new_v4(),
                thread_type: ThreadType::Anomaly,
                associated_entities: vec![],
                tension: 0.5,
                created_at: Timestamp::zero(),
                last_touched: Timestamp::zero(),
                resolution: ResolutionState::Open,
                description: format!("Anomaly thread {}", i),
            });
        }
        let ctx = make_ctx(&system, &journey, None, None);
        assert_eq!(match_events(&events, &ctx).len(), 1);
    }

    #[test]
    fn prerequisite_cargo_blocks_when_missing() {
        use super::super::seed_event::*;
        let event = SeedEvent {
            id: "cargo_gated".into(),
            encounter_type: "contextual".into(),
            tone: "tense".into(),
            category: "side_quest".into(),
            priority: 2,
            context_requirements: ContextRequirements {
                prerequisites: Some(Prerequisites {
                    cargo_contains: Some("Xenoarchaeological samples".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            text: "Test cargo event.".repeat(20),
            choices: vec![SeedChoice {
                label: "OK".into(),
                effects: vec![EffectDef::Pass {}],
                tone_note: String::new(),
                follows: None,
            }],
            intents: vec![],
            trigger: EventTrigger::default(),
            event_kind: EventKind::default(),
        };
        let events = vec![event];
        let system = test_system(InfrastructureLevel::Colony, None);
        let journey = test_journey(80.0, 0.9, 3);
        let ctx = make_ctx(&system, &journey, None, None);
        assert!(match_events(&events, &ctx).is_empty());
    }

    #[test]
    fn prerequisite_cargo_passes_when_present() {
        use super::super::seed_event::*;
        let event = SeedEvent {
            id: "cargo_gated".into(),
            encounter_type: "contextual".into(),
            tone: "tense".into(),
            category: "side_quest".into(),
            priority: 2,
            context_requirements: ContextRequirements {
                prerequisites: Some(Prerequisites {
                    cargo_contains: Some("Xenoarchaeological samples".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            text: "Test cargo event.".repeat(20),
            choices: vec![SeedChoice {
                label: "OK".into(),
                effects: vec![EffectDef::Pass {}],
                tone_note: String::new(),
                follows: None,
            }],
            intents: vec![],
            trigger: EventTrigger::default(),
            event_kind: EventKind::default(),
        };
        let events = vec![event];
        let system = test_system(InfrastructureLevel::Colony, None);
        let mut journey = test_journey(80.0, 0.9, 3);
        journey.ship.cargo.insert("Xenoarchaeological samples".into(), 1);
        let ctx = make_ctx(&system, &journey, None, None);
        assert_eq!(match_events(&events, &ctx).len(), 1);
    }
}