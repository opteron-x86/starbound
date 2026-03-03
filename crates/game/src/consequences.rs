// file: crates/game/src/consequences.rs
//! The consequence system — turns player choices into game state changes.
//!
//! Day 8: Making choices matter.
//!
//! Each seed event choice has a `mechanical_effect` string. This module
//! interprets those strings into concrete `Effect` values and applies
//! them to the journey state. The effect vocabulary is intentionally
//! small and composable — the LLM will eventually use the same vocabulary,
//! and a small, well-defined set is easier to validate.
//!
//! Design principle: effects are deterministic given the same game state.
//! Randomness belongs in the encounter pipeline, not in consequences.

use uuid::Uuid;

use starbound_core::crew::Mood;
use starbound_core::journey::Journey;
use starbound_core::narrative::{
    EventCategory, GameEvent, ResolutionState, Thread, ThreadType,
};

// ---------------------------------------------------------------------------
// Effect types
// ---------------------------------------------------------------------------

/// A single atomic change to the game state.
/// Effects are composed — one choice can produce several.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Add or remove fuel. Clamped to [0, capacity].
    Fuel(f32),
    /// Add or remove generic resources (credits/trade goods).
    Resources(f64),
    /// Add or remove hull condition. Clamped to [0.0, 1.0].
    Hull(f32),
    /// Adjust stress for all crew. Clamped to [0.0, 1.0].
    CrewStress(f32),
    /// Set mood for a random crew member (or all if `all` is true).
    CrewMood { mood: Mood, all: bool },
    /// Adjust professional trust for all crew toward the captain.
    TrustProfessional(f32),
    /// Adjust personal trust for all crew toward the captain.
    TrustPersonal(f32),
    /// Adjust ideological trust for all crew toward the captain.
    TrustIdeological(f32),
    /// Spawn a new narrative thread.
    SpawnThread {
        thread_type: ThreadType,
        description: String,
    },
    /// Add a cargo item.
    AddCargo { item: String, quantity: u32 },
    /// Remove all cargo (jettison).
    JettisonCargo,
    /// Damage a specific ship module. Amount subtracted from condition.
    DamageModule { module: ModuleTarget, amount: f32 },
    /// Repair a specific ship module. Amount added to condition.
    RepairModule { module: ModuleTarget, amount: f32 },
    /// Add a concern to a random crew member's active concerns.
    AddConcern(String),
    /// Log a narrative note (no mechanical change, but appears in the log).
    Narrative(String),
    /// No mechanical effect — the choice was about tone, not state.
    Pass,
}

/// Which ship module an effect targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleTarget {
    Engine,
    Sensors,
    Comms,
    Weapons,
    LifeSupport,
}

// ---------------------------------------------------------------------------
// The consequence outcome — what happened, in words
// ---------------------------------------------------------------------------

/// Summary of effects applied, suitable for the event log and CLI display.
#[derive(Debug, Clone)]
pub struct ConsequenceReport {
    /// Human-readable lines describing what changed.
    pub changes: Vec<String>,
    /// The narrative log entry for this encounter outcome.
    pub log_entry: String,
    /// Whether any threads were spawned.
    pub threads_spawned: usize,
}

// ---------------------------------------------------------------------------
// Effect resolution: mechanical_effect string → Vec<Effect>
// ---------------------------------------------------------------------------

