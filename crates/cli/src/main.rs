// file: crates/cli/src/main.rs
//! Starbound — terminal prototype.
//!
//! The first playable version. A map, a ship, a crew, and the galaxy.
//! Travel between systems, encounter events, make choices, watch
//! time slip away.

use std::collections::HashMap;
use std::io::{self, Write};

use rand::rngs::StdRng;
use rand::SeedableRng;
use uuid::Uuid;

use starbound_core::contract::{Contract, ContractState, ContractType};
use starbound_core::crew::*;
use starbound_core::galaxy::*;
use starbound_core::journey::Journey;
use starbound_core::mission::*;
use starbound_core::npc::Npc;
use starbound_core::ship::*;
use starbound_core::reputation::PlayerProfile;
use starbound_core::time::Timestamp;

use starbound_encounters::library::all_seed_events;
use starbound_encounters::pipeline::{
    run_pipeline, PipelineConfig, PipelineResult, PipelineState, PlayerIntent,
};
use starbound_encounters::seed_event::{SeedEvent, EffectDef, FollowUpDelay};
use starbound_encounters::templates::{resolve_template, TemplateContext};

use starbound_simulation::generate::{generate_galaxy, GeneratedGalaxy};
use starbound_simulation::travel::{describe_plan, plan_all_routes, TravelPlan};
use starbound_simulation::tick::{tick_galaxy, TickResult, TickEventCategory};

