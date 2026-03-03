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

use starbound_core::crew::*;
use starbound_core::galaxy::*;
use starbound_core::journey::Journey;
use starbound_core::mission::*;
use starbound_core::ship::*;
use starbound_core::time::Timestamp;

use starbound_encounters::library::all_seed_events;
use starbound_encounters::pipeline::{
    run_pipeline, PipelineConfig, PipelineResult, PipelineState,
};
use starbound_encounters::seed_event::SeedEvent;

use starbound_simulation::generate::{generate_galaxy, GeneratedGalaxy};
use starbound_simulation::travel::{describe_plan, plan_all_routes, TravelPlan};

use starbound_game::travel::execute_travel;

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

    fn faction_name(&self, id: Uuid) -> &str {
        self.galaxy.factions.iter()
            .find(|f| f.id == id)
            .map(|f| f.name.as_str())
            .unwrap_or("Independent")
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
    }
}

// ---------------------------------------------------------------------------
// Display functions
// ---------------------------------------------------------------------------

fn display_system_info(gs: &GameState) {
    let sys = gs.current_system();
    let faction_str = match sys.controlling_faction {
        Some(id) => gs.faction_name(id).to_string(),
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
}

fn display_ship_status(gs: &GameState) {
    let ship = &gs.journey.ship;
    let time = &gs.journey.time;

    println!("\n  Ship: {}  |  Hull: {:.0}%  |  Fuel: {:.0}/{:.0}  |  Credits: {:.0}",
        ship.name,
        ship.hull_condition * 100.0,
        ship.fuel, ship.fuel_capacity,
        gs.journey.resources);

    println!("  Time: {:.1} months personal / {:.1} years galactic",
        time.personal_days / 30.44,
        time.galactic_years());
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

fn display_encounter(event: &SeedEvent) {
    println!("\n{}", DIVIDER);
    println!();

    for paragraph in event.text.split("\n\n") {
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
        println!("  1) Travel        2) Crew");
        println!("  3) Mission       4) Log");
        println!("  5) Quit");
        println!();

        let choice = prompt("  > ");

        match choice.as_str() {
            "1" | "travel" => travel_menu(gs),
            "2" | "crew" => {
                display_crew_detail(gs);
                pause();
            }
            "3" | "mission" => {
                display_mission(gs);
                pause();
            }
            "4" | "log" => {
                display_event_log(gs);
                pause();
            }
            "5" | "q" | "quit" => {
                println!("\n  The galaxy continues without you.\n");
                break;
            }
            _ => {}
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

    println!("\n  Total elapsed: {:.1} months personal / {:.1} years galactic",
        outcome.total_personal_years * 12.0,
        outcome.total_galactic_years);

    pause();

    // Record visit.
    gs.record_visit();

    // Run encounter pipeline.
    let system = gs.current_system().clone();
    let years_since = gs.galactic_years_since_last_visit();

    let result = run_pipeline(
        &gs.events, &system, &gs.journey, years_since,
        &gs.pipeline_state, &gs.pipeline_config, &mut gs.rng,
    );

    match result {
        PipelineResult::Event { event, .. } => {
            let tone = match event.tone.as_str() {
                "tense" => starbound_core::narrative::Tone::Tense,
                "quiet" => starbound_core::narrative::Tone::Quiet,
                "wonder" => starbound_core::narrative::Tone::Wonder,
                "urgent" => starbound_core::narrative::Tone::Urgent,
                "melancholy" => starbound_core::narrative::Tone::Melancholy,
                _ => starbound_core::narrative::Tone::Mundane,
            };
            gs.pipeline_state.record_event(&event.id, tone);

            clear_screen();
            display_encounter(event);
            println!();

            let input = prompt("  > ");
            let choice_idx: usize = input.parse::<usize>()
                .unwrap_or(1)
                .saturating_sub(1)
                .min(event.choices.len().saturating_sub(1));

            let chosen = &event.choices[choice_idx];
            println!("\n  > {}", chosen.label);

            pause();
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