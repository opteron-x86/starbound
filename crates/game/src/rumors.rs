// file: crates/game/src/rumors.rs
//! Rumor generation — assembles actionable information from live game state.
//!
//! Rumors are not pre-authored content. They are assembled at runtime from
//! game state when the player selects "Gather Rumors" at a location with
//! the Rumors service.
//!
//! Three scanners produce candidates:
//!   - Trade scanner: price differentials across known economies
//!   - Faction scanner: recent galactic tick events
//!   - Thread scanner: unresolved narrative threads and potential seeds
//!
//! Candidates are scored, deduplicated by category, and the top 2-4 are
//! selected based on infrastructure level.

use rand::prelude::*;
use uuid::Uuid;

use starbound_core::galaxy::{
    InfrastructureLevel, Location, StarSystem, TradeGood,
};
use starbound_core::journey::Journey;
use starbound_core::narrative::{ResolutionState, Thread, ThreadType};
use starbound_core::rumor::{
    base_reliability, rumor_count_range, Rumor, RumorCategory, RumorContent,
};

use starbound_simulation::generate::GeneratedGalaxy;
use starbound_simulation::tick::TickEvent;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Context needed to generate rumors at the player's current location.
pub struct RumorContext<'a> {
    pub galaxy: &'a GeneratedGalaxy,
    pub journey: &'a Journey,
    /// Recent galactic tick events (kept in CLI GameState).
    pub recent_tick_events: &'a [TickEvent],
    /// Current location — must have the Rumors service.
    pub location: &'a Location,
    /// The system the player is in.
    pub system: &'a StarSystem,
}

