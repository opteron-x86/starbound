// file: crates/cli/src/main.rs
//! Starbound — terminal prototype.
//!
//! The first playable version. A map, a ship, a crew, and the galaxy.
//! Travel between systems, encounter events, make choices, watch
//! time slip away.

use std::collections::HashMap;
use std::io::{self, Write};

use rand::rngs::StdRng;
use rand::Rng;
use rand::SeedableRng;
use uuid::Uuid;

use starbound_core::contract::{Contract, ContractState, ContractType};
use starbound_core::crew::*;
use starbound_core::galaxy::*;
use starbound_core::journey::Journey;
use starbound_core::mission::*;
use starbound_core::npc::{Npc, NpcRelationType, DispositionTier};
use starbound_core::ship::*;
use starbound_core::reputation::PlayerProfile;
use starbound_core::rumor::RumorContent;
use starbound_core::time::Timestamp;

use starbound_encounters::library::all_seed_events;
use starbound_encounters::pipeline::{
    run_pipeline, PipelineConfig, PipelineResult, PipelineState, PlayerIntent, EventTrigger,
};
use starbound_encounters::seed_event::{SeedEvent, EffectDef, FollowUpDelay};
use starbound_encounters::templates::{resolve_template, TemplateContext};

use starbound_simulation::generate::{generate_galaxy, GeneratedGalaxy};
use starbound_simulation::templates::load_people_templates;
use starbound_simulation::travel::{describe_plan, plan_all_routes, TravelPlan};
use starbound_simulation::tick::{tick_galaxy, TickResult, TickEvent, TickEventCategory};

use starbound_game::travel::execute_travel;
use starbound_game::consequences::{convert_effects, apply_effects};
use starbound_game::rumors::{generate_rumors, validate_rumors_at_location, RumorContext};
use starbound_game::crew_conversation::{
    generate_topics, conversation_effects_to_game_effects, apply_concern_removals,
    describe_crew_state, ConversationTopic, ConversationEffect,
};
use starbound_game::npc_interaction::{
    build_npc_presentation, ask_about_area, ask_about_connection,
    contract_refusal_text, farewell_text, NpcAction,
};

use starbound_llm::config::LlmConfig;
use starbound_llm::generate::generate_encounter;
use starbound_llm::rumor_flavor::{flavor_rumor, RumorSource};
use starbound_llm::prompt::DestinationInfo;

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

const DIVIDER: &str = "──────────────────────────────────────────────────────";
const THIN_DIVIDER: &str = "- - - - - - - - - - - - - - - - - - - - - - - - - - -";

fn prompt(msg: &str) -> String {
    print!("{}", msg);
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input.trim().to_string()
}

fn pause() {
    prompt("\n  [press enter to continue]");
}

fn clear_screen() {
    print!("\x1B[2J\x1B[1;1H");
    io::stdout().flush().unwrap();
}