/// Resolve a mechanical effect string into concrete effects.
/// This is the central vocabulary — every effect the game can produce
/// is defined here. Unknown effects produce a Pass with a log note.
pub fn resolve_effects(mechanical_effect: &str, journey: &Journey) -> Vec<Effect> {
    match mechanical_effect {
        // -----------------------------------------------------------------
        // Fuel and trade
        // -----------------------------------------------------------------
        "buy_fuel" => vec![
            Effect::Fuel(20.0),
            Effect::Resources(-30.0),
        ],
        "buy_fuel_and_talk" => vec![
            Effect::Fuel(20.0),
            Effect::Resources(-30.0),
            Effect::CrewStress(-0.05),
            Effect::Narrative("A small kindness at a quiet refueling stop.".into()),
        ],
        "buy_fuel_expensive" => {
            // Price scales with desperation — lower fuel = higher price.
            let fuel_frac = journey.ship.fuel / journey.ship.fuel_capacity;
            let price_multiplier = 2.0 + (1.0 - fuel_frac as f64);
            let fill_amount = (journey.ship.fuel_capacity - journey.ship.fuel).min(40.0);
            vec![
                Effect::Fuel(fill_amount),
                Effect::Resources(-(fill_amount as f64 * price_multiplier)),
                Effect::CrewStress(0.05),
                Effect::Narrative("Paid through the nose, but the tanks are fuller.".into()),
            ]
        }
        "buy_fuel_minimum" => vec![
            Effect::Fuel(10.0),
            Effect::Resources(-25.0),
            Effect::Narrative("Just enough to reach the next port. Hopefully.".into()),
        ],
        "negotiate_fuel" => {
            // Negotiation gets a moderate deal — not as bad as full price.
            let fill_amount = (journey.ship.fuel_capacity - journey.ship.fuel).min(30.0);
            vec![
                Effect::Fuel(fill_amount),
                Effect::Resources(-(fill_amount as f64 * 1.5)),
                Effect::Narrative("Talked the price down. Not cheap, but fair enough.".into()),
            ]
        }
        "seek_alternatives" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Mystery,
                description: "Heard about an alternative fuel source — \
                    unorthodox, possibly dangerous, worth investigating.".into(),
            },
            Effect::Narrative("No fuel today, but a lead on something interesting.".into()),
        ],
        "open_trade" => vec![
            Effect::Fuel(15.0),
            Effect::Resources(-40.0),
            Effect::Hull(0.05),
            Effect::Narrative("Resupplied. The ship feels a little more whole.".into()),
        ],
        
        // -----------------------------------------------------------------
        // Corridor Guild encounters
        // -----------------------------------------------------------------
        "guild_preferred_rates" => vec![
            Effect::Fuel(30.0),
            Effect::Resources(-50.0),
            Effect::SpawnThread {
                thread_type: ThreadType::Debt,
                description: "Accepted Corridor Guild preferred rates. They'll \
                    remember — and they'll expect reciprocity.".into(),
            },
            Effect::Narrative("Signed on with the Guild's network. Good prices. Strings attached.".into()),
        ],
        "buy_fuel_independent" => vec![
            Effect::Fuel(20.0),
            Effect::Resources(-80.0),
            Effect::Narrative("Paid more than you had to. The independent seller looked grateful.".into()),
        ],
        "guild_probe_network" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "The Corridor Guild's logistics network extends further \
                    than official records suggest. They know shipping routes, cargo \
                    manifests, and who's moving what — including near the frontier.".into(),
            },
            Effect::Narrative("Learned more than the Guild rep intended to share.".into()),
        ],

        // -----------------------------------------------------------------
        // Lattice encounters
        // -----------------------------------------------------------------
        "lattice_copy_intel" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "Copied data from a Lattice dead drop. Coordinates, \
                    a name, and a warning: someone else knows about the signal.".into(),
            },
            Effect::Narrative("Took the knowledge. Left no trace.".into()),
        ],
        "lattice_take_chip" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "Took a Lattice dead drop chip. The coordinates and \
                    name might be useful. Taking the chip means they know you found it.".into(),
            },
            Effect::AddCargo { item: "Lattice data chip".into(), quantity: 1 },
            Effect::Narrative("Pocketed the chip. Someone will notice it's gone.".into()),
        ],
        "lattice_destroy" => vec![
            Effect::Narrative("Crushed the chip under your boot. Some connections aren't worth making.".into()),
        ],
        "lattice_accept_broker" => vec![
            Effect::Resources(-100.0),
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "Established a channel with a Lattice intelligence broker. \
                    They have information about the signal and faction movements near \
                    distorted space. The relationship is transactional — for now.".into(),
            },
            Effect::Narrative("Made contact with the shadows. They had exactly what you needed.".into()),
        ],
        "lattice_negotiate" => vec![
            Effect::Resources(-60.0),
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "Negotiated limited terms with a Lattice broker. They \
                    provided partial intelligence — enough to be useful, not enough \
                    to feel comfortable.".into(),
            },
            Effect::Narrative("Bargained them down. They respected the counter-offer.".into()),
        ],

        // -----------------------------------------------------------------
        // Order of the Quiet Star encounters
        // -----------------------------------------------------------------
        "quiet_star_listen" => vec![
            Effect::CrewMood { mood: Mood::Inspired, all: true },
            Effect::CrewStress(-0.05),
            Effect::SpawnThread {
                thread_type: ThreadType::Anomaly,
                description: "The Order describes patterns in the time distortion — \
                    rhythmic fluctuations that correlate with the signal's frequency. \
                    They believe the distortion and the signal share a source.".into(),
            },
            Effect::Narrative("Listened. What they described changes the shape of everything.".into()),
        ],
        "quiet_star_ask_signal" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Mystery,
                description: "The Order has been tracking the signal independently. \
                    Their data suggests the source is deep in distorted space — \
                    somewhere time runs so slowly it might as well have stopped.".into(),
            },
            Effect::Narrative("They know about the signal. They've been listening longer than you have.".into()),
        ],
        "quiet_star_join" => vec![
            Effect::CrewStress(-0.1),
            Effect::CrewMood { mood: Mood::Hopeful, all: true },
            Effect::SpawnThread {
                thread_type: ThreadType::Anomaly,
                description: "Spent time in the vigil, listening at the edge of \
                    distorted space. Something was there — not a sound exactly, \
                    but a structure in the silence.".into(),
            },
            Effect::Narrative("Sat in the quiet. Heard something. Can't explain what.".into()),
        ],
        "quiet_star_mission_clue" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Mystery,
                description: "The pilgrim described the signal as a question — \
                    mathematical, patient, waiting for comprehension. The pattern \
                    she drew matches nothing in human mathematics, but your \
                    science officer thinks it resembles a topology proof.".into(),
            },
            Effect::CrewMood { mood: Mood::Inspired, all: false },
            Effect::Narrative(
                "The pattern burns in your mind. It means something. You almost understand.".into(),
            ),
        ],
        "quiet_star_record_pattern" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Mystery,
                description: "Recorded the pattern the pilgrim drew on the viewport. \
                    Analysis pending, but initial comparison suggests non-human \
                    mathematical notation.".into(),
            },
            Effect::Narrative("Captured the image. Science will find what faith can't.".into()),
        ],
        "quiet_star_how_know" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "The pilgrim knew about the mission — claims the Order \
                    has contacts who share information about signal-seekers. \
                    Unclear whether this means allies or surveillance.".into(),
            },
            Effect::Narrative("She smiled. 'The distortion tells us who is listening.' Make of that what you will.".into()),
        ],

        // -----------------------------------------------------------------
        // Ashfall Salvage encounters
        // -----------------------------------------------------------------
        "ashfall_repair_credits" => vec![
            Effect::Hull(0.2),
            Effect::RepairModule { module: ModuleTarget::Engine, amount: 0.15 },
            Effect::Resources(-120.0),
            Effect::Narrative("Professional work. No questions asked. Hull integrity restored.".into()),
        ],
        "ashfall_repair_trade" => vec![
            Effect::Hull(0.15),
            Effect::RepairModule { module: ModuleTarget::Engine, amount: 0.1 },
            Effect::JettisonCargo,
            Effect::Narrative("Traded cargo for repairs. Fair deal, out where fair is relative.".into()),
        ],
        "ashfall_probe_services" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "Ashfall Salvage runs more than a repair shop — they \
                    move people, cargo, and information across borders the official \
                    routes don't cross. The frontier has its own economy.".into(),
            },
            Effect::Narrative("Asked around. The frontier has layers you hadn't seen.".into()),
        ],

        // -----------------------------------------------------------------
        // Hegemony Military Command encounters
        // -----------------------------------------------------------------
        "military_probe_intel" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "The military officer let slip that ships returning \
                    from deep-frontier distorted space show anomalous navigation \
                    log corruption. Hegemony Command is tracking it.".into(),
            },
            Effect::Narrative("She said more than she meant to. Or exactly as much as she intended.".into()),
        ],
        "military_ask_routes" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "Learned that Hegemony Military Command has designated \
                    certain frontier routes as restricted — specifically near \
                    systems with high time distortion. They're not saying why.".into(),
            },
            Effect::Narrative("Restricted zones. Near the frontier. Near the signal. That's not a coincidence.".into()),
        ],

        // -----------------------------------------------------------------
        // Spacers' Collective encounters
        // -----------------------------------------------------------------
        "spacers_listen" => vec![
            Effect::CrewStress(-0.05),
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "The spacer's network has noticed unusual shipping \
                    pattern changes — routes being rerouted around certain frontier \
                    systems, contracts drying up near distorted space.".into(),
            },
            Effect::Narrative("Bought a round. Heard three rumors. Two of them might even be true.".into()),
        ],
        "spacers_favor" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Debt,
                description: "The spacer needs a message delivered to a contact at \
                    a frontier system. Simple job, he says. Spacers don't ask for \
                    favors unless the official channels won't work.".into(),
            },
            Effect::AddCargo { item: "Sealed message tube (Spacers' Collective)".into(), quantity: 1 },
            Effect::Narrative("Took the job. A message, a destination, and a handshake.".into()),
        ],

        // -----------------------------------------------------------------
        // Combat and evasion
        // -----------------------------------------------------------------
        "jettison_cargo" => vec![
            Effect::JettisonCargo,
            Effect::CrewMood { mood: Mood::Anxious, all: false },
            Effect::Narrative("Cargo jettisoned. The pirates took it and vanished.".into()),
            Effect::SpawnThread {
                thread_type: ThreadType::Grudge,
                description: "Lost cargo to pirates in unclaimed space. \
                    The crew remembers.".into(),
            },
        ],
        "hold_course" => {
            // Risky — could go either way. For Phase 1, a mild consequence.
            vec![
                Effect::Hull(-0.1),
                Effect::DamageModule { module: ModuleTarget::Engine, amount: 0.1 },
                Effect::CrewStress(0.15),
                Effect::Narrative(
                    "They fired a warning shot that wasn't entirely a warning. \
                     Hull took a hit, but they broke off.".into(),
                ),
                Effect::SpawnThread {
                    thread_type: ThreadType::Grudge,
                    description: "Faced down pirates and survived. They know your ship now.".into(),
                },
            ]
        }
        "negotiate_pirates" => vec![
            Effect::Resources(-100.0),
            Effect::Narrative(
                "Negotiated a 'transit fee.' Everyone pretended it wasn't extortion.".into(),
            ),
        ],
        "flee" => vec![
            Effect::Fuel(-10.0),
            Effect::CrewStress(0.1),
            Effect::Narrative("Burned hard and got clear. Cost fuel, saved everything else.".into()),
        ],

        // -----------------------------------------------------------------
        // Derelicts and exploration
        // -----------------------------------------------------------------
        "board_derelict" => vec![
            Effect::CrewStress(0.1),
            Effect::SpawnThread {
                thread_type: ThreadType::Mystery,
                description: "Found something on the derelict — a log fragment, \
                    a name, a heading. Someone was looking for the same signal.".into(),
            },
            Effect::Narrative("The derelict held no survivors, but it held a story.".into()),
        ],
        "salvage_derelict" => vec![
            Effect::Hull(0.1),
            Effect::RepairModule { module: ModuleTarget::Engine, amount: 0.1 },
            Effect::Resources(50.0),
            Effect::CrewMood { mood: Mood::Determined, all: false },
            Effect::Narrative("Stripped what was useful. Practical. Necessary.".into()),
        ],
        "detailed_scan" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Mystery,
                description: "The survey picked up something faint — a signal \
                    signature that doesn't match any known source.".into(),
            },
            Effect::Narrative("Thorough scan completed. Found more than expected.".into()),
        ],

        // -----------------------------------------------------------------
        // Ancient structures
        // -----------------------------------------------------------------
        "dock_ancient" => vec![
            Effect::CrewStress(0.15),
            Effect::CrewMood { mood: Mood::Inspired, all: true },
            Effect::SpawnThread {
                thread_type: ThreadType::Anomaly,
                description: "Docked with an ancient structure of unknown origin. \
                    The interior defies easy description. Sensors recorded data \
                    that will take weeks to analyze.".into(),
            },
            Effect::TrustProfessional(0.05),
            Effect::Narrative(
                "You went inside. What you found will take time to understand.".into(),
            ),
        ],
        "probe_ancient" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Anomaly,
                description: "Probe returned data from the ancient structure. \
                    Partial readings — enough to confirm it's artificial, \
                    not enough to explain it.".into(),
            },
            Effect::Narrative("The probe came back. Most of the data is incomprehensible.".into()),
        ],
        "observe_ancient" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Anomaly,
                description: "Recorded external observations of an ancient structure. \
                    Detailed but distant.".into(),
            },
            Effect::Narrative("Documented everything from a safe distance.".into()),
        ],

        // -----------------------------------------------------------------
        // Faction encounters
        // -----------------------------------------------------------------
        "comply_checkpoint" => vec![
            Effect::Narrative("Inspection passed without incident.".into()),
        ],
        "comply_and_probe" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "The checkpoint officer mentioned something interesting — \
                    increased patrols, a missing ship, a restricted zone that \
                    wasn't restricted last year.".into(),
            },
            Effect::Narrative(
                "Complied and asked questions. Learned something worth knowing.".into(),
            ),
        ],
        "partial_comply" => vec![
            Effect::CrewStress(0.05),
            Effect::TrustIdeological(0.05),
            Effect::SpawnThread {
                thread_type: ThreadType::Grudge,
                description: "Cited independent vessel rights at a faction checkpoint. \
                    They let you through, but they remembered your ship.".into(),
            },
            Effect::Narrative("Stood on principle. Made an impression. Not sure what kind.".into()),
        ],

        // -----------------------------------------------------------------
        // Crew and personal
        // -----------------------------------------------------------------
        "crew_bond" => vec![
            Effect::CrewStress(-0.1),
            Effect::TrustPersonal(0.05),
            Effect::CrewMood { mood: Mood::Content, all: false },
            Effect::Narrative("Sat with the crew. Listened. Was present.".into()),
        ],
        "captain_opens_up" => vec![
            Effect::CrewStress(-0.1),
            Effect::TrustPersonal(0.1),
            Effect::TrustIdeological(0.03),
            Effect::SpawnThread {
                thread_type: ThreadType::Relationship,
                description: "Shared something personal with the crew. \
                    The walls are a little thinner now.".into(),
            },
            Effect::Narrative("You told them something true. It cost you nothing and everything.".into()),
        ],
        "shore_leave" => vec![
            Effect::CrewStress(-0.2),
            Effect::Resources(-30.0),
            Effect::CrewMood { mood: Mood::Hopeful, all: true },
            Effect::Narrative("The crew dispersed into the station. Came back lighter.".into()),
        ],

        // -----------------------------------------------------------------
        // Repairs
        // -----------------------------------------------------------------
        "navigate_to_repairs" => vec![
            // This is a decision to prioritize — the narrative effect matters most.
            Effect::Narrative("Set course for the nearest repair facility.".into()),
            Effect::AddConcern("Heading for repairs — hull integrity critical.".into()),
        ],
        "field_repair" => vec![
            Effect::Hull(0.15),
            Effect::RepairModule { module: ModuleTarget::Sensors, amount: 0.15 },
            Effect::Resources(-20.0),
            Effect::CrewStress(0.05),
            Effect::TrustProfessional(0.05),
            Effect::Narrative("Jury-rigged repairs. Holding together, for now.".into()),
        ],
        "ignore_damage" => vec![
            Effect::CrewStress(0.1),
            Effect::TrustProfessional(-0.05),
            Effect::CrewMood { mood: Mood::Anxious, all: false },
            Effect::Narrative("Pressed on despite the damage. The crew noticed.".into()),
        ],

        // -----------------------------------------------------------------
        // Information and intel
        // -----------------------------------------------------------------
        "gather_intel" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "Picked up rumors — faction movements, missing ships, \
                    a place that used to be safe and isn't anymore.".into(),
            },
            Effect::Narrative("Listened to the local talk. Filed it away.".into()),
        ],
        "check_jobs" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Debt,
                description: "Took a contract — delivery, investigation, or escort. \
                    Someone is counting on you now.".into(),
            },
            Effect::Narrative("Found work. The kind that keeps the tanks full.".into()),
        ],

        // -----------------------------------------------------------------
        // Smuggling
        // -----------------------------------------------------------------
        "accept_smuggle" => vec![
            Effect::Resources(200.0),
            Effect::AddCargo { item: "Sealed containers (unknown contents)".into(), quantity: 5 },
            Effect::CrewStress(0.1),
            Effect::TrustIdeological(-0.05),
            Effect::SpawnThread {
                thread_type: ThreadType::Debt,
                description: "Carrying unknown cargo across a faction border. \
                    Someone is expecting delivery.".into(),
            },
            Effect::Narrative("Took the job. Didn't ask what's in the containers.".into()),
        ],
        "refuse_smuggle" => vec![
            Effect::TrustIdeological(0.03),
            Effect::Narrative("Turned it down. The man shrugged and found someone else.".into()),
        ],
        "ask_what_cargo" => vec![
            Effect::SpawnThread {
                thread_type: ThreadType::Secret,
                description: "The smuggler described the cargo — medical supplies, \
                    he said. Whether that's true is another question.".into(),
            },
            Effect::Narrative("Asked questions. Got answers. Not sure they were honest.".into()),
        ],

        // -----------------------------------------------------------------
        // Quiet moments and pass-throughs
        // -----------------------------------------------------------------
        "rest" => vec![
            Effect::CrewStress(-0.05),
            Effect::Narrative("Took a moment. The universe waited.".into()),
        ],
        "write_log" => vec![
            Effect::Narrative("Added an entry to the log. Preserving what matters.".into()),
        ],
        "open_navigation" => vec![
            Effect::Pass,
        ],
        "pass" => vec![
            Effect::Pass,
        ],
        "log_and_ignore" => vec![
            Effect::Narrative("Noted the coordinates. Moved on.".into()),
        ],

        // -----------------------------------------------------------------
        // Unknown effect — graceful fallback
        // -----------------------------------------------------------------
        unknown => vec![
            Effect::Narrative(format!(
                "Something happened. [unhandled effect: {}]", unknown
            )),
        ],
    }
}