/// Generate rumors at the player's current location.
///
/// Returns 1-4 rumors selected from all scanner candidates,
/// scored by relevance and variety-balanced across categories.
pub fn generate_rumors(ctx: &RumorContext, rng: &mut StdRng) -> Vec<Rumor> {
    let infra_label = ctx.location.infrastructure.to_string();
    let (min_count, max_count) = rumor_count_range(&infra_label);
    let target_count = rng.gen_range(min_count..=max_count);
    let reliability = base_reliability(&infra_label);

    // Gather candidates from all scanners.
    let mut candidates: Vec<ScoredCandidate> = Vec::new();

    candidates.extend(scan_trade(ctx, reliability));
    candidates.extend(scan_factions(ctx, reliability));
    candidates.extend(scan_threads(ctx, reliability));
    candidates.extend(scan_local_color(ctx, reliability));

    if candidates.is_empty() {
        return vec![];
    }

    // Sort by score (descending).
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    // Select with variety: at most one per category.
    let mut selected: Vec<ScoredCandidate> = Vec::new();
    let mut used_categories: Vec<RumorCategory> = Vec::new();
    let mut selected_ids: Vec<Uuid> = Vec::new();

    // First pass: one per category (highest-scoring).
    for candidate in &candidates {
        if selected.len() >= target_count {
            break;
        }
        if used_categories.contains(&candidate.category) {
            continue;
        }
        used_categories.push(candidate.category);
        selected_ids.push(candidate.id);
        selected.push(candidate.clone());
    }

    // Second pass: fill remaining slots with duplicates if needed.
    if selected.len() < target_count {
        for candidate in &candidates {
            if selected.len() >= target_count {
                break;
            }
            if selected_ids.contains(&candidate.id) {
                continue;
            }
            selected_ids.push(candidate.id);
            selected.push(candidate.clone());
        }
    }

    // Convert to Rumor structs.
    selected
        .into_iter()
        .map(|c| c.into_rumor(ctx))
        .collect()
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// A candidate rumor with a relevance score for selection.
#[derive(Clone)]
struct ScoredCandidate {
    id: Uuid,
    category: RumorCategory,
    content: RumorContent,
    display_text: String,
    summary: String,
    score: f64,
    reliability: f64,
    expires_in: f64,
}

impl ScoredCandidate {
    fn into_rumor(self, ctx: &RumorContext) -> Rumor {
        Rumor {
            id: self.id,
            category: self.category,
            content: self.content,
            source_system: ctx.system.id,
            source_location: ctx.location.id,
            generated_at: ctx.journey.time.galactic_days,
            expires_in: self.expires_in,
            reliability: self.reliability,
            acted_on: false,
            outcome: None,
            display_text: self.display_text,
            summary: self.summary,
        }
    }
}

// ---------------------------------------------------------------------------
// Trade scanner
// ---------------------------------------------------------------------------

/// Scan all known economies for profitable trade routes relative to the
/// player's current location.
fn scan_trade(ctx: &RumorContext, reliability: f64) -> Vec<ScoredCandidate> {
    let local_economy = match &ctx.location.economy {
        Some(e) => e,
        None => return vec![],
    };

    let mut candidates = Vec::new();

    for good in TradeGood::all() {
        let local_buy = local_economy.buy_price(*good);

        // Check every location in every system for sell opportunities.
        for system in &ctx.galaxy.systems {
            if system.id == ctx.system.id {
                continue; // Skip current system.
            }
            for loc in &system.locations {
                let other_economy = match &loc.economy {
                    Some(e) => e,
                    None => continue,
                };
                let sell_there = other_economy.sell_price(*good);
                let spread = sell_there - local_buy;

                // Only surface profitable spreads above a minimum threshold.
                if spread < 3.0 {
                    continue;
                }

                let display = format!(
                    "\"{}\" is selling for {:.0} credits at {} — you can buy it here for {:.0}. \
                     That's roughly {:.0} per unit profit.\"",
                    good.display_name(),
                    sell_there,
                    system.name,
                    local_buy,
                    spread,
                );

                let summary = format!(
                    "{}: buy here ~{:.0}, sell at {} ~{:.0} (+{:.0}/unit)",
                    good.display_name(),
                    local_buy,
                    system.name,
                    sell_there,
                    spread,
                );

                // Score: higher spread = more relevant. Normalize by base price.
                let score = spread / good.base_price();

                candidates.push(ScoredCandidate {
                    id: Uuid::new_v4(),
                    category: RumorCategory::TradeTip,
                    content: RumorContent::TradeTip {
                        good: good.display_name().to_string(),
                        buy_system: ctx.system.id,
                        buy_location: Some(ctx.location.id),
                        sell_system: system.id,
                        sell_location: Some(loc.id),
                        estimated_spread: spread,
                    },
                    display_text: display,
                    summary,
                    score,
                    reliability,
                    expires_in: RumorCategory::TradeTip.default_expiry(),
                });
            }
        }
    }

    // Keep only the top 3 trade tips (don't flood with trade data).
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(3);
    candidates
}

// ---------------------------------------------------------------------------
// Faction scanner
// ---------------------------------------------------------------------------

/// Read recent galactic tick events and surface political/military shifts
/// relevant to factions present at this location.
fn scan_factions(ctx: &RumorContext, reliability: f64) -> Vec<ScoredCandidate> {
    let mut candidates = Vec::new();

    // Factions present at this system.
    let local_faction_ids: Vec<Uuid> = ctx.system.faction_presence.iter()
        .map(|fp| fp.faction_id)
        .collect();

    for event in ctx.recent_tick_events {
        // Score events that involve factions present locally higher.
        let involves_local = event.entities.iter()
            .any(|eid| local_faction_ids.contains(eid));

        if !involves_local && ctx.recent_tick_events.len() > 5 {
            // For large event sets, skip events that don't touch local factions.
            continue;
        }

        // Resolve entity names for display.
        let faction_names: Vec<String> = event.entities.iter()
            .filter_map(|eid| {
                ctx.galaxy.factions.iter()
                    .find(|f| f.id == *eid)
                    .map(|f| f.name.clone())
                    .or_else(|| {
                        ctx.galaxy.civilizations.iter()
                            .find(|c| c.id == *eid)
                            .map(|c| c.name.clone())
                    })
            })
            .collect();

        let display = format!(
            "\"{}\"",
            event.description,
        );

        let summary = event.description.clone();

        let score = if involves_local { 0.8 } else { 0.4 };

        candidates.push(ScoredCandidate {
            id: Uuid::new_v4(),
            category: RumorCategory::FactionIntel,
            content: RumorContent::FactionIntel {
                summary: event.description.clone(),
                factions_involved: event.entities.clone(),
                implication: if involves_local {
                    format!(
                        "This directly affects {} presence at {}.",
                        faction_names.join(" and "),
                        ctx.system.name,
                    )
                } else {
                    "Distant events, but the ripples may reach here.".into()
                },
            },
            display_text: display,
            summary,
            score,
            reliability,
            expires_in: RumorCategory::FactionIntel.default_expiry(),
        });
    }

    // Keep top 3.
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(3);
    candidates
}

// ---------------------------------------------------------------------------
// Thread scanner
// ---------------------------------------------------------------------------

/// Read the player's thread ledger and surface hints for unresolved threads,
/// plus potential new thread seeds from galactic state.
fn scan_threads(ctx: &RumorContext, reliability: f64) -> Vec<ScoredCandidate> {
    let mut candidates = Vec::new();

    // Existing open threads with high tension.
    for thread in &ctx.journey.threads {
        if thread.resolution != ResolutionState::Open {
            continue;
        }
        if thread.tension < 0.3 {
            continue;
        }

        // Check if the thread connects to this system.
        let connects_here = thread.associated_entities.contains(&ctx.system.id);

        let display = format!(
            "\"People are still talking about {}. {}\"",
            thread.description,
            if connects_here {
                "It happened right here."
            } else {
                "Word travels."
            },
        );

        let summary = format!("Thread: {} (tension {:.0}%)", thread.description, thread.tension * 100.0);

        let score = thread.tension as f64 * if connects_here { 1.5 } else { 0.8 };

        candidates.push(ScoredCandidate {
            id: Uuid::new_v4(),
            category: RumorCategory::ThreadSeed,
            content: RumorContent::ThreadSeed {
                description: thread.description.clone(),
                related_system: if connects_here { Some(ctx.system.id) } else { None },
                thread_type: format!("{}", thread.thread_type),
            },
            display_text: display,
            summary,
            score,
            reliability: 1.0, // Threads are facts about the player's own story.
            expires_in: RumorCategory::ThreadSeed.default_expiry(),
        });
    }

    // Generate a potential new thread seed from galactic state.
    // Look for systems with high faction tension nearby.
    for system in &ctx.galaxy.systems {
        if system.id == ctx.system.id {
            continue;
        }
        if system.faction_presence.len() < 2 {
            continue;
        }

        // Check for contested systems (multiple factions with significant strength).
        let strong_factions: Vec<&Uuid> = system.faction_presence.iter()
            .filter(|fp| fp.strength >= 0.3)
            .map(|fp| &fp.faction_id)
            .collect();

        if strong_factions.len() >= 2 {
            let faction_names: Vec<String> = strong_factions.iter()
                .filter_map(|fid| {
                    ctx.galaxy.factions.iter()
                        .find(|f| f.id == **fid)
                        .map(|f| f.name.clone())
                })
                .collect();

            // Only emit if the player doesn't already have a thread about this.
            let already_tracked = ctx.journey.threads.iter().any(|t| {
                t.associated_entities.contains(&system.id)
                    && t.resolution == ResolutionState::Open
            });
            if already_tracked {
                continue;
            }

            let display = format!(
                "\"Things are tense at {}. {} are both vying for influence there.\"",
                system.name,
                faction_names.join(" and "),
            );

            let summary = format!(
                "Contested: {} ({} competing factions)",
                system.name,
                strong_factions.len(),
            );

            candidates.push(ScoredCandidate {
                id: Uuid::new_v4(),
                category: RumorCategory::ThreadSeed,
                content: RumorContent::ThreadSeed {
                    description: format!(
                        "Power struggle at {} between {}",
                        system.name,
                        faction_names.join(" and "),
                    ),
                    related_system: Some(system.id),
                    thread_type: format!("{}", ThreadType::Mystery),
                },
                display_text: display,
                summary,
                score: 0.6,
                reliability,
                expires_in: RumorCategory::ThreadSeed.default_expiry(),
            });
        }
    }

    candidates.truncate(3);
    candidates
}

// ---------------------------------------------------------------------------
// Local color scanner
// ---------------------------------------------------------------------------

/// Generate atmospheric details from the current system's state.
/// These have no mechanical payload — pure world texture.
fn scan_local_color(ctx: &RumorContext, reliability: f64) -> Vec<ScoredCandidate> {
    let mut candidates = Vec::new();

    // Time distortion commentary for frontier/edge systems.
    if ctx.system.time_factor > 1.0 {
        let display = if ctx.system.time_factor >= 8.0 {
            format!(
                "\"Don't linger here. Clocks run ×{:.0} — a week at {} \
                 costs you months outside.\"",
                ctx.system.time_factor, ctx.system.name,
            )
        } else if ctx.system.time_factor >= 1.5 {
            format!(
                "\"Time runs a bit thick at {}. ×{:.1} — \
                 nothing dramatic, but it adds up.\"",
                ctx.system.name, ctx.system.time_factor,
            )
        } else {
            return candidates; // Not interesting enough.
        };

        let summary = format!(
            "{}: time factor ×{:.1}",
            ctx.system.name, ctx.system.time_factor,
        );

        candidates.push(ScoredCandidate {
            id: Uuid::new_v4(),
            category: RumorCategory::LocalColor,
            content: RumorContent::LocalColor {
                description: display.clone(),
            },
            display_text: display,
            summary,
            score: 0.3,
            reliability: 1.0, // Physical facts don't lie.
            expires_in: RumorCategory::LocalColor.default_expiry(),
        });
    }

    // Infrastructure commentary.
    if ctx.location.infrastructure <= InfrastructureLevel::Outpost {
        let display = format!(
            "\"Not much out here. {} is barely an outpost — \
             don't expect reliable information.\"",
            ctx.location.name,
        );
        let summary = format!("{}: low infrastructure", ctx.location.name);

        candidates.push(ScoredCandidate {
            id: Uuid::new_v4(),
            category: RumorCategory::LocalColor,
            content: RumorContent::LocalColor {
                description: display.clone(),
            },
            display_text: display,
            summary,
            score: 0.2,
            reliability: 1.0,
            expires_in: RumorCategory::LocalColor.default_expiry(),
        });
    }

    candidates
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use starbound_core::galaxy::*;
    use starbound_core::ship::{Module, Ship, ShipModules};
    use starbound_core::mission::{MissionState, MissionType};
    use starbound_core::reputation::PlayerProfile;
    use starbound_core::time::Timestamp;
    use std::collections::HashMap;

    fn test_economy(fuel_price: f32, food_prod: f32, food_cons: f32) -> SystemEconomy {
        let mut production = HashMap::new();
        let mut consumption = HashMap::new();
        production.insert(TradeGood::Food, food_prod);
        consumption.insert(TradeGood::Food, food_cons);
        production.insert(TradeGood::MedicalSupplies, 0.2);
        consumption.insert(TradeGood::MedicalSupplies, 0.8);
        SystemEconomy {
            production,
            consumption,
            price_volatility: 0.5,
            fuel_price,
            supply_price: 2.0,
        }
    }

    fn test_location(name: &str, infra: InfrastructureLevel, economy: Option<SystemEconomy>) -> Location {
        Location {
            id: Uuid::new_v4(),
            name: name.into(),
            location_type: LocationType::Station,
            orbital_distance: 1.0,
            infrastructure: infra,
            controlling_faction: None,
            economy,
            description: "A test location.".into(),
            services: vec![
                LocationService::Docking,
                LocationService::Trade,
                LocationService::Rumors,
            ],
            discovered: true,
        }
    }

    fn test_system(name: &str, locations: Vec<Location>) -> StarSystem {
        StarSystem {
            id: Uuid::new_v4(),
            name: name.into(),
            position: (0.0, 0.0),
            star_type: StarType::YellowDwarf,
            controlling_civ: None,
            infrastructure_level: InfrastructureLevel::Hub,
            history: vec![],
            active_threads: vec![],
            time_factor: 1.0,
            faction_presence: vec![],
            locations,
        }
    }

    fn test_journey(current_system: Uuid) -> Journey {
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
                    engine: Module::standard("Test Engine"),
                    sensors: Module::standard("Test Sensors"),
                    comms: Module::standard("Test Comms"),
                    weapons: Module::standard("Test Weapons"),
                    life_support: Module::standard("Test Life Support"),
                },
            },
            current_system,
            current_location: None,
            time: Timestamp::zero(),
            resources: 500.0,
            mission: MissionState {
                mission_type: MissionType::Search,
                core_truth: "Test".into(),
                knowledge_nodes: vec![],
            },
            crew: vec![],
            threads: vec![],
            event_log: vec![],
            civ_standings: HashMap::new(),
            profile: PlayerProfile::new(),
            active_contracts: vec![],
            discovered_rumors: vec![],
        }
    }

    fn minimal_galaxy(systems: Vec<StarSystem>) -> GeneratedGalaxy {
        GeneratedGalaxy {
            sector: Sector {
                id: Uuid::new_v4(),
                name: "Test Sector".into(),
                description: "A test sector.".into(),
                system_ids: systems.iter().map(|s| s.id).collect(),
            },
            start_system_id: systems[0].id,
            civilizations: vec![],
            factions: vec![],
            connections: vec![],
            npcs: vec![],
            systems,
        }
    }

    #[test]
    fn test_trade_scanner_finds_spread() {
        // System A: food is cheap to buy (high production, low consumption).
        let loc_a = test_location("Station A", InfrastructureLevel::Hub, Some(test_economy(3.0, 0.9, 0.1)));
        let sys_a = test_system("Alpha", vec![loc_a.clone()]);

        // System B: food is expensive (low production, high consumption).
        let loc_b = test_location("Station B", InfrastructureLevel::Colony, Some(test_economy(3.0, 0.1, 0.9)));
        let sys_b = test_system("Beta", vec![loc_b]);

        let galaxy = minimal_galaxy(vec![sys_a.clone(), sys_b]);
        let journey = test_journey(sys_a.id);

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc_a,
            system: &sys_a,
        };

        let candidates = scan_trade(&ctx, 0.8);
        assert!(!candidates.is_empty(), "Should find at least one trade tip");
        assert!(candidates[0].score > 0.0, "Top trade tip should have positive score");
        assert_eq!(candidates[0].category, RumorCategory::TradeTip);
    }

    #[test]
    fn test_faction_scanner_reads_events() {
        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let faction_id = Uuid::new_v4();

        let mut sys = test_system("Alpha", vec![loc.clone()]);
        sys.faction_presence.push(FactionPresence {
            faction_id,
            strength: 0.6,
            visibility: 0.8,
            services: vec![],
        });

        let galaxy = minimal_galaxy(vec![sys.clone()]);
        let journey = test_journey(sys.id);

        let events = vec![
            TickEvent {
                tick_number: 1,
                galactic_day: 365.0,
                description: "Tensions rose between factions.".into(),
                entities: vec![faction_id],
                category: starbound_simulation::tick::TickEventCategory::Military,
            },
        ];

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &events,
            location: &loc,
            system: &sys,
        };

        let candidates = scan_factions(&ctx, 0.8);
        assert!(!candidates.is_empty(), "Should find faction intel from tick events");
        assert_eq!(candidates[0].category, RumorCategory::FactionIntel);
    }

    #[test]
    fn test_local_color_time_distortion() {
        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let mut sys = test_system("Drift", vec![loc.clone()]);
        sys.time_factor = 2.0;

        let galaxy = minimal_galaxy(vec![sys.clone()]);
        let journey = test_journey(sys.id);

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };

        let candidates = scan_local_color(&ctx, 0.8);
        assert!(!candidates.is_empty(), "Should comment on time distortion");
        assert_eq!(candidates[0].category, RumorCategory::LocalColor);
    }

    #[test]
    fn test_generate_rumors_variety() {
        // Set up a galaxy where all scanners have candidates.
        let loc_a = test_location("Station A", InfrastructureLevel::Hub, Some(test_economy(3.0, 0.9, 0.1)));
        let mut sys_a = test_system("Alpha", vec![loc_a.clone()]);
        sys_a.time_factor = 2.0;

        let loc_b = test_location("Station B", InfrastructureLevel::Colony, Some(test_economy(3.0, 0.1, 0.9)));
        let sys_b = test_system("Beta", vec![loc_b]);

        let galaxy = minimal_galaxy(vec![sys_a.clone(), sys_b]);
        let journey = test_journey(sys_a.id);

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc_a,
            system: &sys_a,
        };

        let mut rng = StdRng::seed_from_u64(42);
        let rumors = generate_rumors(&ctx, &mut rng);

        assert!(!rumors.is_empty(), "Should generate at least one rumor");

        // Check that we have variety (not all the same category).
        let categories: Vec<RumorCategory> = rumors.iter().map(|r| r.category).collect();
        let unique: std::collections::HashSet<_> = categories.iter().collect();
        if rumors.len() > 1 {
            assert!(unique.len() > 1, "Multiple rumors should span multiple categories");
        }
    }
}