use starbound_game::travel::execute_travel;
use starbound_game::consequences::{convert_effects, apply_effects};

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

    /// Get all living NPCs at the player's current system.
    fn npcs_here(&self) -> Vec<&Npc> {
        self.galaxy.npcs.iter()
            .filter(|n| n.home_system_id == self.journey.current_system && n.alive)
            .collect()
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
        .find(|s| s.name == "Cygnus Gate")
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
    };

    let mut visit_log = HashMap::new();
    visit_log.insert(start_system.id, 0.0);

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

    if !sys.planetary_bodies.is_empty() {
        let body_names: Vec<&str> = sys.planetary_bodies.iter()
            .map(|b| b.name.as_str())
            .collect();
        println!("  Bodies: {}", body_names.join(", "));
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

    // NPCs present at this system.
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

    // Economy summary — fuel and supply prices.
    if let Some(ref econ) = sys.economy {
        println!();
        println!("  Fuel: {:.1} cr/unit  |  Supplies: {:.1} cr/unit",
            econ.fuel_price, econ.supply_price);
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

fn display_intro() {
    clear_screen();
    println!();
    println!("{}", DIVIDER);
    println!();

    for line in wrap_text(
        "You stand on the bridge of the Persistence, docked at \
         Cygnus Gate — a transit station in contested space, \
         halfway between Hegemony territory and the Freehold \
         Compact. Neither faction claims it officially. Both \
         keep an eye on it.", 60
    ) { println!("  {}", line); }
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
    display_intro();

    loop {
        clear_screen();
        display_system_info(gs);
        display_ship_status(gs);

        println!("\n  What do you do?");
        println!("  1) Travel        2) Actions");
        println!("  3) People        4) Contracts");
        println!("  5) Crew          6) Mission");
        println!("  7) Log           8) Threads");
        println!("  9) Quit");
        println!();

        let choice = prompt("  > ");

        match choice.as_str() {
            "1" | "travel" => travel_menu(gs),
            "2" | "actions" | "act" => action_menu(gs),
            "3" | "people" | "talk" => people_menu(gs),
            "4" | "contracts" => display_contracts(gs),
            "5" | "crew" => {
                display_crew_detail(gs);
                pause();
            }
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
// Action menu — player-initiated encounters
// ---------------------------------------------------------------------------

/// Determine which actions are available at the current system.
fn available_actions(gs: &GameState) -> Vec<PlayerIntent> {
    let system = gs.current_system();
    let mut actions = Vec::new();

    // Scan is always available — you can always point your sensors at something.
    actions.push(PlayerIntent::Scan);

    // Investigate is always available — curiosity doesn't need infrastructure.
    actions.push(PlayerIntent::Investigate);

    // Trade requires at least an outpost.
    let infra_rank = match system.infrastructure_level {
        starbound_core::galaxy::InfrastructureLevel::None => 0,
        starbound_core::galaxy::InfrastructureLevel::Outpost => 1,
        starbound_core::galaxy::InfrastructureLevel::Colony => 2,
        starbound_core::galaxy::InfrastructureLevel::Established => 3,
        starbound_core::galaxy::InfrastructureLevel::Hub => 4,
        starbound_core::galaxy::InfrastructureLevel::Capital => 5,
    };

    if infra_rank >= 1 {
        actions.push(PlayerIntent::Trade);
    }

    // Repair requires at least an outpost.
    if infra_rank >= 1 {
        actions.push(PlayerIntent::Repair);
    }

    // Resupply requires at least an outpost.
    if infra_rank >= 1 {
        actions.push(PlayerIntent::Resupply);
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
        _ => run_intent_encounter(gs, intent),
    }
}

// ---------------------------------------------------------------------------
// Trade screen — buy and sell goods
// ---------------------------------------------------------------------------

fn trade_screen(gs: &mut GameState) {
    let economy = match gs.current_system().economy.clone() {
        Some(e) => e,
        None => {
            println!("\n  No trade facilities here.");
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
    let economy = match gs.current_system().economy.clone() {
        Some(e) => e,
        None => {
            println!("\n  No resupply facilities here.");
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
    let infra = gs.current_system().infrastructure_level;
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

fn run_intent_encounter(gs: &mut GameState, intent: PlayerIntent) {
    let system = gs.current_system().clone();
    let years_since = gs.galactic_years_since_last_visit();

    let result = run_pipeline(
        &gs.events, &system, &gs.journey, years_since,
        &gs.pipeline_state, &gs.pipeline_config, &mut gs.rng,
        Some(intent),
    );

    match result {
        PipelineResult::Event { event, .. } => {
            let event = event.clone();
            run_encounter_chain(gs, &event);
        }
        PipelineResult::Silence { reason, .. } => {
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
            };

            for line in wrap_text(msg, 60) {
                println!("  {}", line);
            }

            // Log it for debugging.
            let _ = reason;

            pause();
        }
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
        let faction_str = npc.faction_id
            .map(|fid| gs.faction_name(fid).to_string())
            .unwrap_or_else(|| "Independent".into());
        println!("  {}) {} — {} ({})", i + 1, npc.name, npc.title, faction_str);
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

        clear_screen();
        display_header(&format!("{} — {}", npc.name, npc.title));
        println!();

        for line in wrap_text(&npc.bio, 60) {
            println!("  {}", line);
        }
        println!();

        // Check if the player has a completable contract from this NPC.
        let has_turnable = gs.journey.active_contracts.iter()
            .any(|c| c.issuer_npc_id == npc_id && c.state == ContractState::ReadyToComplete);

        let mut options: Vec<&str> = Vec::new();
        options.push("Ask about work");
        if has_turnable {
            options.push("Turn in contract");
        }
        options.push("Ask about the area");
        options.push("Leave");

        for (i, opt) in options.iter().enumerate() {
            println!("  {}) {}", i + 1, opt);
        }
        println!();

        let input = prompt("  > ");
        let choice: usize = input.parse::<usize>().unwrap_or(0);

        if choice == 0 || choice > options.len() {
            return;
        }

        let chosen = options[choice - 1];
        match chosen {
            "Ask about work" => offer_contracts(gs, npc_idx),
            "Turn in contract" => turn_in_contract(gs, npc_idx),
            "Ask about the area" => {
                let system = gs.current_system();
                clear_screen();
                display_header("Local Intel");
                println!();

                let msg = format!(
                    "\"{}? It's a {} system. {}. Not much changes here, \
                     unless you count the ships passing through.\"",
                    system.name,
                    system.infrastructure_level,
                    if system.faction_presence.len() > 2 {
                        "Lot of factions have a stake here"
                    } else {
                        "Quiet, mostly"
                    }
                );
                for line in wrap_text(&msg, 60) {
                    println!("  {}", line);
                }

                // Mention active threads in the area if the NPC knows something.
                if !system.active_threads.is_empty() {
                    println!();
                    let npc = &gs.galaxy.npcs[npc_idx];
                    println!("  {} pauses, as if deciding whether to say more.", npc.name);
                    println!("  \"There have been some... unusual reports lately.\"");
                }

                pause();
            }
            "Leave" => return,
            _ => return,
        }
    }
}

fn offer_contracts(gs: &mut GameState, npc_idx: usize) {
    let npc = &gs.galaxy.npcs[npc_idx];
    let npc_id = npc.id;

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

    let dest_name = gs.system_name(contract.destination_system_id).to_string();
    println!("  Destination: {}", dest_name);
    println!("  Reward: {:.0} credits", contract.reward_credits);
    if let Some((ref cargo, qty)) = contract.cargo_given {
        println!("  Cargo provided: {} x{}", cargo, qty);
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
            gs.journey.active_contracts.push(accepted);

            clear_screen();
            println!();
            println!("  \"Good. Don't let me down.\"");
            println!();
            println!("  Contract accepted. Check your contracts log.");
            pause();
        }
        _ => {
            println!("  \"Your call. Offer stands if you change your mind.\"");
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
    let dest_id = if conn.system_a == home_id { conn.system_b } else { conn.system_a };
    let dest_name = gs.system_name(dest_id);

    // Generate based on faction category.
    let category = npc.faction_id
        .and_then(|fid| gs.galaxy.factions.iter().find(|f| f.id == fid))
        .map(|f| f.category);

    let (title, desc, cargo_name, cargo_qty, reward) = match category {
        Some(FactionCategory::Guild) => (
            format!("Deliver repair components to {}", dest_name),
            format!(
                "\"We've got a maintenance backlog at {}. \
                 Standard repair components — nothing exotic, but they \
                 need them yesterday. Deliver, get the dock master to sign off, \
                 and come back for your pay.\"",
                dest_name
            ),
            "Repair components",
            8,
            200.0,
        ),
        Some(FactionCategory::Military) => (
            format!("Transport sealed cargo to {}", dest_name),
            format!(
                "\"Military business. Sealed containers, don't ask what's inside. \
                 Take them to {} garrison, hand them over, bring back the receipt. \
                 Standard courier rate.\"",
                dest_name
            ),
            "Sealed military cargo",
            5,
            250.0,
        ),
        Some(FactionCategory::Economic) => (
            format!("Supply run to {}", dest_name),
            format!(
                "\"The market at {} is running short on manufactured goods. \
                 We've got a shipment ready to go. Deliver it, collect payment \
                 on delivery, and bring back our cut.\"",
                dest_name
            ),
            "Manufactured goods",
            12,
            180.0,
        ),
        Some(FactionCategory::Criminal) => (
            format!("Discreet delivery to {}", dest_name),
            format!(
                "\"I've got a package. It needs to get to {} without anyone \
                 asking questions. No manifests, no declarations. \
                 You handle it clean, I make it worth your while.\"",
                dest_name
            ),
            "Unmarked cargo",
            3,
            300.0,
        ),
        Some(FactionCategory::Religious) => (
            format!("Deliver relics to {}", dest_name),
            format!(
                "\"These artifacts need to reach the monastery at {}. \
                 They're delicate — not physically, but... spiritually. \
                 Handle them with respect. The Order will remember your service.\"",
                dest_name
            ),
            "Religious artifacts",
            4,
            150.0,
        ),
        _ => (
            format!("Courier run to {}", dest_name),
            format!(
                "\"Standard job. Take this cargo to {}, hand it off, \
                 come back with confirmation. Simple work, fair pay.\"",
                dest_name
            ),
            "General cargo",
            6,
            175.0,
        ),
    };

    Some(Contract::delivery(
        npc.id,
        npc.faction_id,
        title,
        desc,
        home_id,
        dest_id,
        cargo_name,
        cargo_qty,
        reward,
    ))
}

fn turn_in_contract(gs: &mut GameState, npc_idx: usize) {
    let npc_id = gs.galaxy.npcs[npc_idx].id;
    let npc_name = gs.galaxy.npcs[npc_idx].name.clone();

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
    println!("  {} nods. \"Job's done. Clean work.\"", npc_name);
    println!();
    println!("  +{:.0} credits", reward);

    // Pay the player.
    gs.journey.resources += reward;

    // Improve disposition.
    let npc = &mut gs.galaxy.npcs[npc_idx];
    npc.disposition = (npc.disposition + 0.15).min(1.0);
    npc.notes.push(format!("Completed contract: {}", title));

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
            let dest = gs.system_name(contract.destination_system_id);
            let origin = gs.system_name(contract.origin_system_id);
            let status = match contract.state {
                ContractState::Active => {
                    format!("Deliver to {} — then return to {}", dest, origin)
                }
                ContractState::ReadyToComplete => {
                    format!("Return to {} to collect payment", origin)
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
    let mut messages: Vec<String> = Vec::new();

    for contract in &mut gs.journey.active_contracts {
        if contract.state != ContractState::Active {
            continue;
        }

        match contract.contract_type {
            ContractType::Delivery => {
                // Delivery: arrived at destination with the cargo?
                if contract.destination_system_id == current_system {
                    if let Some((ref cargo_name, qty)) = contract.cargo_required {
                        let held = gs.journey.ship.cargo.get(cargo_name).copied().unwrap_or(0);
                        if held >= qty {
                            // Remove the cargo.
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
            }
            _ => {} // Other types for later.
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
        }
    } else {
        gs.last_ticked_day = gs.journey.time.galactic_days;
    }

    // Record visit.
    gs.record_visit();

    // Check if any active contracts advance at this system.
    check_contract_progress(gs);

    // --- Check for pending follow-up events (from NextArrival chains) ---
    let pending: Vec<String> = gs.pending_followups.drain(..).collect();
    for event_id in pending {
        if let Some(event) = gs.events.iter().find(|e| e.id == event_id).cloned() {
            run_encounter_chain(gs, &event);
        }
    }

    // Run encounter pipeline.
    let system = gs.current_system().clone();
    let years_since = gs.galactic_years_since_last_visit();

    let result = run_pipeline(
        &gs.events, &system, &gs.journey, years_since,
        &gs.pipeline_state, &gs.pipeline_config, &mut gs.rng,
        None,
    );

    match result {
        PipelineResult::Event { event, .. } => {
            let event = event.clone();
            run_encounter_chain(gs, &event);
        }
        PipelineResult::Silence { .. } => {
            gs.pipeline_state.record_silence();

            clear_screen();
            display_header(dest_name);
            println!();
            for line in wrap_text(
                "Nothing demands your attention. The system is quiet. \
                 Your crew goes about their routines. The ship hums. \
                 Outside, stars burn with indifference.", 60,
            ) { println!("  {}", line); }

            pause();
        }
    }   
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
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
            game_loop(&mut gs);
        }
        _ => {
            println!("\n  The galaxy waits.\n");
        }
    }
}