// ---------------------------------------------------------------------------
// Effect application
// ---------------------------------------------------------------------------

/// Apply a list of effects to the journey state. Returns a report
/// describing what changed, suitable for the event log and display.
pub fn apply_effects(
    effects: &[Effect],
    journey: &mut Journey,
    event_description: &str,
) -> ConsequenceReport {
    let mut changes: Vec<String> = Vec::new();
    let mut threads_spawned: usize = 0;
    let mut narrative_notes: Vec<String> = Vec::new();

    for effect in effects {
        match effect {
            Effect::Fuel(delta) => {
                let before = journey.ship.fuel;
                journey.ship.fuel = (journey.ship.fuel + delta)
                    .max(0.0)
                    .min(journey.ship.fuel_capacity);
                let actual = journey.ship.fuel - before;
                if actual.abs() > 0.01 {
                    if actual > 0.0 {
                        changes.push(format!("Fuel +{:.0}", actual));
                    } else {
                        changes.push(format!("Fuel {:.0}", actual));
                    }
                }
            }

            Effect::Resources(delta) => {
                let before = journey.resources;
                journey.resources = (journey.resources + delta).max(0.0);
                let actual = journey.resources - before;
                if actual.abs() > 0.01 {
                    if actual > 0.0 {
                        changes.push(format!("Resources +{:.0}", actual));
                    } else {
                        changes.push(format!("Resources {:.0}", actual));
                    }
                }
            }

            Effect::Hull(delta) => {
                let before = journey.ship.hull_condition;
                journey.ship.hull_condition = (journey.ship.hull_condition + delta)
                    .max(0.0)
                    .min(1.0);
                let actual = journey.ship.hull_condition - before;
                if actual.abs() > 0.001 {
                    let pct = actual * 100.0;
                    if pct > 0.0 {
                        changes.push(format!("Hull +{:.0}%", pct));
                    } else {
                        changes.push(format!("Hull {:.0}%", pct));
                    }
                }
            }

            Effect::CrewStress(delta) => {
                if journey.crew.is_empty() {
                    continue;
                }
                for member in &mut journey.crew {
                    member.state.stress = (member.state.stress + delta).clamp(0.0, 1.0);
                }
                if *delta > 0.0 {
                    changes.push(format!("Crew stress +{:.0}%", delta * 100.0));
                } else {
                    changes.push(format!("Crew stress {:.0}%", delta * 100.0));
                }
            }

            Effect::CrewMood { mood, all } => {
                if journey.crew.is_empty() {
                    continue;
                }
                if *all {
                    for member in &mut journey.crew {
                        member.state.mood = *mood;
                    }
                    changes.push(format!("Crew mood → {}", mood));
                } else {
                    // Affect the crew member with the highest stress.
                    if let Some(member) = journey.crew.iter_mut()
                        .max_by(|a, b| a.state.stress.partial_cmp(&b.state.stress).unwrap())
                    {
                        member.state.mood = *mood;
                        changes.push(format!("{} mood → {}", member.name, mood));
                    }
                }
            }

            Effect::TrustProfessional(delta) => {
                for member in &mut journey.crew {
                    member.trust.professional = (member.trust.professional + delta).clamp(-1.0, 1.0);
                }
                if delta.abs() > 0.001 {
                    let direction = if *delta > 0.0 { "gained" } else { "lost" };
                    changes.push(format!("Professional trust {}", direction));
                }
            }

            Effect::TrustPersonal(delta) => {
                for member in &mut journey.crew {
                    member.trust.personal = (member.trust.personal + delta).clamp(-1.0, 1.0);
                }
                if delta.abs() > 0.001 {
                    let direction = if *delta > 0.0 { "gained" } else { "lost" };
                    changes.push(format!("Personal trust {}", direction));
                }
            }

            Effect::TrustIdeological(delta) => {
                for member in &mut journey.crew {
                    member.trust.ideological = (member.trust.ideological + delta).clamp(-1.0, 1.0);
                }
                if delta.abs() > 0.001 {
                    let direction = if *delta > 0.0 { "gained" } else { "lost" };
                    changes.push(format!("Ideological trust {}", direction));
                }
            }

            Effect::SpawnThread { thread_type, description } => {
                let thread = Thread {
                    id: Uuid::new_v4(),
                    thread_type: *thread_type,
                    associated_entities: vec![],
                    tension: starting_tension(*thread_type),
                    created_at: journey.time,
                    last_touched: journey.time,
                    resolution: ResolutionState::Open,
                    description: description.clone(),
                };
                journey.threads.push(thread);
                threads_spawned += 1;
                changes.push(format!("New thread: {} — {}", thread_type, short_desc(description)));
            }

            Effect::AddCargo { item, quantity } => {
                let entry = journey.ship.cargo.entry(item.clone()).or_insert(0);
                *entry += quantity;
                changes.push(format!("Cargo +{} {}", quantity, item));
            }

            Effect::JettisonCargo => {
                if !journey.ship.cargo.is_empty() {
                    let items: Vec<String> = journey.ship.cargo.keys().cloned().collect();
                    journey.ship.cargo.clear();
                    changes.push(format!("Jettisoned cargo: {}", items.join(", ")));
                }
            }

            Effect::DamageModule { module, amount } => {
                let m = get_module_mut(&mut journey.ship.modules, *module);
                m.condition = (m.condition - amount).max(0.0);
                changes.push(format!("{} damaged ({:.0}%)", module_name(*module), m.condition * 100.0));
            }

            Effect::RepairModule { module, amount } => {
                let m = get_module_mut(&mut journey.ship.modules, *module);
                m.condition = (m.condition + amount).min(1.0);
                changes.push(format!("{} repaired ({:.0}%)", module_name(*module), m.condition * 100.0));
            }

            Effect::AddConcern(concern) => {
                // Add to the crew member with lowest stress (most bandwidth).
                if let Some(member) = journey.crew.iter_mut()
                    .min_by(|a, b| a.state.stress.partial_cmp(&b.state.stress).unwrap())
                {
                    member.state.active_concerns.push(concern.clone());
                    // Cap at 3 active concerns.
                    if member.state.active_concerns.len() > 3 {
                        member.state.active_concerns.remove(0);
                    }
                }
            }

            Effect::Narrative(text) => {
                narrative_notes.push(text.clone());
            }

            Effect::Pass => {
                // Intentionally nothing.
            }
        }
    }

    // Build the log entry.
    let log_entry = if !narrative_notes.is_empty() {
        narrative_notes.join(" ")
    } else if !changes.is_empty() {
        format!("{} [{}]", event_description, changes.join("; "))
    } else {
        event_description.to_string()
    };

    // Write to the event log.
    journey.event_log.push(GameEvent {
        timestamp: journey.time,
        category: EventCategory::Encounter,
        description: log_entry.clone(),
        associated_entities: vec![],
        consequences: changes.clone(),
    });

    ConsequenceReport {
        changes,
        log_entry,
        threads_spawned,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn get_module_mut(
    modules: &mut starbound_core::ship::ShipModules,
    target: ModuleTarget,
) -> &mut starbound_core::ship::Module {
    match target {
        ModuleTarget::Engine => &mut modules.engine,
        ModuleTarget::Sensors => &mut modules.sensors,
        ModuleTarget::Comms => &mut modules.comms,
        ModuleTarget::Weapons => &mut modules.weapons,
        ModuleTarget::LifeSupport => &mut modules.life_support,
    }
}

fn module_name(target: ModuleTarget) -> &'static str {
    match target {
        ModuleTarget::Engine => "Engine",
        ModuleTarget::Sensors => "Sensors",
        ModuleTarget::Comms => "Comms",
        ModuleTarget::Weapons => "Weapons",
        ModuleTarget::LifeSupport => "Life support",
    }
}

/// How much tension a new thread starts with. Different thread types
/// have different narrative urgency.
fn starting_tension(thread_type: ThreadType) -> f32 {
    match thread_type {
        ThreadType::Relationship => 0.3,
        ThreadType::Mystery => 0.6,
        ThreadType::Debt => 0.5,
        ThreadType::Grudge => 0.7,
        ThreadType::Promise => 0.4,
        ThreadType::Secret => 0.5,
        ThreadType::Anomaly => 0.8,
    }
}

/// Truncate a description to a short snippet for change summaries.
fn short_desc(s: &str) -> String {
    let truncated: String = s.chars().take(50).collect();
    if s.len() > 50 {
        format!("{}...", truncated.trim())
    } else {
        truncated
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
    use starbound_core::time::Timestamp;

    fn test_journey_with_crew() -> Journey {
        let crew = vec![
            CrewMember {
                id: Uuid::new_v4(),
                name: "Test Crew A".into(),
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
                    stress: 0.3,
                    active_concerns: vec![],
                },
                origin: CrewOrigin::Starting,
            },
            CrewMember {
                id: Uuid::new_v4(),
                name: "Test Crew B".into(),
                role: CrewRole::Engineer,
                drives: PersonalityDrives {
                    security: 0.5, freedom: 0.5, purpose: 0.5,
                    connection: 0.5, knowledge: 0.5, justice: 0.5,
                },
                trust: Trust::starting_crew(),
                relationships: HashMap::new(),
                background: String::new(),
                state: CrewState {
                    mood: Mood::Content,
                    stress: 0.5,
                    active_concerns: vec![],
                },
                origin: CrewOrigin::Starting,
            },
        ];

        Journey {
            ship: Ship {
                name: "Test Ship".into(),
                hull_condition: 0.8,
                fuel: 50.0,
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
            time: Timestamp { personal_days: 30.0, galactic_days: 1000.0 },
            resources: 500.0,
            mission: MissionState {
                mission_type: MissionType::Search,
                core_truth: "Test".into(),
                knowledge_nodes: vec![],
            },
            crew,
            threads: vec![],
            event_log: vec![],
            civ_standings: HashMap::new(),
        }
    }

    #[test]
    fn buy_fuel_adds_fuel_removes_resources() {
        let mut journey = test_journey_with_crew();
        let effects = resolve_effects("buy_fuel", &journey);
        let report = apply_effects(&effects, &mut journey, "Bought fuel");

        assert_eq!(journey.ship.fuel, 70.0);
        assert_eq!(journey.resources, 470.0);
        assert!(!report.changes.is_empty());
        assert_eq!(journey.event_log.len(), 1);
    }

    #[test]
    fn fuel_clamped_to_capacity() {
        let mut journey = test_journey_with_crew();
        journey.ship.fuel = 95.0;
        let effects = vec![Effect::Fuel(20.0)];
        apply_effects(&effects, &mut journey, "Overfill test");

        assert_eq!(journey.ship.fuel, 100.0); // Clamped to capacity.
    }

    #[test]
    fn resources_dont_go_negative() {
        let mut journey = test_journey_with_crew();
        journey.resources = 10.0;
        let effects = vec![Effect::Resources(-100.0)];
        apply_effects(&effects, &mut journey, "Broke");

        assert_eq!(journey.resources, 0.0);
    }

    #[test]
    fn crew_stress_clamped() {
        let mut journey = test_journey_with_crew();
        let effects = vec![Effect::CrewStress(2.0)]; // Absurdly high.
        apply_effects(&effects, &mut journey, "Stress test");

        for member in &journey.crew {
            assert!(member.state.stress <= 1.0);
        }
    }

    #[test]
    fn spawn_thread_creates_thread() {
        let mut journey = test_journey_with_crew();
        assert!(journey.threads.is_empty());

        let effects = resolve_effects("board_derelict", &journey);
        let report = apply_effects(&effects, &mut journey, "Boarded a derelict");

        assert_eq!(journey.threads.len(), 1);
        assert_eq!(report.threads_spawned, 1);
        assert_eq!(journey.threads[0].thread_type, ThreadType::Mystery);
        assert_eq!(journey.threads[0].resolution, ResolutionState::Open);
    }

    #[test]
    fn jettison_cargo_clears_cargo() {
        let mut journey = test_journey_with_crew();
        journey.ship.cargo.insert("Spice".into(), 10);
        journey.ship.cargo.insert("Machine parts".into(), 5);

        let effects = vec![Effect::JettisonCargo];
        apply_effects(&effects, &mut journey, "Jettisoned");

        assert!(journey.ship.cargo.is_empty());
    }

    #[test]
    fn module_damage_and_repair() {
        let mut journey = test_journey_with_crew();

        let effects = vec![
            Effect::DamageModule { module: ModuleTarget::Engine, amount: 0.3 },
        ];
        apply_effects(&effects, &mut journey, "Damaged");
        assert!((journey.ship.modules.engine.condition - 0.7).abs() < 0.01);

        let effects = vec![
            Effect::RepairModule { module: ModuleTarget::Engine, amount: 0.2 },
        ];
        apply_effects(&effects, &mut journey, "Repaired");
        assert!((journey.ship.modules.engine.condition - 0.9).abs() < 0.01);
    }

    #[test]
    fn crew_mood_targets_most_stressed() {
        let mut journey = test_journey_with_crew();
        // Crew B has stress 0.5, Crew A has 0.3.
        let effects = vec![Effect::CrewMood { mood: Mood::Anxious, all: false }];
        apply_effects(&effects, &mut journey, "Mood shift");

        // Crew B (highest stress) should have changed.
        assert_eq!(journey.crew[1].state.mood, Mood::Anxious);
        // Crew A should be unchanged.
        assert_eq!(journey.crew[0].state.mood, Mood::Content);
    }

    #[test]
    fn trust_changes_apply_to_all_crew() {
        let mut journey = test_journey_with_crew();
        let before_a = journey.crew[0].trust.personal;
        let before_b = journey.crew[1].trust.personal;

        let effects = vec![Effect::TrustPersonal(0.1)];
        apply_effects(&effects, &mut journey, "Trust test");

        assert!((journey.crew[0].trust.personal - (before_a + 0.1)).abs() < 0.001);
        assert!((journey.crew[1].trust.personal - (before_b + 0.1)).abs() < 0.001);
    }

    #[test]
    fn unknown_effect_produces_narrative() {
        let journey = test_journey_with_crew();
        let effects = resolve_effects("some_future_effect", &journey);

        assert_eq!(effects.len(), 1);
        match &effects[0] {
            Effect::Narrative(text) => {
                assert!(text.contains("some_future_effect"));
            }
            _ => panic!("Unknown effect should produce Narrative"),
        }
    }

    #[test]
    fn expensive_fuel_scales_with_desperation() {
        let mut journey_low = test_journey_with_crew();
        journey_low.ship.fuel = 10.0;

        let mut journey_mid = test_journey_with_crew();
        journey_mid.ship.fuel = 50.0;

        let effects_low = resolve_effects("buy_fuel_expensive", &journey_low);
        let effects_mid = resolve_effects("buy_fuel_expensive", &journey_mid);

        // Low fuel should cost more per unit.
        let cost_low = effects_low.iter().find_map(|e| match e {
            Effect::Resources(d) => Some(d.abs()),
            _ => None,
        }).unwrap();

        let cost_mid = effects_mid.iter().find_map(|e| match e {
            Effect::Resources(d) => Some(d.abs()),
            _ => None,
        }).unwrap();

        assert!(cost_low > cost_mid,
            "Low fuel should cost more: low={:.0}, mid={:.0}", cost_low, cost_mid);
    }

    #[test]
    fn shore_leave_reduces_stress_costs_resources() {
        let mut journey = test_journey_with_crew();
        let initial_stress_a = journey.crew[0].state.stress;
        let initial_resources = journey.resources;

        let effects = resolve_effects("shore_leave", &journey);
        apply_effects(&effects, &mut journey, "Shore leave");

        assert!(journey.crew[0].state.stress < initial_stress_a);
        assert!(journey.resources < initial_resources);
        assert_eq!(journey.crew[0].state.mood, Mood::Hopeful);
    }

    #[test]
    fn hold_course_damages_hull_and_engine() {
        let mut journey = test_journey_with_crew();
        let initial_hull = journey.ship.hull_condition;
        let initial_engine = journey.ship.modules.engine.condition;

        let effects = resolve_effects("hold_course", &journey);
        apply_effects(&effects, &mut journey, "Stood ground");

        assert!(journey.ship.hull_condition < initial_hull);
        assert!(journey.ship.modules.engine.condition < initial_engine);
        assert_eq!(journey.threads.len(), 1); // Grudge thread spawned.
    }

    #[test]
    fn event_log_grows_with_each_application() {
        let mut journey = test_journey_with_crew();
        assert!(journey.event_log.is_empty());

        let effects = resolve_effects("rest", &journey);
        apply_effects(&effects, &mut journey, "Rested");

        let effects = resolve_effects("buy_fuel", &journey);
        apply_effects(&effects, &mut journey, "Refueled");

        assert_eq!(journey.event_log.len(), 2);
    }
}