fn display_header(title: &str) {
    println!("\n{}", DIVIDER);
    println!("  {}", title);
    println!("{}", DIVIDER);
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = String::new();

    for word in text.split_whitespace() {
        if current_line.is_empty() {
            current_line = word.to_string();
        } else if current_line.len() + 1 + word.len() > width {
            lines.push(current_line);
            current_line = word.to_string();
        } else {
            current_line.push(' ');
            current_line.push_str(word);
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    lines
}

// ---------------------------------------------------------------------------
// Game state
// ---------------------------------------------------------------------------

struct GameState {
    galaxy: GeneratedGalaxy,
    journey: Journey,
    events: Vec<SeedEvent>,
    pipeline_state: PipelineState,
    pipeline_config: PipelineConfig,
    rng: StdRng,
    /// Track last visit times per system (galactic_days).
    visit_log: HashMap<Uuid, f64>,
    /// Galactic day at which we last ran the tick engine.
    last_ticked_day: f64,
    /// Event IDs queued to fire on next arrival (from `FollowUpDelay::NextArrival`).
    pending_followups: Vec<String>,
    /// Topic IDs recently discussed in crew conversations (anti-repeat).
    discussed_topics: HashMap<Uuid, Vec<String>>,
    /// Knowledge items already shared by each NPC (NPC ID → shared items).
    /// Prevents NPCs from repeating themselves across visits.
    npc_shared_knowledge: HashMap<Uuid, Vec<String>>,
    /// Cached personality expressions from people.json (loaded once).
    personality_expressions: HashMap<String, Vec<String>>,
    /// LLM configuration. When enabled, encounter generation tries
    /// the LLM first and falls back to the seed library.
    llm_config: LlmConfig,
    /// Counter for generating unique LLM event IDs.
    llm_event_counter: u32,
    /// Recent scene summaries for LLM context continuity.
    /// Prevents re-introductions and contradictions. Capped at 5.
    scene_history: Vec<String>,
    /// Recent galactic tick events — used by the rumor system's faction scanner.
    /// Populated after each tick, capped at the most recent ~20 events.
    recent_tick_events: Vec<TickEvent>,
}

impl GameState {
    fn current_system(&self) -> &StarSystem {
        self.galaxy.systems.iter()
            .find(|s| s.id == self.journey.current_system)
            .expect("Player should be in a valid system")
    }

    fn system_name(&self, id: Uuid) -> &str {
        self.galaxy.systems.iter()
            .find(|s| s.id == id)
            .map(|s| s.name.as_str())
            .unwrap_or("Unknown")
    }

    fn civ_name(&self, id: Uuid) -> &str {
        self.galaxy.civilizations.iter()
            .find(|f| f.id == id)
            .map(|f| f.name.as_str())
            .unwrap_or("Independent")
    }

    fn faction_name(&self, id: Uuid) -> &str {
        self.galaxy.factions.iter()
            .find(|f| f.id == id)
            .map(|f| f.name.as_str())
            .unwrap_or("Unknown Faction")
    }
    
    fn connections_from_current(&self) -> Vec<Connection> {
        let id = self.journey.current_system;
        self.galaxy.connections.iter()
            .filter(|c| c.system_a == id || c.system_b == id)
            .cloned()
            .collect()
    }

    fn galactic_years_since_last_visit(&self) -> Option<f64> {
        self.visit_log.get(&self.journey.current_system)
            .map(|last_day| (self.journey.time.galactic_days - last_day) / 365.25)
    }

    fn record_visit(&mut self) {
        self.visit_log.insert(
            self.journey.current_system,
            self.journey.time.galactic_days,
        );
    }

    /// Get all living NPCs at the player's current location.
    /// If at system edge (no location), returns no NPCs.
    fn npcs_here(&self) -> Vec<&Npc> {
        let loc_id = match self.journey.current_location {
            Some(id) => id,
            None => return vec![], // At system edge — no NPCs in open space.
        };
        self.galaxy.npcs.iter()
            .filter(|n| {
                n.home_system_id == self.journey.current_system
                    && n.alive
                    && n.home_location_id == Some(loc_id)
            })
            .collect()
    }

    /// Get all living NPCs in the current system (any location).
    fn npcs_in_system(&self) -> Vec<&Npc> {
        self.galaxy.npcs.iter()
            .filter(|n| n.home_system_id == self.journey.current_system && n.alive)
            .collect()
    }

    /// Look up a location name by ID within the current system.
    fn location_name(&self, loc_id: Uuid) -> &str {
        self.current_system().locations.iter()
            .find(|l| l.id == loc_id)
            .map(|l| l.name.as_str())
            .unwrap_or("Unknown")
    }

    /// Get the current location (if docked).
    fn current_location(&self) -> Option<&Location> {
        self.journey.current_location.and_then(|loc_id| {
            self.current_system().locations.iter().find(|l| l.id == loc_id)
        })
    }

    /// Get the NPC's faction name, or "Independent".
    fn npc_faction_name(&self, npc: &Npc) -> String {
        npc.faction_id
            .and_then(|fid| self.galaxy.factions.iter().find(|f| f.id == fid))
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "Independent".into())
    }

    /// Get the system name where an NPC lives.
    fn npc_system_name(&self, npc: &Npc) -> &str {
        self.galaxy.systems.iter()
            .find(|s| s.id == npc.home_system_id)
            .map(|s| s.name.as_str())
            .unwrap_or("Unknown")
    }

    /// Get the location name where an NPC is posted.
    fn npc_location_name(&self, npc: &Npc) -> &str {
        npc.home_location_id
            .and_then(|loc_id| {
                self.galaxy.systems.iter()
                    .find(|s| s.id == npc.home_system_id)
                    .and_then(|s| s.locations.iter().find(|l| l.id == loc_id))
                    .map(|l| l.name.as_str())
            })
            .unwrap_or("Unknown")
    }

    /// Find an NPC by ID.
    fn find_npc(&self, npc_id: Uuid) -> Option<&Npc> {
        self.galaxy.npcs.iter().find(|n| n.id == npc_id)
    }

    /// Generate a unique ID for an LLM-generated event.
    fn next_llm_id(&mut self) -> String {
        self.llm_event_counter += 1;
        format!("llm_{:04}", self.llm_event_counter)
    }

    /// Record a scene summary for LLM context continuity.
    /// Keeps the last 5 scenes.
    fn record_scene(&mut self, summary: String) {
        self.scene_history.push(summary);
        if self.scene_history.len() > 5 {
            self.scene_history.remove(0);
        }
    }

    /// Build established facts about the current location for the LLM.
    /// These are non-negotiable truths the LLM must not contradict.
    fn build_established_facts(&self) -> Vec<String> {
        let mut facts = Vec::new();
        let system = self.current_system();

        // Location facts.
        if let Some(loc) = self.current_location() {
            facts.push(format!(
                "You are at {}, a {} in the {} system",
                loc.name, loc.location_type.category_str(), system.name,
            ));
            if !loc.description.is_empty() {
                facts.push(format!("Location description: {}", loc.description));
            }
            let services: Vec<String> = loc.services.iter().map(|s| format!("{}", s)).collect();
            if !services.is_empty() {
                facts.push(format!("Available services: {}", services.join(", ")));
            }
        } else {
            facts.push(format!("You are at the edge of the {} system, not docked", system.name));
        }

        // Civilization and faction facts.
        if let Some(civ_id) = system.controlling_civ {
            facts.push(format!("This system is controlled by {}", self.civ_name(civ_id)));
        } else {
            facts.push("This system is unclaimed — no controlling civilization".into());
        }

        // Faction presence — what IS and ISN'T here.
        let present_factions: Vec<String> = system.faction_presence.iter()
            .filter_map(|fp| {
                self.galaxy.factions.iter()
                    .find(|f| f.id == fp.faction_id)
                    .map(|f| format!("{} ({}, strength {:.0}%)", f.name, f.category, fp.strength * 100.0))
            })
            .collect();
        if !present_factions.is_empty() {
            facts.push(format!("Factions present: {}", present_factions.join("; ")));
        }

        // NPCs at this location — who the player can meet.
        let npcs = self.npcs_here();
        if npcs.is_empty() {
            facts.push("No notable NPCs at this location".into());
        } else {
            for npc in &npcs {
                let faction_str = self.npc_faction_name(npc);
                let mut npc_fact = format!(
                    "NPC here: {} — {} ({})", npc.name, npc.title, faction_str
                );
                if let Some(last) = npc.last_interaction() {
                    npc_fact.push_str(&format!(". Previously: {}", last.summary));
                }
                facts.push(npc_fact);
            }
        }

        // Star type fact — helps prevent astrophysics contradictions.
        facts.push(format!("Star type: {} (use this for any astronomical references)", system.star_type));

        // Active contracts — what the player is carrying and where it's going.
        for contract in &self.journey.active_contracts {
            if contract.state == ContractState::Active {
                let dest_sys = self.system_name(contract.destination_system_id);
                let dest_loc = contract.destination_location_id
                    .and_then(|loc_id| {
                        self.galaxy.systems.iter()
                            .find(|s| s.id == contract.destination_system_id)
                            .and_then(|s| s.locations.iter().find(|l| l.id == loc_id))
                            .map(|l| l.name.clone())
                    });
                let dest_str = match dest_loc {
                    Some(loc) => format!("{} in {} system", loc, dest_sys),
                    None => format!("{} system", dest_sys),
                };
                let mut fact = format!("Active contract: {} — destination: {}", contract.title, dest_str);
                if let Some((ref cargo, qty)) = contract.cargo_required {
                    fact.push_str(&format!(". Carrying {} x{} for this delivery", cargo, qty));
                }
                // Clarify if current location is NOT the destination.
                if contract.destination_system_id != self.journey.current_system {
                    fact.push_str(". Destination is in ANOTHER system, not here");
                } else if let Some(dest_loc_id) = contract.destination_location_id {
                    if self.journey.current_location != Some(dest_loc_id) {
                        fact.push_str(". Destination is at a different location in this system, not here");
                    }
                }
                facts.push(fact);
            }
        }

        // Cargo context — what's in the hold and why.
        if !self.journey.ship.cargo.is_empty() {
            let cargo_items: Vec<String> = self.journey.ship.cargo.iter()
                .map(|(name, qty)| format!("{} x{}", name, qty))
                .collect();
            facts.push(format!("Cargo hold: {}", cargo_items.join(", ")));
        }

        facts
    }

    /// Get the economy at the current location.
    fn current_economy(&self) -> Option<&SystemEconomy> {
        self.current_location().and_then(|loc| loc.economy.as_ref())
    }

    /// Get the primary dockable location (highest infrastructure with docking).
    fn primary_location(&self) -> Option<&Location> {
        let sys = self.current_system();
        sys.locations.iter()
            .filter(|l| l.services.contains(&LocationService::Docking))
            .max_by_key(|l| l.infrastructure)
    }

    /// Get the current location type as a string for encounter matching.
    fn current_location_type_str(&self) -> Option<String> {
        self.current_location().map(|l| l.location_type.category_str().to_string())
    }

    /// Find an NPC by ID (mutable access via index).
    fn npc_index(&self, npc_id: Uuid) -> Option<usize> {
        self.galaxy.npcs.iter().position(|n| n.id == npc_id)
    }

    fn template_context(&self) -> TemplateContext {
        let system = self.current_system();

        // Find the dominant faction at this system (highest strength).
        let faction_info = system.faction_presence.iter()
            .max_by(|a, b| a.strength.partial_cmp(&b.strength).unwrap())
            .and_then(|fp| {
                self.galaxy.factions.iter()
                    .find(|f| f.id == fp.faction_id)
                    .map(|f| (f.name.clone(), format!("{}", f.category)))
            });

        let civ_name = system.controlling_civ
            .and_then(|cid| self.galaxy.civilizations.iter()
                .find(|c| c.id == cid)
                .map(|c| c.name.clone()));

        let crew_name = if !self.journey.crew.is_empty() {
            Some(self.journey.crew[0].name.clone())
        } else {
            None
        };

        TemplateContext {
            system_name: system.name.clone(),
            system_description: format!("{} {} system", system.infrastructure_level, system.star_type),
            faction_name: faction_info.as_ref().map(|(n, _)| n.clone()),
            faction_category: faction_info.map(|(_, c)| c),
            civ_name,
            crew_random_name: crew_name,
            ship_name: self.journey.ship.name.clone(),
            personal_months: self.journey.time.personal_days / 30.44,
            galactic_years: self.journey.time.galactic_days / 365.25,
            custom: std::collections::HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Starting crew
// ---------------------------------------------------------------------------

fn create_starting_crew() -> Vec<CrewMember> {
    vec![
        CrewMember {
            id: Uuid::new_v4(),
            name: "Kael Vasquez".into(),
            role: CrewRole::Navigator,
            drives: PersonalityDrives {
                security: 0.3, freedom: 0.7, purpose: 0.5,
                connection: 0.4, knowledge: 0.8, justice: 0.3,
            },
            trust: Trust::starting_crew(),
            relationships: HashMap::new(),
            background: "Former cartographer for the Hegemony Survey Corps. \
                Resigned over a dispute about classified star charts. \
                Knows the Near Reach better than anyone alive.".into(),
            state: CrewState {
                mood: Mood::Determined,
                stress: 0.2,
                active_concerns: vec!["Plotting the most efficient route".into()],
            },
            origin: CrewOrigin::Starting,
        },
        CrewMember {
            id: Uuid::new_v4(),
            name: "Reva Okonkwo".into(),
            role: CrewRole::Engineer,
            drives: PersonalityDrives {
                security: 0.6, freedom: 0.3, purpose: 0.7,
                connection: 0.5, knowledge: 0.6, justice: 0.5,
            },
            trust: Trust::starting_crew(),
            relationships: HashMap::new(),
            background: "Third-generation spacer. Her grandmother built \
                colony hab modules; her mother maintained station reactors. \
                Reva keeps your ship running and takes it personally when \
                something breaks.".into(),
            state: CrewState {
                mood: Mood::Content,
                stress: 0.15,
                active_concerns: vec!["Engine calibration".into()],
            },
            origin: CrewOrigin::Starting,
        },
        CrewMember {
            id: Uuid::new_v4(),
            name: "Josen Tark".into(),
            role: CrewRole::Comms,
            drives: PersonalityDrives {
                security: 0.4, freedom: 0.5, purpose: 0.4,
                connection: 0.8, knowledge: 0.5, justice: 0.6,
            },
            trust: Trust::starting_crew(),
            relationships: HashMap::new(),
            background: "Linguist and signals specialist. Has a knack for \
                reading subtext in transmissions. Quiet, observant, \
                occasionally funny in ways that take a moment to land.".into(),
            state: CrewState {
                mood: Mood::Hopeful,
                stress: 0.1,
                active_concerns: vec!["Calibrating long-range comms".into()],
            },
            origin: CrewOrigin::Starting,
        },
    ]
}

// ---------------------------------------------------------------------------
// New game setup
// ---------------------------------------------------------------------------

fn new_game(seed: u64) -> GameState {
    let galaxy = generate_galaxy(seed);

    let start_system = galaxy.systems.iter()
        .find(|s| s.id == galaxy.start_system_id)
        .unwrap_or(&galaxy.systems[0]);

    let journey = Journey {
        ship: Ship {
            name: "Persistence".into(),
            hull_condition: 0.95,
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
        current_system: start_system.id,
        current_location: None,
        time: Timestamp::zero(),
        resources: 500.0,
        mission: MissionState {
            mission_type: MissionType::Search,
            core_truth: "A signal has been detected from beyond the edge of \
                mapped space. It encodes mathematical structures that predate \
                all known civilizations. Find its source.".into(),
            knowledge_nodes: vec![],
        },
        crew: create_starting_crew(),
        threads: vec![],
        event_log: vec![],
        civ_standings: HashMap::new(),
        profile: PlayerProfile::new(),
        active_contracts: vec![],
        discovered_rumors: vec![],
    };

    let mut visit_log = HashMap::new();
    visit_log.insert(start_system.id, 0.0);

    // Load personality expressions from people.json (cached for the session).
    let people_templates = load_people_templates();
    let personality_expressions = people_templates.personality_expressions;

    GameState {
        galaxy,
        journey,
        events: all_seed_events(),
        pipeline_state: PipelineState::default(),
        pipeline_config: PipelineConfig::default(),
        rng: StdRng::seed_from_u64(seed),
        visit_log,
        last_ticked_day: 0.0,
        pending_followups: Vec::new(),
        discussed_topics: HashMap::new(),
        npc_shared_knowledge: HashMap::new(),
        personality_expressions,
        llm_config: LlmConfig::default(), // Disabled by default — enabled at startup.
        llm_event_counter: 0,
        scene_history: Vec::new(),
        recent_tick_events: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Display functions
// ---------------------------------------------------------------------------

fn display_system_info(gs: &GameState) {
    let sys = gs.current_system();
    let faction_str = match sys.controlling_civ {
        Some(id) => gs.civ_name(id).to_string(),
        None => "Unclaimed".into(),
    };

    display_header(&format!("{} — {}", sys.name, faction_str));

    println!("  Star: {}  |  Infrastructure: {}",
        sys.star_type, sys.infrastructure_level);

    if !sys.locations.is_empty() {
        let dockable: Vec<&Location> = sys.locations.iter()
            .filter(|l| l.discovered && !l.services.is_empty())
            .collect();
        let scannable: Vec<&Location> = sys.locations.iter()
            .filter(|l| l.discovered && l.services.is_empty())
            .collect();
        let hidden = sys.locations.iter().filter(|l| !l.discovered).count();

        if !dockable.is_empty() {
            println!();
            for loc in &dockable {
                let infra_str = if loc.infrastructure != InfrastructureLevel::None {
                    format!(" [{}]", loc.infrastructure)
                } else { String::new() };
                println!("  · {} — {}{}", loc.name, loc.location_type, infra_str);
            }
        }
        if !scannable.is_empty() {
            for loc in &scannable {
                println!("  · {} — {} (no docking)", loc.name, loc.location_type);
            }
        }
        if hidden > 0 {
            println!("  · {} unidentified signal{}", hidden, if hidden == 1 { "" } else { "s" });
        }
    }

    if let Some(years) = gs.galactic_years_since_last_visit() {
        if years > 0.1 {
            println!("  Last visit: {:.0} galactic years ago", years);
        }
    }
    
    // Faction presence — show what the player would actually notice.
    if !sys.faction_presence.is_empty() {
        println!();
        let mut visible: Vec<_> = sys.faction_presence.iter()
            .filter(|fp| fp.visibility >= 0.3)
            .collect();
        visible.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap());

        if visible.is_empty() {
            println!("  No obvious faction activity.");
        } else {
            for fp in &visible {
                let name = gs.faction_name(fp.faction_id);
                let desc = if fp.strength >= 0.7 {
                    "strong presence"
                } else if fp.strength >= 0.4 {
                    "established presence"
                } else {
                    "minor presence"
                };
                println!("  · {} — {}", name, desc);
            }
        }

        let hidden = sys.faction_presence.iter()
            .filter(|fp| fp.visibility < 0.3 && fp.visibility > 0.0)
            .count();
        if hidden > 0 {
            println!("  ... and signs of {} other interest{}.",
                hidden, if hidden == 1 { "" } else { "s" });
        }
    }

    // NPCs — show those at current location when docked, or system overview.
    if gs.journey.current_location.is_some() {
        let npcs = gs.npcs_here();
        if !npcs.is_empty() {
            println!();
            for npc in &npcs {
                let faction_str = npc.faction_id
                    .map(|fid| gs.faction_name(fid).to_string())
                    .unwrap_or_else(|| "Independent".into());
                println!("  {} — {} ({})", npc.name, npc.title, faction_str);
            }
        }
    } else {
        // At system edge — show overview of who's where.
        let npcs = gs.npcs_in_system();
        if !npcs.is_empty() {
            println!();
            for npc in &npcs {
                let faction_str = npc.faction_id
                    .map(|fid| gs.faction_name(fid).to_string())
                    .unwrap_or_else(|| "Independent".into());
                let loc_str = npc.home_location_id
                    .map(|lid| gs.location_name(lid).to_string())
                    .unwrap_or_else(|| "unknown".into());
                println!("  {} — {} ({}, at {})", npc.name, npc.title, faction_str, loc_str);
            }
        }
    }

    // Economy summary — fuel and supply prices from the primary station.
    if let Some(loc) = gs.primary_location() {
        if let Some(ref econ) = loc.economy {
            println!();
            println!("  Fuel: {:.1} cr/unit  |  Supplies: {:.1} cr/unit  (at {})",
                econ.fuel_price, econ.supply_price, loc.name);
        }
    }

    // Show current location.
    if let Some(loc) = gs.current_location() {
        if loc.services.contains(&LocationService::Docking) {
            println!("\n  Docked: {}", loc.name);
        } else {
            println!("\n  Orbiting: {}", loc.name);
        }
    } else {
        println!("\n  Position: System edge");
    }
}

fn display_ship_status(gs: &GameState) {
    let ship = &gs.journey.ship;
    let time = &gs.journey.time;

    println!("\n  Ship: {}  |  Hull: {:.0}%  |  Fuel: {:.0}/{:.0}  |  Credits: {:.0}",
        ship.name,
        ship.hull_condition * 100.0,
        ship.fuel, ship.fuel_capacity,
        gs.journey.resources);

    // Supply status — show warning level when relevant.
    let supply_status = starbound_game::supplies::SupplyStatus::assess(
        ship.supplies, ship.supply_capacity,
    );
    let days_left = starbound_game::supplies::days_remaining(&gs.journey);

    if supply_status.is_warning() {
        println!("  Supplies: {:.0}/{:.0} [{}]  (~{:.0} days remaining)",
            ship.supplies, ship.supply_capacity,
            supply_status.label(),
            days_left);
    } else {
        println!("  Supplies: {:.0}/{:.0}  (~{:.0} days remaining)",
            ship.supplies, ship.supply_capacity, days_left);
    }

    println!("  Time: {:.1} months personal / {:.1} years galactic",
        time.personal_days / 30.44,
        time.galactic_years());

    // Cargo summary.
    let total_cargo: u32 = ship.cargo.values().sum();
    if total_cargo > 0 {
        let items: Vec<String> = ship.cargo.iter()
            .map(|(name, qty)| format!("{} x{}", name, qty))
            .collect();
        println!("  Cargo: {}/{}  [{}]", total_cargo, ship.cargo_capacity, items.join(", "));
    } else {
        println!("  Cargo: empty  (capacity: {})", ship.cargo_capacity);
    }

    // Reputation — show active labels if any.
    if !gs.journey.profile.labels.is_empty() {
        let label_strs: Vec<String> = gs.journey.profile.labels.iter()
            .filter(|l| l.strength >= 0.3)
            .map(|l| format!("{} ({:.0}%)", l.kind, l.strength * 100.0))
            .collect();
        if !label_strs.is_empty() {
            println!("  Known as: {}", label_strs.join(", "));
        }
    }
}

fn display_routes(plans: &[TravelPlan], gs: &GameState) {
    display_header("Available Routes");

    for (i, plan) in plans.iter().enumerate() {
        let dest_name = gs.system_name(plan.destination_id);
        let desc = describe_plan(plan, dest_name);
        let marker = if plan.feasible { " " } else { "×" };
        println!("  {}{}) {}", marker, i + 1, desc);
    }
}

fn display_galactic_news(tick_result: &TickResult, _gs: &GameState) {
    clear_screen();
    display_header("While You Were Away");
    println!();

    let years = tick_result.days_consumed / 365.25;
    for line in wrap_text(
        &format!(
            "{:.1} galactic years have passed. The galaxy has not been idle.",
            years,
        ), 60,
    ) { println!("  {}", line); }
    println!();

    // Show the most interesting events (cap at 8 to avoid wall of text).
    let mut shown = 0;
    for event in &tick_result.events {
        if shown >= 8 { break; }

        // Skip some internal consolidation noise.
        if event.category == TickEventCategory::Internal
            && event.description.contains("focused on internal consolidation")
        {
            continue;
        }

        let icon = match event.category {
            TickEventCategory::Expansion => "  ▸",
            TickEventCategory::Infrastructure => "  ▸",
            TickEventCategory::Diplomacy => "  ▸",
            TickEventCategory::Military => "  ▸",
            TickEventCategory::Internal => "  ▸",
            TickEventCategory::Faction => "  ▸",
            TickEventCategory::Economic => "  ▸",
        };

        for line in wrap_text(&format!("{} {}", icon, event.description), 60) {
            println!("{}", line);
        }
        shown += 1;
    }

    let remaining = tick_result.events.len().saturating_sub(8);
    if remaining > 0 {
        println!("
  ... and {} other developments.", remaining);
    }
}

fn display_encounter(event: &SeedEvent, ctx: &TemplateContext) {
    let resolved_text = resolve_template(&event.text, ctx);

    println!("\n{}", DIVIDER);
    println!();

    for paragraph in resolved_text.split("\n\n") {
        for line in wrap_text(paragraph.trim(), 60) {
            println!("  {}", line);
        }
        println!();
    }

    println!("{}", THIN_DIVIDER);

    for (i, choice) in event.choices.iter().enumerate() {
        println!("  {}) {}", i + 1, choice.label);
    }
}

fn parse_tone(s: &str) -> starbound_core::narrative::Tone {
    match s {
        "tense" => starbound_core::narrative::Tone::Tense,
        "quiet" => starbound_core::narrative::Tone::Quiet,
        "wonder" => starbound_core::narrative::Tone::Wonder,
        "urgent" => starbound_core::narrative::Tone::Urgent,
        "melancholy" => starbound_core::narrative::Tone::Melancholy,
        _ => starbound_core::narrative::Tone::Mundane,
    }
}

/// Resolve template placeholders in effect text fields.
/// Only affects effects that contain text (Narrative, SpawnThread, AddConcern, AddCargo).
fn resolve_effect_defs(defs: &[EffectDef], ctx: &TemplateContext) -> Vec<EffectDef> {
    defs.iter().map(|def| match def {
        EffectDef::Narrative { text } => EffectDef::Narrative {
            text: resolve_template(text, ctx),
        },
        EffectDef::SpawnThread { thread_type, description } => EffectDef::SpawnThread {
            thread_type: thread_type.clone(),
            description: resolve_template(description, ctx),
        },
        EffectDef::AddConcern { text } => EffectDef::AddConcern {
            text: resolve_template(text, ctx),
        },
        EffectDef::AddCargo { item, quantity } => EffectDef::AddCargo {
            item: resolve_template(item, ctx),
            quantity: *quantity,
        },
        other => other.clone(),
    }).collect()
}

/// Run a single encounter: display it, get the player's choice, apply effects,
/// show consequences. Returns the follow-up info if the chosen option has one.
fn run_encounter<'a>(
    gs: &mut GameState,
    event: &'a SeedEvent,
) -> Option<&'a starbound_encounters::seed_event::FollowUp> {
    let tone = parse_tone(&event.tone);
    gs.pipeline_state.record_event(&event.id, tone);

    clear_screen();
    let ctx = gs.template_context();
    display_encounter(event, &ctx);
    println!();

    let input = prompt("  > ");
    let choice_idx: usize = input.parse::<usize>()
        .unwrap_or(1)
        .saturating_sub(1)
        .min(event.choices.len().saturating_sub(1));

    let chosen = &event.choices[choice_idx];

    // Resolve templates in effect text fields before applying.
    let resolved_effects = resolve_effect_defs(&chosen.effects, &ctx);
    let effects = convert_effects(&resolved_effects);
    let report = apply_effects(
        &effects,
        &mut gs.journey,
        &format!("{}: {}", event.id, chosen.label),
    );

    // Display consequences.
    clear_screen();
    display_header("Consequences");
    println!();

    for line in wrap_text(&format!("You chose: {}", chosen.label), 60) {
        println!("  {}", line);
    }
    println!();

    if !report.log_entry.is_empty() {
        for line in wrap_text(&report.log_entry, 60) {
            println!("  {}", line);
        }
        println!();
    }

    if !report.changes.is_empty() {
        println!("{}", THIN_DIVIDER);
        for change in &report.changes {
            println!("  · {}", change);
        }
    }

    if report.threads_spawned > 0 {
        println!();
        println!("  {} new thread{} in the ledger.",
            report.threads_spawned,
            if report.threads_spawned == 1 { "" } else { "s" },
        );
    }

    pause();

    chosen.follows.as_ref()
}

/// Run an encounter and follow any immediate chains. Queues NextArrival
/// follow-ups in GameState for later.
// ---------------------------------------------------------------------------
// Encounter generation — LLM with seed library fallback
// ---------------------------------------------------------------------------

/// Try to generate and run an encounter. Tries LLM first (if enabled),
/// falls back to the seed library pipeline.
///
/// Returns true if an encounter fired, false if silence.
fn try_encounter(
    gs: &mut GameState,
    trigger: EventTrigger,
    years_since: Option<f64>,
    destination: Option<DestinationInfo>,
) -> bool {
    // --- Silence check (applies to both LLM and seed pipeline) ---
    // Player actions always fire. Ambient triggers roll for silence first.
    if !trigger.is_player_action() {
        let silence_rate = trigger.base_silence_rate();
        if gs.rng.gen::<f64>() < silence_rate {
            gs.pipeline_state.record_silence();
            return false;
        }
    }

    // Gather context needed by both LLM and pipeline.
    let system = gs.current_system().clone();
    let loc_type = gs.current_location_type_str();
    let loc_infra = gs.current_location().map(|l| l.infrastructure);

    let faction_name = system.faction_presence.iter()
        .max_by(|a, b| a.strength.partial_cmp(&b.strength).unwrap())
        .and_then(|fp| gs.galaxy.factions.iter().find(|f| f.id == fp.faction_id))
        .map(|f| f.name.clone());

    let civ_name = system.controlling_civ
        .and_then(|cid| gs.galaxy.civilizations.iter().find(|c| c.id == cid))
        .map(|c| c.name.clone());

    let location_name = gs.current_location().map(|l| l.name.clone());
    let location_description = gs.current_location().map(|l| l.description.clone());

    // --- Try LLM generation ---
    if gs.llm_config.is_available() {
        let event_id = gs.next_llm_id();

        // Build context filtered by trigger type.
        let (established_facts, recent_scenes) = build_filtered_context(gs, &trigger);

        let npcs_here: Vec<&Npc> = if trigger_needs_npcs(&trigger) {
            gs.npcs_here()
        } else {
            vec![]
        };

        let example = gs.events.iter()
            .find(|e| e.matches_trigger(&trigger))
            .or_else(|| gs.events.first());

        let result = generate_encounter(
            &gs.llm_config,
            &trigger,
            &system,
            &gs.journey,
            npcs_here,
            location_name.clone(),
            loc_type.clone(),
            location_description.clone(),
            faction_name,
            civ_name,
            recent_scenes,
            established_facts,
            destination,
            example,
            &event_id,
        );

        if let Some(gen) = result {
            if let Some(tokens) = gen.tokens_used {
                eprintln!("  [LLM] Generated {} ({} tokens)", event_id, tokens);
            }

            let text_preview: String = gen.event.text.chars().take(150).collect();
            let choice_labels: Vec<&str> = gen.event.choices.iter()
                .map(|c| c.label.as_str())
                .collect();
            let scene_summary = format!(
                "At {}: {}... [choices: {}]",
                location_name.as_deref().unwrap_or("unknown"),
                text_preview.trim(),
                choice_labels.join(", "),
            );
            gs.record_scene(scene_summary);

            let event = gen.event;
            run_encounter_chain(gs, &event);
            return true;
        }
        // LLM failed — fall through to seed pipeline.
    }

    // --- Seed library pipeline fallback ---
    let result = run_pipeline(
        &gs.events, &system, &gs.journey, years_since,
        &gs.pipeline_state, &gs.pipeline_config, &mut gs.rng,
        trigger,
        loc_type.as_deref(),
        loc_infra,
    );

    match result {
        PipelineResult::Event { event, .. } => {
            let event = event.clone();

            let text_preview: String = event.text.chars().take(150).collect();
            let scene_summary = format!(
                "At {}: {}...",
                location_name.as_deref().unwrap_or("unknown"),
                text_preview.trim(),
            );
            gs.record_scene(scene_summary);

            run_encounter_chain(gs, &event);
            true
        }
        PipelineResult::Silence { .. } => {
            gs.pipeline_state.record_silence();
            false
        }
    }
}

/// Whether this trigger type needs NPC context.
fn trigger_needs_npcs(trigger: &EventTrigger) -> bool {
    match trigger {
        EventTrigger::Transit => false,  // Crew only, no NPCs.
        _ => true,
    }
}

/// Build context filtered by trigger type — transit events get minimal context,
/// action events get everything.
fn build_filtered_context(gs: &GameState, trigger: &EventTrigger) -> (Vec<String>, Vec<String>) {
    match trigger {
        EventTrigger::Transit => {
            // Transit = crew moment. Only needs: star type, crew names, destination.
            // No cargo, no contracts, no threads, no NPCs, no faction details.
            let system = gs.current_system();
            let facts = vec![
                format!("Star type: {} — {}", system.star_type, system.star_type.star_descriptor()),
                format!("Ship: {}", gs.journey.ship.name),
                format!("This is a quiet moment during sublight travel. Focus on the crew and the ship."),
            ];
            // Only last 1 scene for transit (prevent over-referencing).
            let scenes = gs.scene_history.last().cloned().into_iter().collect();
            (facts, scenes)
        }
        EventTrigger::Docked => {
            // Docked = first impression of a place. Location description, NPCs, atmosphere.
            // No cargo, no contracts unless at destination.
            let mut facts = Vec::new();
            let system = gs.current_system();

            if let Some(loc) = gs.current_location() {
                facts.push(format!("You are at {}, a {}", loc.name, loc.location_type.category_str()));
                if !loc.description.is_empty() {
                    facts.push(format!("Location: {}", loc.description));
                }
            }
            facts.push(format!("Star: {} — {}", system.star_type, system.star_type.light_description()));

            // NPCs present.
            let npcs = gs.npcs_here();
            for npc in &npcs {
                let mut npc_fact = format!("NPC here: {} — {}", npc.name, npc.title);
                if let Some(last) = npc.last_interaction() {
                    npc_fact.push_str(&format!(". Previously: {}", last.summary));
                }
                facts.push(npc_fact);
            }

            let scenes: Vec<String> = gs.scene_history.iter().rev().take(2).rev().cloned().collect();
            (facts, scenes)
        }
        EventTrigger::Arrival => {
            // Arrival = fuller context but still focused on location.
            let mut facts = gs.build_established_facts();
            // Strip cargo/contract details from arrival — player hasn't acted yet.
            facts.retain(|f| !f.starts_with("Active contract:") && !f.starts_with("Cargo hold:"));
            let scenes: Vec<String> = gs.scene_history.iter().rev().take(2).rev().cloned().collect();
            (facts, scenes)
        }
        _ => {
            // Action/Linger = full context. This is where everything matters.
            let facts = gs.build_established_facts();
            let scenes = gs.scene_history.clone();
            (facts, scenes)
        }
    }
}

// ---------------------------------------------------------------------------
// Encounter display and resolution
// ---------------------------------------------------------------------------

fn run_encounter_chain(gs: &mut GameState, initial_event: &SeedEvent) {
    // Run the initial event.
    let followup = run_encounter(gs, initial_event);

    // Handle follow-up chain.
    let mut next_id = match followup {
        Some(f) => match f.delay {
            FollowUpDelay::Immediate => Some(f.event_id.clone()),
            FollowUpDelay::NextArrival => {
                gs.pending_followups.push(f.event_id.clone());
                None
            }
        },
        None => None,
    };

    // Chase immediate follow-ups (with a depth limit to prevent infinite loops).
    let mut depth = 0;
    const MAX_CHAIN_DEPTH: usize = 10;

    while let Some(event_id) = next_id.take() {
        depth += 1;
        if depth > MAX_CHAIN_DEPTH {
            eprintln!("  [Event chain depth limit reached at '{}']", event_id);
            break;
        }

        // Look up the follow-up event by ID.
        let follow_event = gs.events.iter().find(|e| e.id == event_id);
        match follow_event {
            Some(event) => {
                let event = event.clone();
                let followup = run_encounter(gs, &event);
                next_id = match followup {
                    Some(f) => match f.delay {
                        FollowUpDelay::Immediate => Some(f.event_id.clone()),
                        FollowUpDelay::NextArrival => {
                            gs.pending_followups.push(f.event_id.clone());
                            None
                        }
                    },
                    None => None,
                };
            }
            None => {
                eprintln!("  [Follow-up event '{}' not found in library]", event_id);
                break;
            }
        }
    }
}

fn display_event_log(gs: &GameState) {
    display_header("Captain's Log");

    if gs.journey.event_log.is_empty() {
        println!("  The log is empty. Your story hasn't started yet.");
        return;
    }

    for event in gs.journey.event_log.iter().rev().take(10) {
        let pm = event.timestamp.personal_days / 30.44;
        let gy = event.timestamp.galactic_years();
        println!("  [{:.1}m / {:.1}y] {}", pm, gy, event.description);
    }

    if gs.journey.event_log.len() > 10 {
        println!("  ... and {} earlier entries.", gs.journey.event_log.len() - 10);
    }
}

fn display_crew_detail(gs: &GameState) {
    display_header("Crew");

    for member in &gs.journey.crew {
        println!("\n  {} — {} ({})", member.name, member.role, member.state.mood);
        println!("  Stress: {:.0}%", member.state.stress * 100.0);
        if !member.state.active_concerns.is_empty() {
            println!("  On their mind: {}", member.state.active_concerns.join("; "));
        }
        let bg: String = member.background.chars().take(120).collect();
        println!("  {}{}", bg, if member.background.len() > 120 { "..." } else { "" });
    }
}

// ---------------------------------------------------------------------------
// Crew menu — interactive crew conversations
// ---------------------------------------------------------------------------

fn crew_menu(gs: &mut GameState) {
    loop {
        clear_screen();
        display_header("Crew");
        println!();

        for (i, member) in gs.journey.crew.iter().enumerate() {
            let mood_hint = match member.state.mood {
                Mood::Content => "",
                Mood::Anxious => " — seems anxious",
                Mood::Determined => " — focused",
                Mood::Grieving => " — carrying something",
                Mood::Restless => " — restless",
                Mood::Hopeful => " — in good spirits",
                Mood::Withdrawn => " — withdrawn",
                Mood::Angry => " — tense",
                Mood::Inspired => " — energized",
            };
            println!("  {}) {} — {}{}", i + 1, member.name, member.role, mood_hint);
        }
        println!();
        println!("  v) View crew details");
        println!("  0) Back");
        println!();

        let input = prompt("  > ");
        let input = input.trim().to_lowercase();

        if input == "0" || input == "back" {
            return;
        }

        if input == "v" || input == "view" {
            display_crew_detail(gs);
            pause();
            continue;
        }

        if let Ok(idx) = input.parse::<usize>() {
            if idx >= 1 && idx <= gs.journey.crew.len() {
                let member_id = gs.journey.crew[idx - 1].id;
                crew_conversation_screen(gs, member_id);
            }
        }
    }
}

fn crew_conversation_screen(gs: &mut GameState, member_id: Uuid) {
    // Get discussed topics for this crew member (anti-repeat).
    let discussed = gs.discussed_topics
        .entry(member_id)
        .or_insert_with(Vec::new)
        .clone();

    // Find the member index.
    let member_idx = match gs.journey.crew.iter().position(|c| c.id == member_id) {
        Some(i) => i,
        None => return,
    };

    // Generate topics.
    let topics = generate_topics(
        &gs.journey.crew[member_idx],
        &gs.journey,
        &discussed,
    );

    if topics.is_empty() {
        clear_screen();
        let name = gs.journey.crew[member_idx].name.clone();
        display_header(&format!("Talking to {}", name));
        println!();
        println!("  {} doesn't seem to have anything pressing to discuss.", name);
        println!("  Sometimes silence between people is enough.");
        pause();
        return;
    }

    // Show the conversation.
    loop {
        clear_screen();
        let member = &gs.journey.crew[member_idx];
        display_header(&format!("{}", member.name));
        println!();

        // Narrative state description.
        let state_desc = describe_crew_state(member);
        for line in wrap_text(&state_desc, 60) {
            println!("  {}", line);
        }
        println!();
        println!("{}", THIN_DIVIDER);

        // Show available topics (max 3).
        let available: Vec<&ConversationTopic> = topics.iter().take(3).collect();

        println!();
        for (i, topic) in available.iter().enumerate() {
            // Show a brief label derived from the prompt (first sentence).
            let label = topic.prompt
                .split('\n').next().unwrap_or(&topic.prompt)
                .chars().take(70).collect::<String>();
            let label = if topic.prompt.split('\n').next().unwrap_or("").len() > 70 {
                format!("{}...", label.trim())
            } else {
                label.trim().to_string()
            };
            println!("  {}) {}", i + 1, label);
        }
        println!("  0) Leave");
        println!();

        let input = prompt("  > ");
        let idx: usize = match input.trim().parse::<usize>() {
            Ok(n) if n >= 1 && n <= available.len() => n - 1,
            _ => return,
        };

        // Show the full topic and get response.
        let topic = available[idx].clone();
        run_crew_topic(gs, member_idx, &topic);

        // Record as discussed.
        gs.discussed_topics
            .entry(member_id)
            .or_insert_with(Vec::new)
            .push(topic.id.clone());

        // One topic per visit — return after.
        return;
    }
}

fn run_crew_topic(gs: &mut GameState, member_idx: usize, topic: &ConversationTopic) {
    clear_screen();
    let member_name = gs.journey.crew[member_idx].name.clone();
    display_header(&member_name);
    println!();

    // Show the full topic prompt.
    for line in wrap_text(&topic.prompt, 60) {
        println!("  {}", line);
    }

    println!();
    println!("{}", THIN_DIVIDER);
    println!();

    // Show response options.
    for (i, response) in topic.responses.iter().enumerate() {
        println!("  {}) {}", i + 1, response.label);
    }
    println!();

    let input = prompt("  > ");
    let choice: usize = match input.trim().parse::<usize>() {
        Ok(n) if n >= 1 && n <= topic.responses.len() => n - 1,
        _ => 0, // Default to first response.
    };

    let response = &topic.responses[choice];

    // Display follow-up.
    if let Some(ref follow_up) = response.follow_up {
        println!();
        for line in wrap_text(follow_up, 60) {
            println!("  {}", line);
        }
    }

    // Convert and apply effects.
    let game_effects = conversation_effects_to_game_effects(&response.effects);
    let description = format!("Crew conversation: {} — {}", member_name, response.label);
    let _report = apply_effects(&game_effects, &mut gs.journey, &description);

    // Apply concern removals (special handling outside normal effects).
    apply_concern_removals(&mut gs.journey.crew[member_idx], &response.effects);

    // Show what changed (subtle, not a full consequence report).
    let trust_changed = response.effects.iter().any(|e| matches!(e,
        ConversationEffect::TrustProfessional(_) |
        ConversationEffect::TrustPersonal(_) |
        ConversationEffect::TrustIdeological(_)
    ));
    let stress_changed = response.effects.iter().any(|e| matches!(e,
        ConversationEffect::Stress(_)
    ));

    if trust_changed || stress_changed {
        println!();
        println!("{}", THIN_DIVIDER);
        if trust_changed {
            println!("  Something shifted between you.");
        }
        if stress_changed {
            let member = &gs.journey.crew[member_idx];
            if member.state.stress < 0.3 {
                println!("  {} seems lighter.", member.name);
            } else {
                println!("  The weight didn't lift, but it's shared now.");
            }
        }
    }

    pause();
}

fn display_mission(gs: &GameState) {
    display_header("Mission");

    println!("  Type: {}", gs.journey.mission.mission_type);
    println!();
    for line in wrap_text(&gs.journey.mission.core_truth, 58) {
        println!("  {}", line);
    }

    let discovered = gs.journey.mission.discovered_count();
    let total = gs.journey.mission.knowledge_nodes.len();
    if total > 0 {
        println!("\n  Knowledge: {}/{} fragments discovered", discovered, total);
    }
}

fn display_threads(gs: &GameState) {
    display_header("Thread Ledger");

    let open_threads: Vec<&starbound_core::narrative::Thread> = gs.journey.threads.iter()
        .filter(|t| t.resolution == starbound_core::narrative::ResolutionState::Open
            || t.resolution == starbound_core::narrative::ResolutionState::Partial)
        .collect();

    let resolved_threads: Vec<&starbound_core::narrative::Thread> = gs.journey.threads.iter()
        .filter(|t| t.resolution == starbound_core::narrative::ResolutionState::Resolved
            || t.resolution == starbound_core::narrative::ResolutionState::Transformed)
        .collect();

    if open_threads.is_empty() && resolved_threads.is_empty() {
        println!("  No threads yet. Your story is just beginning.");
        return;
    }

    if !open_threads.is_empty() {
        println!("\n  Open threads:");
        for thread in &open_threads {
            let tension_bar = tension_display(thread.tension);
            let age_gy = (gs.journey.time.galactic_days - thread.created_at.galactic_days) / 365.25;
            println!("  {} [{}] {}", tension_bar, thread.thread_type, thread.description);
            if age_gy > 1.0 {
                println!("      ({:.0} galactic years old)", age_gy);
            }
        }
    }

    if !resolved_threads.is_empty() {
        println!("\n  Resolved:");
        for thread in resolved_threads.iter().rev().take(5) {
            println!("  ✓ [{}] {}", thread.thread_type, thread.description);
        }
        if resolved_threads.len() > 5 {
            println!("    ... and {} more.", resolved_threads.len() - 5);
        }
    }
}

/// Visual tension indicator.
fn tension_display(tension: f32) -> String {
    let filled = (tension * 5.0).round() as usize;
    let empty = 5 - filled.min(5);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

// ---------------------------------------------------------------------------
// Title screen and intro
// ---------------------------------------------------------------------------

fn display_title() {
    clear_screen();
    println!();
    println!("    ╔══════════════════════════════════════════════╗");
    println!("    ║                                              ║");
    println!("    ║              S T A R B O U N D               ║");
    println!("    ║                                              ║");
    println!("    ║    The galaxy is vast, old, and strange.     ║");
    println!("    ║    You have a mission. You have a ship.      ║");
    println!("    ║    Everything else is up to you.             ║");
    println!("    ║                                              ║");
    println!("    ╚══════════════════════════════════════════════╝");
    println!();
    println!("  1) New Game");
    println!("  2) Quit");
    println!();
}

fn display_intro(gs: &GameState) {
    clear_screen();
    println!();
    println!("{}", DIVIDER);
    println!();

    let start_name = gs.current_system().name.clone();
    let civ_names: Vec<String> = gs.galaxy.civilizations.iter()
        .take(2)
        .map(|c| c.name.clone())
        .collect();
    let civ_context = if civ_names.len() >= 2 {
        format!("halfway between {} territory and the {}",
            civ_names[0], civ_names[1])
    } else {
        "at the edge of civilized space".into()
    };

    let intro = format!(
        "You stand on the bridge of the Persistence, docked at \
         {} — a transit station in contested space, {}. \
         No single power claims it officially. All of them \
         keep an eye on it.",
        start_name, civ_context
    );
    for line in wrap_text(&intro, 60) {
        println!("  {}", line);
    }
    println!();

    for line in wrap_text(
        "Your mission briefing sits on your console, read twice \
         and understood less each time. A signal from beyond \
         mapped space, encoding mathematics that shouldn't exist. \
         Find its source. That's all they told you. That's all \
         anyone knows.", 60
    ) { println!("  {}", line); }
    println!();

    for line in wrap_text(
        "Your crew is aboard. Your fuel tanks are mostly full. \
         Supplies should last a few months. \
         The galaxy is out there, waiting to not notice you.", 60
    ) { println!("  {}", line); }
    println!();
    println!("{}", DIVIDER);

    pause();
}

// ---------------------------------------------------------------------------
// Main game loop
// ---------------------------------------------------------------------------

fn game_loop(gs: &mut GameState) {
    display_intro(gs);

    // Auto-dock at the start system's primary location.
    if gs.journey.current_location.is_none() {
        if let Some(loc) = gs.primary_location() {
            gs.journey.current_location = Some(loc.id);
        }
    }

    loop {
        clear_screen();
        display_system_info(gs);
        display_ship_status(gs);

        let is_docked = gs.journey.current_location.is_some();

        println!("\n  What do you do?");
        if is_docked {
            println!("  1) Navigate      2) Actions");
            println!("  3) People        4) Contracts");
            println!("  5) Crew          6) Mission");
            println!("  7) Log           8) Threads");
            println!("  9) Quit");
        } else {
            println!("  1) Navigate      5) Crew");
            println!("  6) Mission       7) Log");
            println!("  8) Threads       9) Quit");
        }
        println!();

        let choice = prompt("  > ");

        match choice.as_str() {
            "1" | "navigate" | "nav" => system_map(gs),
            "2" | "actions" | "act" if is_docked => action_menu(gs),
            "3" | "people" | "talk" if is_docked => people_menu(gs),
            "4" | "contracts" if is_docked => display_contracts(gs),
            "5" | "crew" => crew_menu(gs),
            "6" | "mission" => {
                display_mission(gs);
                pause();
            }
            "7" | "log" => {
                display_event_log(gs);
                pause();
            }
            "8" | "threads" => {
                display_threads(gs);
                pause();
            }
            "9" | "q" | "quit" => {
                println!("\n  The galaxy continues without you.\n");
                break;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// System map — intra-system navigation
// ---------------------------------------------------------------------------

fn system_map(gs: &mut GameState) {
    loop {
        clear_screen();
        let sys = gs.current_system();
        display_header(&format!("{} — System Map", sys.name));

        let current_dist = gs.current_location()
            .map(|l| l.orbital_distance)
            .unwrap_or(0.0); // system edge = 0 AU

        let current_loc_name = gs.current_location()
            .map(|l| l.name.clone())
            .unwrap_or_else(|| "System edge".into());

        println!("\n  Current position: {} ({:.1} AU)", current_loc_name, current_dist);
        println!();

        // List all discovered locations.
        let sys = gs.current_system();
        let mut locs: Vec<&Location> = sys.locations.iter()
            .filter(|l| l.discovered)
            .collect();
        locs.sort_by(|a, b| a.orbital_distance.partial_cmp(&b.orbital_distance).unwrap());

        for (i, loc) in locs.iter().enumerate() {
            let dist = (loc.orbital_distance - current_dist).abs();
            let travel_hours = dist * 12.0;
            let is_here = gs.journey.current_location == Some(loc.id);

            let services_str = if loc.services.is_empty() {
                "no services".into()
            } else {
                loc.services.iter()
                    .map(|s| format!("{}", s))
                    .collect::<Vec<_>>()
                    .join(", ")
            };

            if is_here {
                let here_label = if loc.services.contains(&LocationService::Docking) {
                    "DOCKED"
                } else {
                    "ORBITING"
                };
                println!("  {}) {} — {} [{}]", i + 1, loc.name, loc.location_type, here_label);
                println!("     {}", services_str);
            } else {
                let time_str = if travel_hours < 1.0 {
                    "< 1 hour".into()
                } else if travel_hours < 24.0 {
                    format!("{:.0} hours", travel_hours)
                } else {
                    format!("{:.1} days", travel_hours / 24.0)
                };
                println!("  {}) {} — {} ({:.1} AU, {})",
                    i + 1, loc.name, loc.location_type, loc.orbital_distance, time_str);
                println!("     {}", services_str);
            }
        }

        // Hidden locations.
        let hidden_count = sys.locations.iter().filter(|l| !l.discovered).count();
        if hidden_count > 0 {
            println!("\n  {} unidentified signal{} detected",
                hidden_count, if hidden_count == 1 { "" } else { "s" });
        }

        println!();
        if hidden_count > 0 {
            println!("  s) Scan system (reveal hidden locations)");
        }
        println!("  j) FTL Jump (leave system)");
        println!("  0) Back");
        println!();

        let input = prompt("  > ");
        let input = input.trim().to_lowercase();

        if input == "0" || input == "back" {
            return;
        }

        if input == "j" || input == "jump" || input == "ftl" {
            travel_menu(gs);
            return;
        }

        if input == "s" || input == "scan" {
            run_system_scan(gs);
            continue;
        }

        // Navigate to a location.
        if let Ok(idx) = input.parse::<usize>() {
            if idx >= 1 && idx <= locs.len() {
                let target_id = locs[idx - 1].id;

                if gs.journey.current_location == Some(target_id) {
                    println!("  You're already here.");
                    pause();
                    continue;
                }

                navigate_to_location(gs, target_id, current_dist);
                return;
            }
        }
    }
}

/// Sublight travel to a location within the current system.
/// Costs personal time and supplies. May fire a transit ambient event
/// during the journey and an arrival encounter at the destination.
fn navigate_to_location(gs: &mut GameState, target_id: Uuid, from_dist: f32) {
    // Snapshot target info before borrowing gs mutably.
    let (target_name, target_desc, target_dist, can_dock, target_type_str) = {
        let sys = gs.current_system();
        let target = sys.locations.iter().find(|l| l.id == target_id)
            .expect("Target location should exist");
        (
            target.name.clone(),
            target.description.clone(),
            target.orbital_distance,
            target.services.contains(&LocationService::Docking),
            target.location_type.category_str().to_string(),
        )
    };

    let dist = (target_dist - from_dist).abs();
    let travel_hours = dist * 12.0;
    let travel_days = travel_hours / 24.0;

    // Build travel context string.
    let travel_time_str = if travel_hours < 1.0 {
        "short burn".into()
    } else if travel_hours < 24.0 {
        format!("{:.0} hours", travel_hours)
    } else {
        format!("{:.1} days", travel_days)
    };
    let system_name = gs.current_system().name.clone();
    let dest_info = DestinationInfo {
        name: target_name.clone(),
        location_type: target_type_str.clone(),
        description: target_desc.clone(),
        can_dock,
        travel_context: format!(
            "{} sublight transit within {} system",
            travel_time_str, system_name,
        ),
    };

    // Apply time costs.
    gs.journey.time.personal_days += travel_days as f64;
    let sys_tf = gs.current_system().time_factor;
    gs.journey.time.galactic_days += travel_days as f64 * sys_tf;

    // Supply consumption during transit.
    let crew_count = gs.journey.crew.len() as f32;
    let supply_cost = crew_count * 0.1 * travel_days;
    gs.journey.ship.supplies = (gs.journey.ship.supplies - supply_cost).max(0.0);

    // Transit narrative.
    clear_screen();
    display_header("Sublight Transit");
    println!();

    if travel_hours < 1.0 {
        println!("  Short burn to {}.", target_name);
    } else if travel_hours < 24.0 {
        println!("  {:.0} hours at sublight to {}.", travel_hours, target_name);
    } else {
        println!("  {:.1} days at sublight to {}.", travel_days, target_name);
    }
    println!();

    for line in wrap_text(&target_desc, 60) {
        println!("  {}", line);
    }

    // --- Transit ambient event (fires during the journey) ---
    // ~22% chance of a small moment during sublight travel.
    try_encounter(gs, EventTrigger::Transit, None, Some(dest_info.clone()));

    // Set location.
    gs.journey.current_location = Some(target_id);

    if can_dock {
        println!("\n  Docked at {}.", target_name);
    } else {
        println!("\n  Entering orbit around {}.", target_name);
    }

    // Check contract progress at this location.
    check_contract_progress(gs);

    // Validate trade rumors against actual prices at this location.
    if let Some(loc) = gs.current_location().cloned() {
        let sys = gs.current_system().clone();
        let validations = validate_rumors_at_location(
            &gs.journey, &sys, &loc, gs.journey.time.galactic_days,
        );
        for v in &validations {
            println!();
            for line in wrap_text(&v.message, 60) {
                println!("  {}", line);
            }
        }
        // Apply outcomes to rumors.
        for v in validations {
            if let Some(rumor) = gs.journey.discovered_rumors.get_mut(v.rumor_idx) {
                rumor.outcome = Some(v.outcome);
                rumor.acted_on = true;
            }
        }
    }

    pause();

    // --- Docked/orbit ambient event ---
    // ~27% chance of a small atmosphere moment on arrival.
    if can_dock {
        try_encounter(gs, EventTrigger::Docked, None, Some(dest_info.clone()));
    }

    // --- Fire encounter on location arrival ---
    let years_since = gs.galactic_years_since_last_visit();
    try_encounter(gs, EventTrigger::Arrival, years_since, Some(dest_info));
}

/// Scan the system to reveal hidden locations.
fn run_system_scan(gs: &mut GameState) {
    clear_screen();
    display_header("System Scan");
    println!();

    // Scan costs a small amount of time.
    gs.journey.time.personal_days += 0.1; // ~2.4 hours
    let sys_tf = gs.current_system().time_factor;
    gs.journey.time.galactic_days += 0.1 * sys_tf;

    // Sensor condition affects scan quality.
    let sensor_cond = gs.journey.ship.modules.sensors.condition;

    let sys_id = gs.journey.current_system;
    let system = gs.galaxy.systems.iter_mut()
        .find(|s| s.id == sys_id)
        .expect("Current system should exist");

    let mut revealed = Vec::new();
    for loc in &mut system.locations {
        if !loc.discovered {
            // Higher sensor condition = higher chance of revealing.
            // Base 60% + 30% from sensors = 90% at full condition.
            let chance = 0.6 + sensor_cond as f64 * 0.3;
            let roll: f64 = gs.rng.gen();
            if roll < chance {
                loc.discovered = true;
                revealed.push(loc.name.clone());
            }
        }
    }

    if revealed.is_empty() {
        let hidden_remaining = system.locations.iter().filter(|l| !l.discovered).count();
        if hidden_remaining > 0 {
            println!("  Sensors sweep the system... nothing new resolves.");
            println!("  {} signal{} remain unresolved.",
                hidden_remaining, if hidden_remaining == 1 { "" } else { "s" });
            if sensor_cond < 0.5 {
                println!("\n  Sensor array at {:.0}% — degraded scans may be missing contacts.",
                    sensor_cond * 100.0);
            }
        } else {
            println!("  Sensors report all contacts mapped. No new signals.");
        }
    } else {
        println!("  Sensors sweep the system...\n");
        for name in &revealed {
            println!("  ▸ New contact resolved: {}", name);
        }
        let hidden_remaining = system.locations.iter().filter(|l| !l.discovered).count();
        if hidden_remaining > 0 {
            println!("\n  {} signal{} still unresolved.",
                hidden_remaining, if hidden_remaining == 1 { "" } else { "s" });
        }
    }

    // Fire a scan encounter (for events like "your scan attracts attention").
    try_encounter(gs, PlayerIntent::Scan.into(), None, None);

    pause();
}

// ---------------------------------------------------------------------------
// Action menu — player-initiated encounters
// ---------------------------------------------------------------------------

/// Determine which actions are available at the current location.
fn available_actions(gs: &GameState) -> Vec<PlayerIntent> {
    let mut actions = Vec::new();

    // Investigate is always available — examine what's at this location.
    actions.push(PlayerIntent::Investigate);

    // Other actions depend on the current location's services.
    if let Some(loc) = gs.current_location() {
        if loc.services.contains(&LocationService::Trade) {
            actions.push(PlayerIntent::Trade);
        }
        if loc.services.contains(&LocationService::Repair) {
            actions.push(PlayerIntent::Repair);
        }
        if loc.services.contains(&LocationService::Refuel) || loc.services.contains(&LocationService::Trade) {
            actions.push(PlayerIntent::Resupply);
        }
        if loc.services.contains(&LocationService::Rumors) {
            actions.push(PlayerIntent::GatherRumors);
        }
    }

    actions
}

fn action_menu(gs: &mut GameState) {
    let actions = available_actions(gs);

    if actions.is_empty() {
        println!("\n  Nothing to do here. The system is empty.");
        pause();
        return;
    }

    display_header("Actions");

    for (i, action) in actions.iter().enumerate() {
        println!("  {}) {}", i + 1, action.label());
    }
    println!("  0) Back");
    println!();

    let input = prompt("  > ");
    let idx: usize = match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= actions.len() => n - 1,
        _ => return,
    };

    let intent = actions[idx];

    // Direct mechanical screens for Trade, Repair, Resupply.
    // Everything else goes through the encounter pipeline.
    match intent {
        PlayerIntent::Trade => trade_screen(gs),
        PlayerIntent::Resupply => resupply_screen(gs),
        PlayerIntent::Repair => repair_screen(gs),
        PlayerIntent::GatherRumors => rumors_screen(gs),
        _ => run_intent_encounter(gs, intent),
    }
}

// ---------------------------------------------------------------------------
// Trade screen — buy and sell goods
// ---------------------------------------------------------------------------

fn trade_screen(gs: &mut GameState) {
    let economy = match gs.current_economy().cloned() {
        Some(e) => e,
        None => {
            println!("\n  No trade facilities at this location.");
            pause();
            return;
        }
    };

    loop {
        clear_screen();
        display_header("Trade Post");

        let ship = &gs.journey.ship;
        let total_cargo: u32 = ship.cargo.values().sum();

        println!();
        println!("  Credits: {:.0}    Cargo: {}/{}",
            gs.journey.resources, total_cargo, ship.cargo_capacity);
        println!("  Fuel: {:.0}/{:.0}    Supplies: {:.0}/{:.0}",
            ship.fuel, ship.fuel_capacity, ship.supplies, ship.supply_capacity);

        // Sell section — show what the player has.
        let sellable: Vec<(String, u32)> = ship.cargo.iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();

        if !sellable.is_empty() {
            println!("\n  ── SELL ─────────────────────────");
            for (i, (name, qty)) in sellable.iter().enumerate() {
                // Try to match to a trade good for pricing.
                let price = match_trade_good(name)
                    .map(|g| economy.sell_price(g))
                    .unwrap_or(10.0); // fallback for unique/mission items
                println!("  s{}) {} (x{}) — {:.0} cr/unit",
                    i + 1, name, qty, price);
            }
        }

        // Buy section — show available goods.
        println!("\n  ── BUY ──────────────────────────");
        let goods = TradeGood::all();
        for (i, good) in goods.iter().enumerate() {
            let avail = economy.availability(*good);
            if avail == Availability::Unavailable {
                continue;
            }
            let price = economy.buy_price(*good);
            println!("  b{}) {} — {:.0} cr/unit ({})",
                i + 1, good.display_name(), price, avail);
        }

        // Essentials.
        println!("\n  ── ESSENTIALS ───────────────────");
        println!("  f) Fuel — {:.1} cr/unit  [{:.0} to fill]",
            economy.fuel_price,
            (ship.fuel_capacity - ship.fuel) * economy.fuel_price as f32);
        println!("  u) Supplies — {:.1} cr/unit  [{:.0} to fill]",
            economy.supply_price,
            (ship.supply_capacity - ship.supplies) * economy.supply_price as f32);

        println!("\n  0) Leave trade post");
        println!();

        let input = prompt("  > ");
        let input = input.trim().to_lowercase();

        if input == "0" || input == "leave" || input == "back" {
            return;
        }

        // Fuel purchase.
        if input == "f" || input == "fuel" {
            buy_fuel(gs, &economy);
            continue;
        }

        // Supply purchase.
        if input == "u" || input == "supplies" || input == "supply" {
            buy_supplies(gs, &economy);
            continue;
        }

        // Sell: s1, s2, ...
        if input.starts_with('s') {
            if let Ok(idx) = input[1..].parse::<usize>() {
                if idx >= 1 && idx <= sellable.len() {
                    let (name, qty) = &sellable[idx - 1];
                    let price = match_trade_good(name)
                        .map(|g| economy.sell_price(g))
                        .unwrap_or(10.0);
                    sell_cargo(gs, name, *qty, price);
                }
            }
            continue;
        }

        // Buy: b1, b2, ...
        if input.starts_with('b') {
            if let Ok(idx) = input[1..].parse::<usize>() {
                let available_goods: Vec<&TradeGood> = goods.iter()
                    .filter(|g| economy.availability(**g) != Availability::Unavailable)
                    .collect();
                if idx >= 1 && idx <= available_goods.len() {
                    let good = *available_goods[idx - 1];
                    let price = economy.buy_price(good);
                    buy_cargo(gs, good, price);
                }
            }
            continue;
        }
    }
}

fn buy_fuel(gs: &mut GameState, economy: &SystemEconomy) {
    let ship = &gs.journey.ship;
    let space = ship.fuel_capacity - ship.fuel;
    if space < 0.5 {
        println!("  Fuel tanks are full.");
        pause();
        return;
    }

    let price_per = economy.fuel_price as f64;
    let max_affordable = (gs.journey.resources / price_per) as f32;

    println!("\n  Fuel: {:.0}/{:.0}  |  Price: {:.1} cr/unit  |  Credits: {:.0}",
        ship.fuel, ship.fuel_capacity, price_per, gs.journey.resources);
    println!("  1) Fill tank ({:.0} units, {:.0} cr)", space, space as f64 * price_per);
    println!("  2) Buy 10 units ({:.0} cr)", 10.0_f64.min(space as f64) * price_per);
    println!("  3) Buy 20 units ({:.0} cr)", 20.0_f64.min(space as f64) * price_per);
    println!("  0) Cancel");
    println!();

    let input = prompt("  > ");
    let amount = match input.as_str() {
        "1" => space,
        "2" => 10.0_f32.min(space),
        "3" => 20.0_f32.min(space),
        _ => return,
    };

    let cost = amount as f64 * price_per;
    if cost > gs.journey.resources {
        let can_afford = max_affordable;
        if can_afford < 1.0 {
            println!("  Can't afford any fuel.");
            pause();
            return;
        }
        println!("  Can only afford {:.0} units. Buy? (y/n)", can_afford);
        let confirm = prompt("  > ");
        if confirm.trim().to_lowercase() != "y" {
            return;
        }
        gs.journey.ship.fuel += can_afford;
        gs.journey.resources -= can_afford as f64 * price_per;
        println!("  Bought {:.0} units of fuel. -{:.0} credits.", can_afford, can_afford as f64 * price_per);
    } else {
        gs.journey.ship.fuel += amount;
        gs.journey.resources -= cost;
        println!("  Bought {:.0} units of fuel. -{:.0} credits.", amount, cost);
    }
    pause();
}

fn buy_supplies(gs: &mut GameState, economy: &SystemEconomy) {
    let ship = &gs.journey.ship;
    let space = ship.supply_capacity - ship.supplies;
    if space < 0.5 {
        println!("  Supply stores are full.");
        pause();
        return;
    }

    let price_per = economy.supply_price as f64;
    let max_affordable = (gs.journey.resources / price_per) as f32;

    println!("\n  Supplies: {:.0}/{:.0}  |  Price: {:.1} cr/unit  |  Credits: {:.0}",
        ship.supplies, ship.supply_capacity, price_per, gs.journey.resources);
    println!("  1) Fill stores ({:.0} units, {:.0} cr)", space, space as f64 * price_per);
    println!("  2) Buy 10 units ({:.0} cr)", 10.0_f64.min(space as f64) * price_per);
    println!("  3) Buy 20 units ({:.0} cr)", 20.0_f64.min(space as f64) * price_per);
    println!("  0) Cancel");
    println!();

    let input = prompt("  > ");
    let amount = match input.as_str() {
        "1" => space,
        "2" => 10.0_f32.min(space),
        "3" => 20.0_f32.min(space),
        _ => return,
    };

    let cost = amount as f64 * price_per;
    if cost > gs.journey.resources {
        let can_afford = max_affordable;
        if can_afford < 1.0 {
            println!("  Can't afford any supplies.");
            pause();
            return;
        }
        println!("  Can only afford {:.0} units. Buy? (y/n)", can_afford);
        let confirm = prompt("  > ");
        if confirm.trim().to_lowercase() != "y" {
            return;
        }
        gs.journey.ship.supplies += can_afford;
        gs.journey.resources -= can_afford as f64 * price_per;
        println!("  Bought {:.0} units of supplies. -{:.0} credits.", can_afford, can_afford as f64 * price_per);
    } else {
        gs.journey.ship.supplies += amount;
        gs.journey.resources -= cost;
        println!("  Bought {:.0} units of supplies. -{:.0} credits.", amount, cost);
    }
    pause();
}

fn sell_cargo(gs: &mut GameState, name: &str, qty: u32, price_per: f64) {
    println!("\n  {} — {} units @ {:.0} cr/unit", name, qty, price_per);
    println!("  Sell how many? (1-{}, or 'all')", qty);
    println!();

    let input = prompt("  > ");
    let amount: u32 = if input.trim().to_lowercase() == "all" {
        qty
    } else {
        match input.trim().parse::<u32>() {
            Ok(n) if n >= 1 && n <= qty => n,
            _ => return,
        }
    };

    let revenue = amount as f64 * price_per;
    gs.journey.resources += revenue;

    let remaining = qty - amount;
    if remaining == 0 {
        gs.journey.ship.cargo.remove(name);
    } else {
        gs.journey.ship.cargo.insert(name.to_string(), remaining);
    }

    println!("  Sold {} x{} for {:.0} credits.", name, amount, revenue);
    pause();
}

fn buy_cargo(gs: &mut GameState, good: TradeGood, price_per: f64) {
    let total_cargo: u32 = gs.journey.ship.cargo.values().sum();
    let free_space = gs.journey.ship.cargo_capacity - total_cargo;

    if free_space == 0 {
        println!("  Cargo hold is full.");
        pause();
        return;
    }

    let max_affordable = (gs.journey.resources / price_per) as u32;
    let max_buy = free_space.min(max_affordable);

    println!("\n  {} — {:.0} cr/unit  |  Hold space: {}  |  Credits: {:.0}",
        good.display_name(), price_per, free_space, gs.journey.resources);
    println!("  Buy how many? (1-{})", max_buy);
    println!();

    let input = prompt("  > ");
    let amount: u32 = match input.trim().parse::<u32>() {
        Ok(n) if n >= 1 && n <= max_buy => n,
        _ => return,
    };

    let cost = amount as f64 * price_per;
    gs.journey.resources -= cost;

    let name = good.display_name().to_string();
    let current = gs.journey.ship.cargo.get(&name).copied().unwrap_or(0);
    gs.journey.ship.cargo.insert(name.clone(), current + amount);

    println!("  Bought {} x{} for {:.0} credits.", name, amount, cost);
    pause();
}

/// Try to match a cargo item name to a TradeGood for pricing.
fn match_trade_good(name: &str) -> Option<TradeGood> {
    let lower = name.to_lowercase();
    if lower.contains("food") { return Some(TradeGood::Food); }
    if lower.contains("raw material") { return Some(TradeGood::RawMaterials); }
    if lower.contains("manufactured") { return Some(TradeGood::ManufacturedGoods); }
    if lower.contains("medical") { return Some(TradeGood::MedicalSupplies); }
    if lower.contains("construction") { return Some(TradeGood::ConstructionMaterials); }
    if lower.contains("fuel cell") || lower.contains("refined fuel") {
        return Some(TradeGood::RefinedFuelCells);
    }
    None
}

// ---------------------------------------------------------------------------
// Resupply screen — quick fuel + supplies
// ---------------------------------------------------------------------------

fn resupply_screen(gs: &mut GameState) {
    let economy = match gs.current_economy().cloned() {
        Some(e) => e,
        None => {
            println!("\n  No resupply facilities at this location.");
            pause();
            return;
        }
    };

    clear_screen();
    display_header("Resupply");

    let ship = &gs.journey.ship;
    println!();
    println!("  Fuel: {:.0}/{:.0}  |  Supplies: {:.0}/{:.0}  |  Credits: {:.0}",
        ship.fuel, ship.fuel_capacity, ship.supplies, ship.supply_capacity,
        gs.journey.resources);
    println!();

    let fuel_need = ship.fuel_capacity - ship.fuel;
    let supply_need = ship.supply_capacity - ship.supplies;
    let fuel_cost = fuel_need as f64 * economy.fuel_price as f64;
    let supply_cost = supply_need as f64 * economy.supply_price as f64;
    let total = fuel_cost + supply_cost;

    println!("  Fuel price: {:.1} cr/unit  |  Supply price: {:.1} cr/unit",
        economy.fuel_price, economy.supply_price);
    println!();
    println!("  1) Fill everything ({:.0} cr)", total);
    println!("  2) Fuel only ({:.0} cr for {:.0} units)", fuel_cost, fuel_need);
    println!("  3) Supplies only ({:.0} cr for {:.0} units)", supply_cost, supply_need);
    println!("  4) Visit trade post (buy/sell goods)");
    println!("  0) Back");
    println!();

    let input = prompt("  > ");
    match input.as_str() {
        "1" => {
            if total > gs.journey.resources {
                println!("  Can't afford full resupply. Need {:.0} cr, have {:.0}.",
                    total, gs.journey.resources);
                pause();
                return;
            }
            gs.journey.ship.fuel = gs.journey.ship.fuel_capacity;
            gs.journey.ship.supplies = gs.journey.ship.supply_capacity;
            gs.journey.resources -= total;
            println!("  Tanks full. Stores stocked. -{:.0} credits.", total);
            pause();
        }
        "2" => buy_fuel(gs, &economy),
        "3" => buy_supplies(gs, &economy),
        "4" => trade_screen(gs),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Repair screen — pay credits to fix modules
// ---------------------------------------------------------------------------

fn repair_screen(gs: &mut GameState) {
    let infra = gs.current_location()
        .map(|l| l.infrastructure)
        .unwrap_or(InfrastructureLevel::None);
    let infra_rank = match infra {
        InfrastructureLevel::None => { println!("  No repair facilities."); pause(); return; }
        InfrastructureLevel::Outpost => 1,
        InfrastructureLevel::Colony => 2,
        InfrastructureLevel::Established => 3,
        InfrastructureLevel::Hub => 4,
        InfrastructureLevel::Capital => 5,
    };

    loop {
        clear_screen();
        display_header("Repair Bay");
        println!();

        // Cost per 0.1 condition restored, based on infrastructure.
        let cost_per_tick = match infra_rank {
            1 => 50.0,   // Outpost — expensive, limited
            2 => 35.0,   // Colony
            3 => 25.0,   // Established
            _ => 15.0,   // Hub/Capital — cheap, professional
        };

        let modules = [
            ("Engine", gs.journey.ship.modules.engine.condition, &gs.journey.ship.modules.engine.variant),
            ("Sensors", gs.journey.ship.modules.sensors.condition, &gs.journey.ship.modules.sensors.variant),
            ("Comms", gs.journey.ship.modules.comms.condition, &gs.journey.ship.modules.comms.variant),
            ("Weapons", gs.journey.ship.modules.weapons.condition, &gs.journey.ship.modules.weapons.variant),
            ("Life Support", gs.journey.ship.modules.life_support.condition, &gs.journey.ship.modules.life_support.variant),
        ];

        let hull = gs.journey.ship.hull_condition;

        println!("  Hull: {:.0}%    Credits: {:.0}", hull * 100.0, gs.journey.resources);
        println!("  Repair rate: {:.0} cr per 10% condition", cost_per_tick);
        println!();

        let mut repair_options: Vec<(usize, &str, f32)> = Vec::new(); // (idx, name, condition)

        for (i, (name, cond, variant)) in modules.iter().enumerate() {
            let status = if *cond >= 0.95 { "OK" } else { "" };
            println!("  {}) {} ({}) — {:.0}% {}",
                i + 1, name, variant, cond * 100.0, status);
            if *cond < 0.95 {
                let ticks_needed = ((1.0 - cond) / 0.1).ceil();
                let repair_cost = ticks_needed as f64 * cost_per_tick;
                println!("     Full repair: {:.0} cr", repair_cost);
                repair_options.push((i, name, *cond));
            }
        }

        if hull < 0.95 {
            let hull_ticks = ((1.0 - hull) / 0.1).ceil();
            let hull_cost = hull_ticks as f64 * cost_per_tick * 1.5; // hull costs more
            println!("\n  h) Hull repair — {:.0}% → 100% ({:.0} cr)", hull * 100.0, hull_cost);
        }

        if repair_options.is_empty() && hull >= 0.95 {
            println!("\n  Ship is in good shape. No repairs needed.");
            pause();
            return;
        }

        println!("\n  a) Repair all systems");
        println!("  0) Back");
        println!();

        let input = prompt("  > ");
        let input = input.trim().to_lowercase();

        if input == "0" || input == "back" {
            return;
        }

        if input == "h" && hull < 0.95 {
            let hull_ticks = ((1.0 - hull) / 0.1).ceil();
            let hull_cost = hull_ticks as f64 * cost_per_tick * 1.5;
            if hull_cost > gs.journey.resources {
                println!("  Can't afford hull repair.");
                pause();
                continue;
            }
            gs.journey.ship.hull_condition = 1.0;
            gs.journey.resources -= hull_cost;
            println!("  Hull repaired. -{:.0} credits.", hull_cost);
            pause();
            continue;
        }

        if input == "a" {
            let mut total_cost = 0.0;
            for (_, _, cond) in &repair_options {
                let ticks = ((1.0 - cond) / 0.1).ceil();
                total_cost += ticks as f64 * cost_per_tick;
            }
            if hull < 0.95 {
                let hull_ticks = ((1.0 - hull) / 0.1).ceil();
                total_cost += hull_ticks as f64 * cost_per_tick * 1.5;
            }
            if total_cost > gs.journey.resources {
                println!("  Can't afford full repair. Need {:.0} cr.", total_cost);
                pause();
                continue;
            }
            gs.journey.ship.modules.engine.condition = 1.0;
            gs.journey.ship.modules.sensors.condition = 1.0;
            gs.journey.ship.modules.comms.condition = 1.0;
            gs.journey.ship.modules.weapons.condition = 1.0;
            gs.journey.ship.modules.life_support.condition = 1.0;
            gs.journey.ship.hull_condition = 1.0;
            gs.journey.resources -= total_cost;
            println!("  All systems repaired. -{:.0} credits.", total_cost);
            pause();
            return;
        }

        // Individual module repair: 1-5
        if let Ok(idx) = input.parse::<usize>() {
            if idx >= 1 && idx <= 5 {
                let i = idx - 1;
                let module = match i {
                    0 => &mut gs.journey.ship.modules.engine,
                    1 => &mut gs.journey.ship.modules.sensors,
                    2 => &mut gs.journey.ship.modules.comms,
                    3 => &mut gs.journey.ship.modules.weapons,
                    4 => &mut gs.journey.ship.modules.life_support,
                    _ => unreachable!(),
                };
                if module.condition >= 0.95 {
                    println!("  Already in good shape.");
                    pause();
                    continue;
                }
                let ticks = ((1.0 - module.condition) / 0.1).ceil();
                let cost = ticks as f64 * cost_per_tick;
                if cost > gs.journey.resources {
                    println!("  Can't afford repair. Need {:.0} cr.", cost);
                    pause();
                    continue;
                }
                module.condition = 1.0;
                gs.journey.resources -= cost;
                println!("  Module repaired. -{:.0} credits.", cost);
                pause();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rumors screen — gather actionable information
// ---------------------------------------------------------------------------

fn rumors_screen(gs: &mut GameState) {
    let (location, system) = match (gs.current_location().cloned(), gs.current_system().clone()) {
        (Some(loc), sys) => (loc, sys),
        _ => {
            println!("\n  You need to be docked to gather rumors.");
            pause();
            return;
        }
    };

    if !location.services.contains(&LocationService::Rumors) {
        println!("\n  No one here to listen to.");
        pause();
        return;
    }

    // Time cost: 2-4 hours depending on infrastructure.
    let hours = match location.infrastructure {
        InfrastructureLevel::Capital | InfrastructureLevel::Hub => 2.0,
        InfrastructureLevel::Established => 3.0,
        _ => 4.0,
    };

    clear_screen();

    display_header(&format!("Rumors — {}", location.name));
    println!();
    println!("  You spend some time listening around the station.");
    println!("  ({:.0} hours)", hours);
    println!();

    // Build rumor context.
    let ctx = RumorContext {
        galaxy: &gs.galaxy,
        journey: &gs.journey,
        recent_tick_events: &gs.recent_tick_events,
        location: &location,
        system: &system,
    };

    let mut rumors = generate_rumors(&ctx, &mut gs.rng);

    if rumors.is_empty() {
        println!("  Nothing interesting. The station is quiet.");
        pause();

        // Advance time even on empty results.
        let galactic_hours = hours * system.time_factor;
        gs.journey.time.personal_days += hours / 24.0;
        gs.journey.time.galactic_days += galactic_hours / 24.0;
        return;
    }

    // --- Optional LLM flavor pass ---
    // When the LLM is available, try to flavor each rumor's delivery.
    // Falls back to template text silently on failure.
    if gs.llm_config.is_available() {
        for rumor in rumors.iter_mut() {
            // Pick a source type based on rumor category.
            let source = match rumor.category {
                starbound_core::rumor::RumorCategory::ContractLead => {
                    if let RumorContent::ContractLead { npc_name: Some(ref name), .. } = rumor.content {
                        RumorSource::FactionContact {
                            name: name.clone(),
                            title: "contact".into(),
                        }
                    } else {
                        RumorSource::Overheard
                    }
                }
                starbound_core::rumor::RumorCategory::FactionIntel => RumorSource::NewsTerminal,
                starbound_core::rumor::RumorCategory::TradeTip => RumorSource::DockWorker,
                _ => RumorSource::Overheard,
            };

            if let Some(flavored) = flavor_rumor(
                &gs.llm_config,
                &rumor.summary,
                &source,
                &location.name,
            ) {
                rumor.display_text = flavored;
            }
        }
    }

    // Display each rumor.
    for (i, rumor) in rumors.iter().enumerate() {
        let reliability_str = if rumor.reliability >= 0.8 {
            "high"
        } else if rumor.reliability >= 0.6 {
            "moderate"
        } else {
            "low"
        };

        println!("  {}) {}", i + 1, rumor.display_text);
        println!("     [{}  —  reliability: {}]", rumor.category, reliability_str);
        println!();
    }

    println!("  Select a rumor to note it (or 0 to continue):");
    println!();

    let input = prompt("  > ");

    // Handle selection — noting a rumor adds it to the journal.
    match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= rumors.len() => {
            let chosen = &rumors[n - 1];
            println!();
            println!("  Noted. {} logged.", chosen.category);

            // Show mechanical detail for trade tips.
            if let RumorContent::TradeTip {
                good, estimated_spread, ..
            } = &chosen.content {
                println!("  (Estimated profit: ~{:.0} credits/unit for {})", estimated_spread, good);
            }

            // Show NPC reference for contract leads.
            if let RumorContent::ContractLead {
                npc_name: Some(ref name), estimated_reward, ..
            } = &chosen.content {
                println!("  (Talk to {} — estimated reward: ~{:.0} credits)", name, estimated_reward);
            }

            // Add all rumors to the journal (the selected one marked as "noted").
            for rumor in rumors {
                gs.journey.discovered_rumors.push(rumor);
            }
        }
        _ => {
            // Still add rumors even if none selected — player heard them.
            for rumor in rumors {
                gs.journey.discovered_rumors.push(rumor);
            }
        }
    }

    // Advance time.
    let galactic_hours = hours * system.time_factor;
    gs.journey.time.personal_days += hours / 24.0;
    gs.journey.time.galactic_days += galactic_hours / 24.0;

    // Prune old rumors (keep last 30).
    if gs.journey.discovered_rumors.len() > 30 {
        let drain = gs.journey.discovered_rumors.len() - 30;
        gs.journey.discovered_rumors.drain(..drain);
    }

    pause();
}

fn run_intent_encounter(gs: &mut GameState, intent: PlayerIntent) {
    let fired = try_encounter(gs, intent.into(), None, None);

    if !fired {
        // Custom silence messages per intent.
        clear_screen();
        display_header(intent.label());
        println!();

        let msg = match intent {
            PlayerIntent::Trade => "No one here is interested in trading right now.",
            PlayerIntent::Investigate => "Your sensors find nothing worth investigating at the moment.",
            PlayerIntent::Repair => "No repair facilities available at this system.",
            PlayerIntent::Resupply => "Supplies aren't available here.",
            PlayerIntent::Scan => "Your sensors sweep the system but find nothing unusual.",
            PlayerIntent::Recruit => "No one here is looking for work.",
            PlayerIntent::Rest => "The crew takes a moment to breathe.",
            PlayerIntent::Smuggle => "No opportunity for that kind of work here.",
            PlayerIntent::Negotiate => "There's no one to negotiate with.",
            PlayerIntent::GatherRumors => "No one here to listen to.",
        };

        for line in wrap_text(msg, 60) {
            println!("  {}", line);
        }

        pause();
    }
}

// ---------------------------------------------------------------------------
// People menu — NPC interactions
// ---------------------------------------------------------------------------

fn people_menu(gs: &mut GameState) {
    let npcs = gs.npcs_here();

    if npcs.is_empty() {
        clear_screen();
        display_header("People");
        println!("  No one of note here.");
        pause();
        return;
    }

    clear_screen();
    display_header("People");
    println!();

    for (i, npc) in npcs.iter().enumerate() {
        let faction_str = gs.npc_faction_name(npc);
        let tier = npc.disposition_tier();

        // Disposition hint — subtle, not a number.
        let disposition_hint = match tier {
            DispositionTier::Hostile => " — hostile",
            DispositionTier::Cold => " — cold",
            DispositionTier::Neutral => "",
            DispositionTier::Warm => " — seems friendly",
            DispositionTier::Friendly => " — warmly disposed",
            DispositionTier::Trusted => " — trusts you",
        };

        // Species hint for synthetics.
        let species_hint = if npc.species.is_synthetic() {
            format!(" [{}]", npc.species.display_label())
        } else {
            String::new()
        };

        println!("  {}) {} — {} ({}){}{}", 
            i + 1, npc.name, npc.title, faction_str, species_hint, disposition_hint);
    }
    println!("  0) Back");
    println!();

    let input = prompt("  > ");
    let idx: usize = match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= npcs.len() => n - 1,
        _ => return,
    };

    let npc_id = npcs[idx].id;
    talk_to_npc(gs, npc_id);
}

fn talk_to_npc(gs: &mut GameState, npc_id: Uuid) {
    let npc_idx = match gs.npc_index(npc_id) {
        Some(i) => i,
        None => return,
    };

    loop {
        let npc = &gs.galaxy.npcs[npc_idx];

        // Gather context for the presentation layer.
        let has_turnable = gs.journey.active_contracts.iter()
            .any(|c| c.issuer_npc_id == npc_id && c.state == ContractState::ReadyToComplete);
        let ship_name = gs.journey.ship.name.clone();
        let system_name = gs.current_system().name.clone();
        let faction_name = gs.npc_faction_name(npc);

        // Build the full NPC presentation.
        let pres = build_npc_presentation(
            npc,
            has_turnable,
            &ship_name,
            &system_name,
            &faction_name,
            &gs.personality_expressions,
            &mut gs.rng,
        );

        clear_screen();
        display_header(&format!("{} — {}", npc.name, npc.title));
        println!();

        // Bio.
        for line in wrap_text(&npc.bio, 60) {
            println!("  {}", line);
        }
        println!();

        // Personality sketch.
        if !pres.personality_sketch.is_empty() {
            for line in wrap_text(&pres.personality_sketch, 60) {
                println!("  {}", line);
            }
            println!();
        }

        println!("{}", THIN_DIVIDER);
        println!();

        // Greeting.
        for line in wrap_text(&pres.greeting, 60) {
            println!("  {}", line);
        }

        // Memory of previous interaction.
        if let Some(ref memory) = pres.memory_line {
            println!();
            for line in wrap_text(memory, 60) {
                println!("  {}", line);
            }
        }

        println!();

        // Menu options.
        for (i, opt) in pres.options.iter().enumerate() {
            println!("  {}) {}", i + 1, opt.label);
        }
        println!();

        let input = prompt("  > ");
        let choice: usize = input.parse::<usize>().unwrap_or(0);

        if choice == 0 || choice > pres.options.len() {
            // Record the visit.
            let npc = &mut gs.galaxy.npcs[npc_idx];
            npc.record_interaction(
                "visited briefly",
                gs.journey.time.galactic_days,
                0.0,
            );
            return;
        }

        match &pres.options[choice - 1].action {
            NpcAction::AskAboutWork => {
                let npc = &gs.galaxy.npcs[npc_idx];
                if !npc.will_offer_contracts() {
                    // Disposition too low for contracts.
                    let refusal = contract_refusal_text(
                        npc, &ship_name, &system_name, &faction_name,
                        &mut gs.rng,
                    );
                    clear_screen();
                    display_header("No Work Available");
                    println!();
                    for line in wrap_text(&refusal, 60) {
                        println!("  {}", line);
                    }
                    pause();
                } else {
                    offer_contracts(gs, npc_idx);
                }
            }
            NpcAction::TurnInContract => {
                turn_in_contract(gs, npc_idx);
            }
            NpcAction::AskAboutArea => {
                npc_ask_about_area(gs, npc_idx);
            }
            NpcAction::AskAboutConnection(_) => {
                npc_connections_menu(gs, npc_idx);
            }
            NpcAction::Leave => {
                // Farewell.
                let npc = &gs.galaxy.npcs[npc_idx];
                let farewell = farewell_text(
                    npc, &ship_name, &system_name, &faction_name,
                    &mut gs.rng,
                );
                println!();
                for line in wrap_text(&farewell, 60) {
                    println!("  {}", line);
                }
                pause();
                return;
            }
        }
    }
}

/// NPC "Ask about the area" — knowledge-driven, personality-shaped.
fn npc_ask_about_area(gs: &mut GameState, npc_idx: usize) {
    let npc = &gs.galaxy.npcs[npc_idx];
    let npc_id = npc.id;
    let ship_name = gs.journey.ship.name.clone();
    let system_name = gs.current_system().name.clone();
    let faction_name = gs.npc_faction_name(npc);

    // Get what we've already shared with this NPC.
    let already_shared = gs.npc_shared_knowledge
        .get(&npc_id)
        .cloned()
        .unwrap_or_default();

    let area = ask_about_area(
        npc,
        &system_name,
        &ship_name,
        &faction_name,
        &gs.galaxy.npcs,
        &already_shared,
        &mut gs.rng,
    );

    clear_screen();
    display_header("Local Intel");
    println!();

    // Framing line.
    if !area.framing.is_empty() {
        for line in wrap_text(&area.framing, 60) {
            println!("  {}", line);
        }
        println!();
    }

    if area.items.is_empty() {
        let npc = &gs.galaxy.npcs[npc_idx];
        if npc.disposition_tier() <= DispositionTier::Cold {
            println!("  {} doesn't seem inclined to share anything.", npc.name);
        } else {
            println!("  {} has nothing new to tell you.", npc.name);
        }
    } else {
        // Display each knowledge item.
        let mut newly_shared = Vec::new();
        for item in &area.items {
            for line in wrap_text(&item.delivered, 60) {
                println!("  {}", line);
            }
            println!();
            newly_shared.push(item.raw.clone());
        }

        // Record what was shared.
        gs.npc_shared_knowledge
            .entry(npc_id)
            .or_insert_with(Vec::new)
            .extend(newly_shared);
    }

    // Connection mention.
    if let Some(ref mention) = area.connection_mention {
        println!("{}", THIN_DIVIDER);
        println!();
        for line in wrap_text(mention, 60) {
            println!("  {}", line);
        }
    }

    // Record the interaction.
    let npc = &mut gs.galaxy.npcs[npc_idx];
    npc.record_interaction(
        "asked about the area",
        gs.journey.time.galactic_days,
        0.02, // Small positive — showing interest.
    );

    pause();
}

/// NPC connections sub-menu — ask about people this NPC knows.
fn npc_connections_menu(gs: &mut GameState, npc_idx: usize) {
    let npc = &gs.galaxy.npcs[npc_idx];
    let npc_name = npc.name.clone();
    let connections: Vec<(Uuid, NpcRelationType, String)> = npc.connections.iter()
        .map(|c| (c.npc_id, c.relationship, c.context.clone()))
        .collect();

    if connections.is_empty() {
        clear_screen();
        display_header("Contacts");
        println!();
        println!("  {} doesn't mention anyone.", npc_name);
        pause();
        return;
    }

    clear_screen();
    display_header(&format!("{}'s Contacts", npc_name));
    println!();

    // Build list of connected NPCs we can display.
    // Use direct field access to avoid borrowing all of `gs` through methods.
    let mut display_conns: Vec<(usize, String, String, NpcRelationType)> = Vec::new();

    for (i, (conn_id, rel_type, _context)) in connections.iter().enumerate() {
        if let Some(connected) = gs.galaxy.npcs.iter().find(|n| n.id == *conn_id) {
            let loc = if connected.home_system_id == gs.journey.current_system {
                connected.home_location_id
                    .and_then(|loc_id| {
                        gs.galaxy.systems.iter()
                            .find(|s| s.id == connected.home_system_id)
                            .and_then(|s| s.locations.iter().find(|l| l.id == loc_id))
                            .map(|l| l.name.clone())
                    })
                    .unwrap_or_else(|| "Unknown".into())
            } else {
                let sys_name = gs.galaxy.systems.iter()
                    .find(|s| s.id == connected.home_system_id)
                    .map(|s| s.name.as_str())
                    .unwrap_or("Unknown");
                format!("{} system", sys_name)
            };
            display_conns.push((i, connected.name.clone(), loc, *rel_type));
        }
    }

    for (i, (_orig_idx, name, loc, rel)) in display_conns.iter().enumerate() {
        println!("  {}) {} — {} ({})", i + 1, name, rel, loc);
    }
    println!("  0) Back");
    println!();

    let input = prompt("  > ");
    let choice: usize = match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= display_conns.len() => n - 1,
        _ => return,
    };

    let (orig_idx, _name, _loc, _rel) = &display_conns[choice];
    let (conn_npc_id, _, _) = &connections[*orig_idx];

    // Gather all data needed for the connection query.
    // Clone/extract what we need to avoid holding &self borrows across &mut gs.rng.
    let current_system_name = gs.galaxy.systems.iter()
        .find(|s| s.id == gs.journey.current_system)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "Unknown".into());
    let ship_name = gs.journey.ship.name.clone();

    // Look up the connected NPC — direct field access.
    let connected_npc_idx = match gs.galaxy.npcs.iter().position(|n| n.id == *conn_npc_id) {
        Some(i) => i,
        None => return,
    };

    let connected_sys_name = gs.galaxy.systems.iter()
        .find(|s| s.id == gs.galaxy.npcs[connected_npc_idx].home_system_id)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| "Unknown".into());

    let faction_name = gs.galaxy.npcs[npc_idx].faction_id
        .and_then(|fid| gs.galaxy.factions.iter().find(|f| f.id == fid))
        .map(|f| f.name.clone())
        .unwrap_or_else(|| "Independent".into());

    // Find the connection struct.
    let connection_clone = gs.galaxy.npcs[npc_idx].connections.iter()
        .find(|c| c.npc_id == *conn_npc_id)
        .cloned();
    let connection = match connection_clone {
        Some(c) => c,
        None => return,
    };

    let info = ask_about_connection(
        &gs.galaxy.npcs[npc_idx],
        &connection,
        &gs.galaxy.npcs[connected_npc_idx],
        &current_system_name,
        &connected_sys_name,
        &ship_name,
        &faction_name,
        &mut gs.rng,
    );

    clear_screen();
    display_header(&format!("About {}", info.name));
    println!();

    println!("  {} — {}", info.name, info.title);
    println!("  Located: {}", info.location);
    println!();
    for line in wrap_text(&info.description, 60) {
        println!("  {}", line);
    }

    // Record the interaction.
    let npc = &mut gs.galaxy.npcs[npc_idx];
    npc.record_interaction(
        format!("asked about {}", info.name),
        gs.journey.time.galactic_days,
        0.01,
    );

    pause();
}

fn offer_contracts(gs: &mut GameState, npc_idx: usize) {
    let npc = &gs.galaxy.npcs[npc_idx];
    let npc_id = npc.id;
    let npc_name = npc.name.clone();

    // Check disposition — cold NPCs won't offer work.
    if !npc.will_offer_contracts() {
        let ship_name = gs.journey.ship.name.clone();
        let system_name = gs.current_system().name.clone();
        let faction_name = gs.npc_faction_name(npc);
        let refusal = contract_refusal_text(
            npc, &ship_name, &system_name, &faction_name,
            &mut gs.rng,
        );
        clear_screen();
        display_header("Contracts");
        println!();
        for line in wrap_text(&refusal, 60) {
            println!("  {}", line);
        }
        pause();
        return;
    }

    // Check if the player already has an active contract from this NPC.
    let already_working = gs.journey.active_contracts.iter()
        .any(|c| c.issuer_npc_id == npc_id
            && (c.state == ContractState::Active || c.state == ContractState::ReadyToComplete));

    if already_working {
        clear_screen();
        display_header("Contracts");
        println!();
        println!("  \"You've already got a job from me. Finish that first.\"");
        pause();
        return;
    }

    // Generate a contract based on faction category.
    // Disposition affects reward: warm+ NPCs offer better pay.
    let contract = generate_contract_for_npc(gs, npc_idx);

    let contract = match contract {
        Some(c) => c,
        None => {
            clear_screen();
            display_header("Contracts");
            println!();
            println!("  \"Nothing right now. Check back later.\"");
            pause();
            return;
        }
    };

    // Apply disposition bonus to reward.
    let npc = &gs.galaxy.npcs[npc_idx];
    let tier = npc.disposition_tier();
    let reward_multiplier = match tier {
        DispositionTier::Neutral => 1.0,
        DispositionTier::Warm => 1.15,
        DispositionTier::Friendly => 1.3,
        DispositionTier::Trusted => 1.5,
        _ => 1.0,
    };
    let mut contract = contract;
    contract.reward_credits *= reward_multiplier;

    // Display the offer.
    clear_screen();
    display_header("Contract Offered");
    println!();

    println!("  {}", contract.title);
    println!();
    for line in wrap_text(&contract.description, 60) {
        println!("  {}", line);
    }
    println!();

    let dest_sys_name = gs.system_name(contract.destination_system_id).to_string();
    let dest_display = if let Some(loc_id) = contract.destination_location_id {
        let loc_name = gs.galaxy.systems.iter()
            .find(|s| s.id == contract.destination_system_id)
            .and_then(|s| s.locations.iter().find(|l| l.id == loc_id))
            .map(|l| l.name.as_str())
            .unwrap_or("unknown");
        format!("{} ({} system)", loc_name, dest_sys_name)
    } else {
        dest_sys_name
    };
    let type_label = match contract.contract_type {
        ContractType::Delivery => "Delivery",
        ContractType::Retrieval => "Retrieval",
        ContractType::Investigation => "Investigation",
    };
    println!("  Destination: {}", dest_display);
    println!("  Type: {}", type_label);
    println!("  Reward: {:.0} credits", contract.reward_credits);
    if reward_multiplier > 1.0 {
        println!("  (Better terms — {} regards you well.)", npc_name);
    }
    if let Some((ref cargo, qty)) = contract.cargo_given {
        println!("  Cargo provided: {} x{}", cargo, qty);
    }
    if contract.cargo_given.is_none() {
        if let Some((ref cargo, qty)) = contract.cargo_required {
            println!("  Must retrieve: {} x{}", cargo, qty);
        }
    }
    println!();
    println!("  1) Accept");
    println!("  2) Decline");
    println!();

    let input = prompt("  > ");
    match input.as_str() {
        "1" | "accept" => {
            // Place cargo in hold.
            if let Some((ref cargo_name, qty)) = contract.cargo_given {
                let current = gs.journey.ship.cargo.get(cargo_name).copied().unwrap_or(0);
                let total_cargo: u32 = gs.journey.ship.cargo.values().sum();
                if total_cargo + qty > gs.journey.ship.cargo_capacity {
                    println!("  Not enough cargo space. Need {} free units.", qty);
                    pause();
                    return;
                }
                gs.journey.ship.cargo.insert(cargo_name.clone(), current + qty);
            }

            let mut accepted = contract;
            accepted.state = ContractState::Active;
            let title_clone = accepted.title.clone();
            gs.journey.active_contracts.push(accepted);

            // Record interaction.
            let npc = &mut gs.galaxy.npcs[npc_idx];
            npc.record_interaction(
                format!("accepted contract: {}", title_clone),
                gs.journey.time.galactic_days,
                0.05,
            );

            clear_screen();
            println!();
            // Personality-shaped acceptance.
            let boldness = gs.galaxy.npcs[npc_idx].personality.boldness;
            if boldness > 0.6 {
                println!("  \"Good. I like someone who doesn't hesitate.\"");
            } else if boldness < 0.4 {
                println!("  \"Good. Be careful out there.\"");
            } else {
                println!("  \"Good. Don't let me down.\"");
            }
            println!();
            println!("  Contract accepted. Check your contracts log.");
            pause();
        }
        _ => {
            // Record the decline.
            let npc = &mut gs.galaxy.npcs[npc_idx];
            npc.record_interaction(
                "declined a contract",
                gs.journey.time.galactic_days,
                -0.02,
            );

            let warmth = npc.personality.warmth;
            if warmth > 0.6 {
                println!("  \"No worries. Offer stands if you change your mind.\"");
            } else {
                println!("  \"Your call.\"");
            }
            pause();
        }
    }
}

fn generate_contract_for_npc(gs: &GameState, npc_idx: usize) -> Option<Contract> {
    let npc = &gs.galaxy.npcs[npc_idx];

    // Find a destination system that isn't the NPC's home.
    let home_id = npc.home_system_id;
    let connections = gs.galaxy.connections.iter()
        .filter(|c| c.system_a == home_id || c.system_b == home_id)
        .collect::<Vec<_>>();

    if connections.is_empty() {
        return None;
    }

    // Pick a connected system as destination (deterministic from NPC id).
    let conn_idx = (npc.id.as_u128() as usize) % connections.len();
    let conn = &connections[conn_idx];
    let dest_sys_id = if conn.system_a == home_id { conn.system_b } else { conn.system_a };
    let dest_sys_name = gs.system_name(dest_sys_id).to_string();

    // Find the primary dockable location at the destination.
    let dest_system = gs.galaxy.systems.iter().find(|s| s.id == dest_sys_id);
    let dest_location = dest_system.and_then(|sys| {
        sys.locations.iter()
            .filter(|l| l.services.contains(&LocationService::Docking))
            .max_by_key(|l| l.infrastructure)
    });
    let dest_loc_id = dest_location.map(|l| l.id);
    let dest_loc_name = dest_location
        .map(|l| l.name.as_str())
        .unwrap_or(&dest_sys_name)
        .to_string();

    // Determine faction category.
    let category = npc.faction_id
        .and_then(|fid| gs.galaxy.factions.iter().find(|f| f.id == fid))
        .map(|f| f.category);

    // Deterministic type selection: hash NPC ID + galactic day (30-day window)
    // so the same NPC offers different types over time.
    let day_window = (gs.journey.time.galactic_days / 30.0) as u128;
    let type_seed = npc.id.as_u128().wrapping_add(day_window);

    // Each faction category has weighted type options.
    // (contract_type_index: 0=delivery, 1=retrieval, 2=investigation)
    let type_options: &[usize] = match category {
        Some(FactionCategory::Guild)     => &[0, 0, 1, 1, 2],    // delivery/retrieval heavy
        Some(FactionCategory::Military)  => &[0, 2, 2, 2],       // investigation heavy
        Some(FactionCategory::Economic)  => &[0, 0, 0, 1],       // delivery heavy
        Some(FactionCategory::Criminal)  => &[0, 0, 2],          // delivery + investigation
        Some(FactionCategory::Religious) => &[1, 1, 2],          // retrieval + investigation
        Some(FactionCategory::Academic)  => &[2, 2, 1],          // investigation heavy
        Some(FactionCategory::Political) => &[2, 2, 0],          // investigation heavy
        None                             => &[0, 0, 1],          // delivery + retrieval
    };

    let chosen_type = type_options[(type_seed as usize) % type_options.len()];

    let mut contract = match (chosen_type, category) {
        // ---- DELIVERY contracts ----
        (0, Some(FactionCategory::Guild)) => {
            Contract::delivery(
                npc.id, npc.faction_id,
                format!("Deliver repair components to {}", dest_loc_name),
                format!(
                    "\"We've got a maintenance backlog at {}. \
                     Standard repair components — nothing exotic, but they \
                     need them yesterday. Deliver, get the dock master to sign off, \
                     and come back for your pay.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Repair components", 8, 200.0,
            )
        }
        (0, Some(FactionCategory::Military)) => {
            Contract::delivery(
                npc.id, npc.faction_id,
                format!("Transport sealed cargo to {}", dest_loc_name),
                format!(
                    "\"Military business. Sealed containers, don't ask what's inside. \
                     Take them to {} garrison, hand them over, bring back the receipt. \
                     Standard courier rate.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Sealed military cargo", 5, 250.0,
            )
        }
        (0, Some(FactionCategory::Economic)) => {
            Contract::delivery(
                npc.id, npc.faction_id,
                format!("Supply run to {}", dest_loc_name),
                format!(
                    "\"The market at {} is running short on manufactured goods. \
                     We've got a shipment ready to go. Deliver it, collect payment \
                     on delivery, and bring back our cut.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Manufactured goods", 12, 180.0,
            )
        }
        (0, Some(FactionCategory::Criminal)) => {
            Contract::delivery(
                npc.id, npc.faction_id,
                format!("Discreet delivery to {}", dest_sys_name),
                format!(
                    "\"I've got a package. It needs to get to {} without anyone \
                     asking questions. No manifests, no declarations. \
                     You handle it clean, I make it worth your while.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, "Unmarked cargo", 3, 300.0,
            )
        }
        (0, _) => {
            Contract::delivery(
                npc.id, npc.faction_id,
                format!("Courier run to {}", dest_loc_name),
                format!(
                    "\"Standard job. Take this cargo to {}, hand it off, \
                     come back with confirmation. Simple work, fair pay.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "General cargo", 6, 175.0,
            )
        }

        // ---- RETRIEVAL contracts ----
        (1, Some(FactionCategory::Guild)) => {
            Contract::retrieval(
                npc.id, npc.faction_id,
                format!("Retrieve salvaged parts from {}", dest_loc_name),
                format!(
                    "\"There's a set of reclaimed drive components at {}. \
                     Paid for, just need someone to pick them up and bring \
                     them back here. Should be straightforward.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Reclaimed drive parts", 6, 220.0,
            )
        }
        (1, Some(FactionCategory::Religious)) => {
            Contract::retrieval(
                npc.id, npc.faction_id,
                format!("Recover relics from {}", dest_loc_name),
                format!(
                    "\"An artifact of the Order was left at {} during \
                     the last evacuation. We need it returned. \
                     You'll know it when you see it — it resonates.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Order relics", 2, 200.0,
            )
        }
        (1, Some(FactionCategory::Economic)) => {
            Contract::retrieval(
                npc.id, npc.faction_id,
                format!("Collect payment from {}", dest_loc_name),
                format!(
                    "\"We have an outstanding balance at {}. \
                     They've got our goods sitting in their hold. \
                     Go collect — here's the manifest.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Collected goods", 8, 190.0,
            )
        }
        (1, Some(FactionCategory::Academic)) => {
            Contract::retrieval(
                npc.id, npc.faction_id,
                format!("Retrieve research samples from {}", dest_loc_name),
                format!(
                    "\"Our field team at {} has samples ready for analysis. \
                     Delicate materials — keep them sealed. \
                     The data is more valuable than the containers.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Research samples", 3, 250.0,
            )
        }
        (1, _) => {
            Contract::retrieval(
                npc.id, npc.faction_id,
                format!("Pick up cargo from {}", dest_loc_name),
                format!(
                    "\"There's a shipment waiting for us at {}. \
                     Go get it, bring it back. I'll make it worth your time.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Retrieved cargo", 5, 185.0,
            )
        }

        // ---- INVESTIGATION contracts ----
        (_, Some(FactionCategory::Military)) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Investigate activity near {}", dest_sys_name),
                format!(
                    "\"We've had reports of unusual activity in the {} system. \
                     Go there, assess the situation, and report back. \
                     Don't engage — just observe and document.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 280.0,
            )
        }
        (_, Some(FactionCategory::Academic)) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Survey anomalous readings at {}", dest_sys_name),
                format!(
                    "\"Our instruments have been picking up unusual readings \
                     from the {} system. We need someone on-site to \
                     confirm and characterize the source. Standard survey protocol.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 260.0,
            )
        }
        (_, Some(FactionCategory::Criminal)) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Scout {} for opportunities", dest_sys_name),
                format!(
                    "\"I need eyes at {}. Security patterns, docking schedules, \
                     who's coming and going. Routine business intelligence. \
                     Just look around and tell me what you see.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 300.0,
            )
        }
        (_, Some(FactionCategory::Political)) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Assess the political situation at {}", dest_sys_name),
                format!(
                    "\"There's been a shift in the local power balance at {}. \
                     I need an outside perspective — someone without ties. \
                     Go there, talk to people, and report back what you find.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 260.0,
            )
        }
        (_, Some(FactionCategory::Religious)) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Investigate temporal readings near {}", dest_sys_name),
                format!(
                    "\"The Order has detected temporal anomalies in the {} region. \
                     We need someone to visit and document what they experience. \
                     Pay attention to how time feels there.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 220.0,
            )
        }
        (_, _) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Check on situation at {}", dest_sys_name),
                format!(
                    "\"I need someone to swing by {} and see what's going on. \
                     Nothing dangerous — just take a look around and let me know.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 200.0,
            )
        }
    };

    contract.destination_location_id = dest_loc_id;
    Some(contract)
}

fn turn_in_contract(gs: &mut GameState, npc_idx: usize) {
    let npc_id = gs.galaxy.npcs[npc_idx].id;
    let npc_name = gs.galaxy.npcs[npc_idx].name.clone();
    let npc_warmth = gs.galaxy.npcs[npc_idx].personality.warmth;
    let npc_boldness = gs.galaxy.npcs[npc_idx].personality.boldness;

    // Find the completable contract.
    let contract_idx = gs.journey.active_contracts.iter()
        .position(|c| c.issuer_npc_id == npc_id && c.state == ContractState::ReadyToComplete);

    let contract_idx = match contract_idx {
        Some(i) => i,
        None => return,
    };

    let contract = &gs.journey.active_contracts[contract_idx];
    let reward = contract.reward_credits;
    let title = contract.title.clone();

    clear_screen();
    display_header("Contract Complete");
    println!();

    println!("  {}", title);
    println!();

    // Personality-shaped completion dialogue.
    if npc_warmth > 0.6 {
        println!("  {} smiles. \"Nicely done. I knew I picked the right person.\"", npc_name);
    } else if npc_boldness > 0.6 {
        println!("  {} nods sharply. \"Clean work. That's what I like.\"", npc_name);
    } else if npc_warmth < 0.3 {
        println!("  {} checks the manifest without comment. \"Everything's in order.\"", npc_name);
    } else {
        println!("  {} nods. \"Job's done. Clean work.\"", npc_name);
    }
    println!();
    println!("  +{:.0} credits", reward);

    // Pay the player.
    gs.journey.resources += reward;

    // Improve disposition (more than Phase A's flat 0.15 — scales with contract quality).
    let disposition_boost = if reward > 250.0 { 0.15 } else { 0.1 };
    let npc = &mut gs.galaxy.npcs[npc_idx];
    npc.record_interaction(
        format!("completed contract: {}", title),
        gs.journey.time.galactic_days,
        disposition_boost,
    );

    // Mark contract completed.
    gs.journey.active_contracts[contract_idx].state = ContractState::Completed;

    // Log it.
    gs.journey.event_log.push(starbound_core::narrative::GameEvent {
        timestamp: gs.journey.time,
        category: starbound_core::narrative::EventCategory::Faction,
        description: format!("Completed: {}. Earned {:.0} credits.", title, reward),
        associated_entities: vec![],
        consequences: vec![format!("+{:.0} credits", reward)],
    });

    pause();
}

fn display_contracts(gs: &GameState) {
    clear_screen();
    display_header("Active Contracts");

    let active: Vec<&Contract> = gs.journey.active_contracts.iter()
        .filter(|c| c.state == ContractState::Active || c.state == ContractState::ReadyToComplete)
        .collect();

    let completed: Vec<&Contract> = gs.journey.active_contracts.iter()
        .filter(|c| c.state == ContractState::Completed)
        .collect();

    if active.is_empty() && completed.is_empty() {
        println!("  No contracts. Talk to people to find work.");
        pause();
        return;
    }

    if !active.is_empty() {
        println!();
        for contract in &active {
            let dest_sys = gs.system_name(contract.destination_system_id);
            let origin_sys = gs.system_name(contract.origin_system_id);

            // Build destination string with location if known.
            let dest_str = if let Some(loc_id) = contract.destination_location_id {
                let loc_name = gs.galaxy.systems.iter()
                    .find(|s| s.id == contract.destination_system_id)
                    .and_then(|s| s.locations.iter().find(|l| l.id == loc_id))
                    .map(|l| l.name.as_str())
                    .unwrap_or("unknown location");
                format!("{} ({} system)", loc_name, dest_sys)
            } else {
                dest_sys.to_string()
            };

            let status = match contract.state {
                ContractState::Active => {
                    let action = match contract.contract_type {
                        ContractType::Delivery => format!("Deliver to {}", dest_str),
                        ContractType::Retrieval => format!("Retrieve from {}", dest_str),
                        ContractType::Investigation => format!("Investigate at {}", dest_str),
                    };
                    format!("{} — then return to {}", action, origin_sys)
                }
                ContractState::ReadyToComplete => {
                    format!("Return to {} to collect payment", origin_sys)
                }
                _ => "Unknown".into(),
            };
            println!("  · {}", contract.title);
            println!("    {}", status);
            println!("    Reward: {:.0} credits", contract.reward_credits);
            println!();
        }
    }

    if !completed.is_empty() {
        println!("  Completed:");
        for contract in completed.iter().rev().take(5) {
            println!("  ✓ {}", contract.title);
        }
        if completed.len() > 5 {
            println!("    ... and {} more.", completed.len() - 5);
        }
    }

    pause();
}

// ---------------------------------------------------------------------------
// Contract state tracking — check on arrival
// ---------------------------------------------------------------------------

/// Check active contracts against the player's current state.
/// Call this on arrival at a new system.
fn check_contract_progress(gs: &mut GameState) {
    let current_system = gs.journey.current_system;
    let current_location = gs.journey.current_location;
    let mut messages: Vec<String> = Vec::new();

    for contract in &mut gs.journey.active_contracts {
        if contract.state != ContractState::Active {
            continue;
        }

        match contract.contract_type {
            ContractType::Delivery => {
                // Must be at the right system.
                if contract.destination_system_id != current_system {
                    continue;
                }
                // If contract specifies a location, must be at that location.
                if let Some(dest_loc) = contract.destination_location_id {
                    if current_location != Some(dest_loc) {
                        continue;
                    }
                }
                // Check cargo.
                if let Some((ref cargo_name, qty)) = contract.cargo_required {
                    let held = gs.journey.ship.cargo.get(cargo_name).copied().unwrap_or(0);
                    if held >= qty {
                        let remaining = held - qty;
                        if remaining == 0 {
                            gs.journey.ship.cargo.remove(cargo_name);
                        } else {
                            gs.journey.ship.cargo.insert(cargo_name.clone(), remaining);
                        }
                        contract.state = ContractState::ReadyToComplete;
                        messages.push(format!(
                            "Contract objective complete: {}. Delivered {} x{}. \
                             Return to the contract issuer to collect payment.",
                            contract.title, cargo_name, qty,
                        ));
                    }
                }
            }
            ContractType::Retrieval => {
                // Must be at the destination system.
                if contract.destination_system_id != current_system {
                    continue;
                }
                if let Some(dest_loc) = contract.destination_location_id {
                    if current_location != Some(dest_loc) {
                        continue;
                    }
                }
                // Auto-acquire the required cargo at the destination.
                if let Some((ref cargo_name, qty)) = contract.cargo_required {
                    let total_cargo: u32 = gs.journey.ship.cargo.values().sum();
                    if total_cargo + qty > gs.journey.ship.cargo_capacity {
                        messages.push(format!(
                            "You've located the {} for contract: {}, \
                             but your cargo hold is too full to take it. \
                             Free up {} units of cargo space.",
                            cargo_name, contract.title, qty,
                        ));
                        continue;
                    }
                    let current = gs.journey.ship.cargo.get(cargo_name).copied().unwrap_or(0);
                    gs.journey.ship.cargo.insert(cargo_name.clone(), current + qty);
                    contract.state = ContractState::ReadyToComplete;
                    messages.push(format!(
                        "Contract objective complete: {}. Retrieved {} x{}. \
                         Return to the contract issuer to collect payment.",
                        contract.title, cargo_name, qty,
                    ));
                } else {
                    // Retrieval with no specific cargo — just arriving is enough.
                    contract.state = ContractState::ReadyToComplete;
                    messages.push(format!(
                        "Contract objective complete: {}. \
                         Return to the contract issuer to collect payment.",
                        contract.title,
                    ));
                }
            }
            ContractType::Investigation => {
                // Must be at the destination system.
                if contract.destination_system_id != current_system {
                    continue;
                }
                if let Some(dest_loc) = contract.destination_location_id {
                    if current_location != Some(dest_loc) {
                        continue;
                    }
                }
                // Arriving at the destination completes the investigation.
                contract.state = ContractState::ReadyToComplete;
                messages.push(format!(
                    "Investigation complete: {}. You've seen enough. \
                     Return to the contract issuer to report your findings.",
                    contract.title,
                ));
            }
        }
    }

    // Display any progress messages.
    if !messages.is_empty() {
        println!();
        println!("{}", THIN_DIVIDER);
        for msg in &messages {
            for line in wrap_text(msg, 60) {
                println!("  {}", line);
            }
        }
    }
}

fn travel_menu(gs: &mut GameState) {
    let connections = gs.connections_from_current();

    if connections.is_empty() {
        println!("\n  No routes out of this system.");
        pause();
        return;
    }

    let plans = plan_all_routes(&connections, &gs.journey.ship, gs.journey.current_system);

    if plans.is_empty() {
        println!("\n  No viable routes from here.");
        pause();
        return;
    }

    display_routes(&plans, gs);
    println!("  0) Back");
    println!();

    let input = prompt("  Choose route > ");
    let idx: usize = match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= plans.len() => n - 1,
        _ => return,
    };

    let plan = &plans[idx];

    if !plan.feasible {
        println!("\n  {}", plan.infeasible_reason.as_deref().unwrap_or("Not feasible."));
        pause();
        return;
    }

    let dest_name = gs.system_name(plan.destination_id).to_string();
    let desc = describe_plan(plan, &dest_name);
    println!("\n  {}", desc);
    let confirm = prompt("  Proceed? (y/n) > ");

    if confirm != "y" && confirm != "yes" {
        return;
    }

    execute_travel_and_arrive(gs, plan, &dest_name);
}

fn execute_travel_and_arrive(gs: &mut GameState, plan: &TravelPlan, dest_name: &str) {
    let outcome = match execute_travel(&mut gs.journey, plan, dest_name) {
        Ok(o) => o,
        Err(e) => {
            println!("\n  Travel failed: {}", e);
            pause();
            return;
        }
    };

    // Transit narrative.
    clear_screen();
    display_header("In Transit");

    let mode_str = match plan.mode {
        TravelMode::Sublight => "sublight",
        TravelMode::Ftl => "FTL",
        TravelMode::Stationary => "stationary",
    };

    println!();
    for line in wrap_text(
        &format!(
            "The {} transit to {} takes {:.1} months of ship time. \
             Outside, {:.1} years pass in the galaxy.",
            mode_str, dest_name,
            outcome.personal_days / 30.44,
            outcome.galactic_days / 365.25,
        ), 60,
    ) { println!("  {}", line); }

    if outcome.fuel_spent > 0.0 {
        println!("\n  Fuel consumed: {:.1}", outcome.fuel_spent);
    }

    if outcome.supplies_consumed > 0.0 {
        println!("  Supplies consumed: {:.1}", outcome.supplies_consumed);
    }

    // Supply warnings — these matter narratively.
    for warning in &outcome.supply_warnings {
        println!();
        for line in wrap_text(&format!("  ⚠ {}", warning), 60) {
            println!("{}", line);
        }
    }

    println!("\n  Total elapsed: {:.1} months personal / {:.1} years galactic",
        outcome.total_personal_years * 12.0,
        outcome.total_galactic_years);

    pause();

    // --- Phase 2: Run galactic tick engine ---
    let galactic_days_elapsed = gs.journey.time.galactic_days - gs.last_ticked_day;
    if galactic_days_elapsed >= 365.25 {
        let tick_result = tick_galaxy(
            &mut gs.galaxy,
            galactic_days_elapsed,
            gs.last_ticked_day,
            &mut gs.rng,
        );
        gs.last_ticked_day = gs.journey.time.galactic_days;

        if !tick_result.events.is_empty() {
            display_galactic_news(&tick_result, gs);
            pause();
            // Store for the rumor system's faction scanner (cap at 20).
            gs.recent_tick_events.extend(tick_result.events.iter().cloned());
            if gs.recent_tick_events.len() > 20 {
                let drain_count = gs.recent_tick_events.len() - 20;
                gs.recent_tick_events.drain(..drain_count);
            }
        }
    } else {
        gs.last_ticked_day = gs.journey.time.galactic_days;
    }

    // Record visit.
    gs.record_visit();

    // Reset crew conversation topics — new system, new things to talk about.
    gs.discussed_topics.clear();

    // Reset scene history — new system, fresh context for LLM.
    gs.scene_history.clear();

    // Player arrives at system edge — not yet docked.
    gs.journey.current_location = None;

    // Check if any active contracts advance at this system.
    check_contract_progress(gs);

    // --- Check for pending follow-up events (from NextArrival chains) ---
    let pending: Vec<String> = gs.pending_followups.drain(..).collect();
    for event_id in pending {
        if let Some(event) = gs.events.iter().find(|e| e.id == event_id).cloned() {
            run_encounter_chain(gs, &event);
        }
    }

    // Show arrival summary.
    clear_screen();
    display_header(&format!("Arriving at {}", dest_name));
    display_system_info(gs);
    pause();

    // Open system map so player can pick where to go.
    system_map(gs);
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

/// Load environment variables from a .env file if it exists.
/// Supports lines like `KEY=value` and `KEY="value"`. Skips comments and blanks.
fn load_dotenv() {
    let paths = [".env", "../.env"];
    for path in &paths {
        if let Ok(contents) = std::fs::read_to_string(path) {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim().trim_matches('"').trim_matches('\'');
                    if !key.is_empty() && std::env::var(key).is_err() {
                        std::env::set_var(key, value);
                    }
                }
            }
            return; // Use the first .env found.
        }
    }
}

fn main() {
    load_dotenv();
    display_title();

    let choice = prompt("  > ");

    match choice.as_str() {
        "1" => {
            let seed_input = prompt("  Galaxy seed (enter for random): ");
            let seed: u64 = if seed_input.is_empty() {
                rand::random()
            } else {
                seed_input.parse().unwrap_or_else(|_| {
                    // Hash string into seed.
                    let mut hash: u64 = 5381;
                    for byte in seed_input.bytes() {
                        hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
                    }
                    hash
                })
            };

            println!("  Seed: {}", seed);
            let mut gs = new_game(seed);

            // --- LLM configuration ---
            let llm_input = prompt("  Enable LLM generation? (y/n, default n): ");
            if llm_input.trim().eq_ignore_ascii_case("y") || llm_input.trim().eq_ignore_ascii_case("yes") {
                gs.llm_config.enabled = true;

                // Check for API key — env var first, then prompt.
                if gs.llm_config.resolve_api_key().is_none() {
                    let key_input = prompt("  OpenRouter API key: ");
                    let key = key_input.trim().to_string();
                    if !key.is_empty() {
                        gs.llm_config.api_key = key;
                    }
                }

                if gs.llm_config.resolve_api_key().is_some() {
                    println!("  LLM enabled (model: {})", gs.llm_config.model);
                } else {
                    println!("  No API key provided. Falling back to seed library.");
                    gs.llm_config.enabled = false;
                }

                // Allow model override.
                if gs.llm_config.enabled {
                    let model_input = prompt("  Model (enter for default): ");
                    if !model_input.trim().is_empty() {
                        gs.llm_config.model = model_input.trim().to_string();
                        println!("  Using model: {}", gs.llm_config.model);
                    }
                }
            }

            game_loop(&mut gs);
        }
        _ => {
            println!("\n  The galaxy waits.\n");
        }
    }
}