// file: crates/game/src/consequences.rs
//! The consequence system — turns player choices into game state changes.
//!
//! Effects are defined as structured `EffectDef` values in event JSON.
//! This module converts them to concrete `Effect` enums and applies
//! them to the journey state.
//!
//! Design principle: effects are deterministic given the same game state.
//! Randomness belongs in the encounter pipeline, not in consequences.
//!
//! ## Redesign additions
//!
//! Some new effects modify world state beyond the Journey (e.g. revealing
//! locations on the system map, changing NPC disposition). These are
//! collected as `DeferredEffect` values in the consequence report for
//! the game loop to process with full world access.

use uuid::Uuid;

use starbound_core::crew::Mood;
use starbound_core::journey::Journey;
use starbound_core::mission::{DiscoveryState, KnowledgeNode, KnowledgeNodeType, Relevance};
use starbound_core::narrative::{
    EventCategory, GameEvent, ResolutionState, Thread, ThreadType,
};

use starbound_encounters::seed_event::EffectDef;

// ---------------------------------------------------------------------------
// Effect types
// ---------------------------------------------------------------------------

/// A single atomic change to the game state.
/// Effects are composed — one choice can produce several.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    // --- Existing effects (unchanged) ---

    /// Add or remove fuel. Clamped to [0, capacity].
    Fuel(f32),
    /// Add or remove supplies. Clamped to [0, capacity].
    Supplies(f32),
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

    // --- New effects (encounter system redesign) ---

    /// Close or transform an existing thread.
    ResolveThread {
        thread_type: ThreadType,
        keyword: String,
        to_state: ResolutionState,
    },
    /// Advance the main quest by adding a knowledge node.
    AddKnowledgeNode { content: String },
    /// Cost personal (and galactic) time.
    TimeCost { hours: f64 },
    /// Change standing with a faction category.
    /// Deferred to the game loop (needs faction registry access).
    FactionStanding {
        faction_category: String,
        delta: f32,
    },
    /// Reveal a hidden location in the current system.
    /// Deferred to the game loop (needs system access).
    DiscoverLocation {
        name: String,
        description: Option<String>,
    },
    /// Shift the player's behavioral profile.
    ReputationShift { label: String, delta: f32 },
    /// Change an NPC's disposition.
    /// Deferred to the game loop (needs NPC registry access).
    NpcDisposition { npc_name: String, delta: f32 },
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

impl ModuleTarget {
    /// Human-readable display name for this module.
    pub fn name(self) -> &'static str {
        match self {
            ModuleTarget::Engine => "Engine",
            ModuleTarget::Sensors => "Sensors",
            ModuleTarget::Comms => "Comms",
            ModuleTarget::Weapons => "Weapons",
            ModuleTarget::LifeSupport => "Life support",
        }
    }
}

// ---------------------------------------------------------------------------
// Deferred effects — require world state beyond Journey
// ---------------------------------------------------------------------------

/// An effect that needs to be processed by the game loop with full
/// world state access. Collected in the consequence report.
#[derive(Debug, Clone)]
pub enum DeferredEffect {
    /// Change standing with a faction category.
    FactionStanding {
        faction_category: String,
        delta: f32,
    },
    /// Reveal a hidden location in the current system.
    DiscoverLocation {
        name: String,
        description: Option<String>,
    },
    /// Change an NPC's disposition toward the player.
    NpcDisposition { npc_name: String, delta: f32 },
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
    /// Effects that require world state beyond Journey.
    /// The game loop should process these after applying the report.
    pub deferred: Vec<DeferredEffect>,
}

// ---------------------------------------------------------------------------
// EffectDef -> Effect conversion
// ---------------------------------------------------------------------------

/// Convert a data-driven `EffectDef` (from event JSON) to a concrete `Effect`.
///
/// This is the single point where authored content meets the game engine.
/// All structural decisions are made in the JSON; this function just
/// translates the data format.
pub fn effect_def_to_effect(def: &EffectDef) -> Effect {
    match def {
        EffectDef::Fuel { delta } => Effect::Fuel(*delta),
        EffectDef::Supplies { delta } => Effect::Supplies(*delta),
        EffectDef::Resources { delta } => Effect::Resources(*delta),
        EffectDef::Hull { delta } => Effect::Hull(*delta),
        EffectDef::CrewStress { delta } => Effect::CrewStress(*delta),
        EffectDef::CrewMood { mood, all } => Effect::CrewMood {
            mood: parse_mood(mood),
            all: *all,
        },
        EffectDef::TrustProfessional { delta } => Effect::TrustProfessional(*delta),
        EffectDef::TrustPersonal { delta } => Effect::TrustPersonal(*delta),
        EffectDef::TrustIdeological { delta } => Effect::TrustIdeological(*delta),
        EffectDef::SpawnThread {
            thread_type,
            description,
        } => Effect::SpawnThread {
            thread_type: parse_thread_type(thread_type),
            description: description.clone(),
        },
        EffectDef::AddCargo { item, quantity } => Effect::AddCargo {
            item: item.clone(),
            quantity: *quantity,
        },
        EffectDef::JettisonCargo {} => Effect::JettisonCargo,
        EffectDef::DamageModule { module, amount } => Effect::DamageModule {
            module: parse_module_target(module),
            amount: *amount,
        },
        EffectDef::RepairModule { module, amount } => Effect::RepairModule {
            module: parse_module_target(module),
            amount: *amount,
        },
        EffectDef::AddConcern { text } => Effect::AddConcern(text.clone()),
        EffectDef::Narrative { text } => Effect::Narrative(text.clone()),
        EffectDef::Pass {} => Effect::Pass,

        // --- New effects ---
        EffectDef::FactionStanding {
            faction_category,
            delta,
        } => Effect::FactionStanding {
            faction_category: faction_category.clone(),
            delta: *delta,
        },
        EffectDef::DiscoverLocation { name, description } => Effect::DiscoverLocation {
            name: name.clone(),
            description: description.clone(),
        },
        EffectDef::ResolveThread {
            thread_type,
            keyword,
            to_state,
        } => Effect::ResolveThread {
            thread_type: parse_thread_type(thread_type),
            keyword: keyword.clone(),
            to_state: parse_resolution_state(to_state),
        },
        EffectDef::AddKnowledgeNode { content } => Effect::AddKnowledgeNode {
            content: content.clone(),
        },
        EffectDef::TimeCost { hours } => Effect::TimeCost { hours: *hours },
        EffectDef::ReputationShift { label, delta } => Effect::ReputationShift {
            label: label.clone(),
            delta: *delta,
        },
        EffectDef::NpcDisposition { npc_name, delta } => Effect::NpcDisposition {
            npc_name: npc_name.clone(),
            delta: *delta,
        },
    }
}

/// Convert a slice of `EffectDef` values to `Effect` values.
pub fn convert_effects(defs: &[EffectDef]) -> Vec<Effect> {
    defs.iter().map(effect_def_to_effect).collect()
}

fn parse_mood(s: &str) -> Mood {
    match s {
        "content" => Mood::Content,
        "anxious" => Mood::Anxious,
        "determined" => Mood::Determined,
        "hopeful" => Mood::Hopeful,
        "frustrated" | "angry" => Mood::Angry,
        "inspired" => Mood::Inspired,
        "grieving" => Mood::Grieving,
        "suspicious" | "withdrawn" => Mood::Withdrawn,
        "restless" => Mood::Restless,
        _ => Mood::Content,
    }
}

fn parse_thread_type(s: &str) -> ThreadType {
    match s {
        "relationship" | "bond" => ThreadType::Relationship,
        "mystery" => ThreadType::Mystery,
        "debt" => ThreadType::Debt,
        "grudge" => ThreadType::Grudge,
        "promise" => ThreadType::Promise,
        "secret" => ThreadType::Secret,
        "anomaly" | "clue" => ThreadType::Anomaly,
        _ => ThreadType::Mystery,
    }
}

fn parse_module_target(s: &str) -> ModuleTarget {
    match s {
        "engine" => ModuleTarget::Engine,
        "sensors" => ModuleTarget::Sensors,
        "comms" => ModuleTarget::Comms,
        "weapons" => ModuleTarget::Weapons,
        "life_support" => ModuleTarget::LifeSupport,
        _ => ModuleTarget::Engine,
    }
}

fn parse_resolution_state(s: &str) -> ResolutionState {
    match s {
        "resolved" => ResolutionState::Resolved,
        "transformed" => ResolutionState::Transformed,
        "partial" => ResolutionState::Partial,
        _ => ResolutionState::Resolved,
    }
}

// ---------------------------------------------------------------------------
// Effect application
// ---------------------------------------------------------------------------

/// Apply a list of effects to the journey state. Returns a report
/// describing what changed, suitable for the event log and display.
///
/// Effects that need world state beyond Journey (DiscoverLocation,
/// FactionStanding, NpcDisposition) are collected in
/// `ConsequenceReport.deferred` for the caller to process.
pub fn apply_effects(
    effects: &[Effect],
    journey: &mut Journey,
    event_description: &str,
) -> ConsequenceReport {
    let mut changes: Vec<String> = Vec::new();
    let mut threads_spawned: usize = 0;
    let mut narrative_notes: Vec<String> = Vec::new();
    let mut deferred: Vec<DeferredEffect> = Vec::new();

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

            Effect::Supplies(delta) => {
                let before = journey.ship.supplies;
                journey.ship.supplies = (journey.ship.supplies + delta)
                    .max(0.0)
                    .min(journey.ship.supply_capacity);
                let actual = journey.ship.supplies - before;
                if actual.abs() > 0.01 {
                    if actual > 0.0 {
                        changes.push(format!("Supplies +{:.0}", actual));
                    } else {
                        changes.push(format!("Supplies {:.0}", actual));
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
                journey.ship.hull_condition =
                    (journey.ship.hull_condition + delta).max(0.0).min(1.0);
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
                    changes.push(format!("Crew mood -> {}", mood));
                } else if let Some(member) = journey
                    .crew
                    .iter_mut()
                    .max_by(|a, b| a.state.stress.partial_cmp(&b.state.stress).unwrap())
                {
                    member.state.mood = *mood;
                    changes.push(format!("{} mood -> {}", member.name, mood));
                }
            }

            Effect::TrustProfessional(delta) => {
                for member in &mut journey.crew {
                    member.trust.professional =
                        (member.trust.professional + delta).clamp(-1.0, 1.0);
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
                    member.trust.ideological =
                        (member.trust.ideological + delta).clamp(-1.0, 1.0);
                }
                if delta.abs() > 0.001 {
                    let direction = if *delta > 0.0 { "gained" } else { "lost" };
                    changes.push(format!("Ideological trust {}", direction));
                }
            }

            Effect::SpawnThread {
                thread_type,
                description,
            } => {
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
                changes.push(format!(
                    "New thread: {} -- {}",
                    thread_type,
                    short_desc(description)
                ));
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
                changes.push(format!(
                    "{} damaged ({:.0}%)",
                    module.name(),
                    m.condition * 100.0
                ));
            }

            Effect::RepairModule { module, amount } => {
                let m = get_module_mut(&mut journey.ship.modules, *module);
                m.condition = (m.condition + amount).min(1.0);
                changes.push(format!(
                    "{} repaired ({:.0}%)",
                    module.name(),
                    m.condition * 100.0
                ));
            }

            Effect::AddConcern(concern) => {
                if let Some(member) = journey
                    .crew
                    .iter_mut()
                    .min_by(|a, b| a.state.stress.partial_cmp(&b.state.stress).unwrap())
                {
                    member.state.active_concerns.push(concern.clone());
                    if member.state.active_concerns.len() > 3 {
                        member.state.active_concerns.remove(0);
                    }
                }
            }

            Effect::Narrative(text) => {
                narrative_notes.push(text.clone());
            }

            Effect::Pass => {}

            // --- New effects (encounter system redesign) ---

            Effect::ResolveThread {
                thread_type,
                keyword,
                to_state,
            } => {
                let keyword_lower = keyword.to_lowercase();
                let resolved = journey.threads.iter_mut().find(|t| {
                    t.thread_type == *thread_type
                        && (t.resolution == ResolutionState::Open
                            || t.resolution == ResolutionState::Partial)
                        && t.description.to_lowercase().contains(&keyword_lower)
                });
                if let Some(thread) = resolved {
                    thread.resolution = *to_state;
                    thread.last_touched = journey.time;
                    changes.push(format!(
                        "Thread {} → {}",
                        short_desc(&thread.description),
                        to_state
                    ));
                }
            }

            Effect::AddKnowledgeNode { content } => {
                let node = KnowledgeNode {
                    id: Uuid::new_v4(),
                    node_type: KnowledgeNodeType::Concrete,
                    description: content.clone(),
                    discovery_state: DiscoveryState::Discovered,
                    dependencies: vec![],
                    access_points: vec!["Encounter discovery".into()],
                    relevance: Relevance::Supporting,
                };
                journey.mission.knowledge_nodes.push(node);
                changes.push(format!("Discovery: {}", short_desc(content)));
            }

            Effect::TimeCost { hours } => {
                let days = hours / 24.0;
                journey.time.personal_days += days;
                // Galactic time is NOT multiplied here — the caller should
                // apply the system's time_factor if appropriate.
                changes.push(format!("Time: {:.1} hours", hours));
            }

            Effect::FactionStanding {
                faction_category,
                delta,
            } => {
                // Deferred — needs faction registry to find matching factions.
                deferred.push(DeferredEffect::FactionStanding {
                    faction_category: faction_category.clone(),
                    delta: *delta,
                });
                let direction = if *delta > 0.0 { "improved" } else { "worsened" };
                changes.push(format!("{} faction standing {}", faction_category, direction));
            }

            Effect::DiscoverLocation { name, description } => {
                // Deferred — needs system map access.
                deferred.push(DeferredEffect::DiscoverLocation {
                    name: name.clone(),
                    description: description.clone(),
                });
                changes.push(format!("Location discovered: {}", name));
            }

            Effect::ReputationShift { label, delta } => {
                // Apply to player profile if the label matches.
                journey.profile.shift_label(label, *delta);
                let direction = if *delta > 0.0 { "grows" } else { "fades" };
                changes.push(format!("Reputation: {} {}", label, direction));
            }

            Effect::NpcDisposition { npc_name, delta } => {
                // Deferred — needs NPC registry access.
                deferred.push(DeferredEffect::NpcDisposition {
                    npc_name: npc_name.clone(),
                    delta: *delta,
                });
                let direction = if *delta > 0.0 {
                    "thinks better of you"
                } else {
                    "thinks less of you"
                };
                changes.push(format!("{} {}", npc_name, direction));
            }
        }
    }

    let log_entry = if !narrative_notes.is_empty() {
        narrative_notes.join(" ")
    } else if !changes.is_empty() {
        format!("{} [{}]", event_description, changes.join("; "))
    } else {
        event_description.to_string()
    };

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
        deferred,
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
    use starbound_core::crew::*;
    use starbound_core::mission::*;
    use starbound_core::reputation::PlayerProfile;
    use starbound_core::ship::*;
    use starbound_core::time::Timestamp;
    use std::collections::HashMap;

    fn test_journey_with_crew() -> Journey {
        let crew = vec![
            CrewMember {
                id: Uuid::new_v4(),
                name: "Test Crew A".into(),
                role: CrewRole::Navigator,
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
                    security: 0.5,
                    freedom: 0.5,
                    purpose: 0.5,
                    connection: 0.5,
                    knowledge: 0.5,
                    justice: 0.5,
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
                supplies: 80.0,
                supply_capacity: 100.0,
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
            time: Timestamp {
                personal_days: 30.0,
                galactic_days: 1000.0,
            },
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
            profile: PlayerProfile::new(),
            active_contracts: vec![],
            discovered_rumors: vec![],
            current_location: None,
        }
    }

    // --- EffectDef -> Effect conversion (existing) ---

    #[test]
    fn effect_def_fuel_converts() {
        let def = EffectDef::Fuel { delta: 20.0 };
        let effect = effect_def_to_effect(&def);
        assert_eq!(effect, Effect::Fuel(20.0));
    }

    #[test]
    fn effect_def_spawn_thread_converts() {
        let def = EffectDef::SpawnThread {
            thread_type: "mystery".into(),
            description: "Something strange.".into(),
        };
        let effect = effect_def_to_effect(&def);
        match effect {
            Effect::SpawnThread {
                thread_type,
                description,
            } => {
                assert_eq!(thread_type, ThreadType::Mystery);
                assert_eq!(description, "Something strange.");
            }
            _ => panic!("Expected SpawnThread"),
        }
    }

    #[test]
    fn effect_def_crew_mood_converts() {
        let def = EffectDef::CrewMood {
            mood: "inspired".into(),
            all: true,
        };
        let effect = effect_def_to_effect(&def);
        assert_eq!(
            effect,
            Effect::CrewMood {
                mood: Mood::Inspired,
                all: true
            }
        );
    }

    #[test]
    fn effect_def_module_repair_converts() {
        let def = EffectDef::RepairModule {
            module: "engine".into(),
            amount: 0.3,
        };
        let effect = effect_def_to_effect(&def);
        assert_eq!(
            effect,
            Effect::RepairModule {
                module: ModuleTarget::Engine,
                amount: 0.3
            }
        );
    }

    #[test]
    fn convert_effects_batch() {
        let defs = vec![
            EffectDef::Fuel { delta: 20.0 },
            EffectDef::Resources { delta: -30.0 },
            EffectDef::Narrative {
                text: "Refueled.".into(),
            },
        ];
        let effects = convert_effects(&defs);
        assert_eq!(effects.len(), 3);
        assert_eq!(effects[0], Effect::Fuel(20.0));
        assert_eq!(effects[1], Effect::Resources(-30.0));
        assert!(matches!(&effects[2], Effect::Narrative(t) if t == "Refueled."));
    }

    // --- New effect conversions ---

    #[test]
    fn effect_def_resolve_thread_converts() {
        let def = EffectDef::ResolveThread {
            thread_type: "mystery".into(),
            keyword: "signal".into(),
            to_state: "resolved".into(),
        };
        let effect = effect_def_to_effect(&def);
        assert_eq!(
            effect,
            Effect::ResolveThread {
                thread_type: ThreadType::Mystery,
                keyword: "signal".into(),
                to_state: ResolutionState::Resolved,
            }
        );
    }

    #[test]
    fn effect_def_faction_standing_converts() {
        let def = EffectDef::FactionStanding {
            faction_category: "guild".into(),
            delta: 0.1,
        };
        let effect = effect_def_to_effect(&def);
        assert_eq!(
            effect,
            Effect::FactionStanding {
                faction_category: "guild".into(),
                delta: 0.1,
            }
        );
    }

    #[test]
    fn effect_def_time_cost_converts() {
        let def = EffectDef::TimeCost { hours: 6.0 };
        let effect = effect_def_to_effect(&def);
        assert_eq!(effect, Effect::TimeCost { hours: 6.0 });
    }

    // --- Effect application (existing) ---

    #[test]
    fn fuel_added_and_clamped() {
        let mut journey = test_journey_with_crew();
        let effects = vec![Effect::Fuel(20.0)];
        let report = apply_effects(&effects, &mut journey, "Fuel test");
        assert_eq!(journey.ship.fuel, 70.0);
        assert!(!report.changes.is_empty());
    }

    #[test]
    fn fuel_clamped_to_capacity() {
        let mut journey = test_journey_with_crew();
        journey.ship.fuel = 95.0;
        let effects = vec![Effect::Fuel(20.0)];
        apply_effects(&effects, &mut journey, "Overfill test");
        assert_eq!(journey.ship.fuel, 100.0);
    }

    #[test]
    fn resources_dont_go_negative() {
        let mut journey = test_journey_with_crew();
        journey.resources = 10.0;
        let effects = vec![Effect::Resources(-100.0)];
        apply_effects(&effects, &mut journey, "Drain test");
        assert_eq!(journey.resources, 0.0);
    }

    #[test]
    fn spawn_thread_adds_to_ledger() {
        let mut journey = test_journey_with_crew();
        assert!(journey.threads.is_empty());
        let effects = vec![Effect::SpawnThread {
            thread_type: ThreadType::Mystery,
            description: "A signal in the dark.".into(),
        }];
        let report = apply_effects(&effects, &mut journey, "Signal");
        assert_eq!(journey.threads.len(), 1);
        assert_eq!(report.threads_spawned, 1);
    }

    #[test]
    fn cargo_jettison_clears_all() {
        let mut journey = test_journey_with_crew();
        journey.ship.cargo.insert("Data cores".into(), 3);
        let effects = vec![Effect::JettisonCargo];
        apply_effects(&effects, &mut journey, "Jettison");
        assert!(journey.ship.cargo.is_empty());
    }

    #[test]
    fn module_damage_and_repair() {
        let mut journey = test_journey_with_crew();
        let effects = vec![Effect::DamageModule {
            module: ModuleTarget::Engine,
            amount: 0.3,
        }];
        apply_effects(&effects, &mut journey, "Damaged");
        assert!((journey.ship.modules.engine.condition - 0.7).abs() < 0.01);

        let effects = vec![Effect::RepairModule {
            module: ModuleTarget::Engine,
            amount: 0.2,
        }];
        apply_effects(&effects, &mut journey, "Repaired");
        assert!((journey.ship.modules.engine.condition - 0.9).abs() < 0.01);
    }

    #[test]
    fn crew_mood_targets_most_stressed() {
        let mut journey = test_journey_with_crew();
        let effects = vec![Effect::CrewMood {
            mood: Mood::Anxious,
            all: false,
        }];
        apply_effects(&effects, &mut journey, "Mood shift");
        assert_eq!(journey.crew[1].state.mood, Mood::Anxious);
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
    fn event_log_grows_with_each_application() {
        let mut journey = test_journey_with_crew();
        assert!(journey.event_log.is_empty());
        apply_effects(&[Effect::CrewStress(-0.05)], &mut journey, "Rested");
        apply_effects(&[Effect::Fuel(20.0)], &mut journey, "Refueled");
        assert_eq!(journey.event_log.len(), 2);
    }

    // --- New effect application ---

    #[test]
    fn resolve_thread_closes_matching_thread() {
        let mut journey = test_journey_with_crew();
        journey.threads.push(Thread {
            id: Uuid::new_v4(),
            thread_type: ThreadType::Mystery,
            associated_entities: vec![],
            tension: 0.6,
            created_at: Timestamp::zero(),
            last_touched: Timestamp::zero(),
            resolution: ResolutionState::Open,
            description: "A strange signal in the dark.".into(),
        });

        let effects = vec![Effect::ResolveThread {
            thread_type: ThreadType::Mystery,
            keyword: "signal".into(),
            to_state: ResolutionState::Resolved,
        }];
        apply_effects(&effects, &mut journey, "Signal resolved");
        assert_eq!(journey.threads[0].resolution, ResolutionState::Resolved);
    }

    #[test]
    fn add_knowledge_node_adds_to_mission() {
        let mut journey = test_journey_with_crew();
        assert!(journey.mission.knowledge_nodes.is_empty());
        let effects = vec![Effect::AddKnowledgeNode {
            content: "The signal originates from a structure.".into(),
        }];
        apply_effects(&effects, &mut journey, "Discovery");
        assert_eq!(journey.mission.knowledge_nodes.len(), 1);
    }

    #[test]
    fn time_cost_advances_personal_time() {
        let mut journey = test_journey_with_crew();
        let before = journey.time.personal_days;
        let effects = vec![Effect::TimeCost { hours: 12.0 }];
        apply_effects(&effects, &mut journey, "Investigation");
        assert!((journey.time.personal_days - (before + 0.5)).abs() < 0.01);
    }

    #[test]
    fn faction_standing_produces_deferred_effect() {
        let mut journey = test_journey_with_crew();
        let effects = vec![Effect::FactionStanding {
            faction_category: "guild".into(),
            delta: 0.1,
        }];
        let report = apply_effects(&effects, &mut journey, "Guild favor");
        assert_eq!(report.deferred.len(), 1);
        assert!(matches!(
            &report.deferred[0],
            DeferredEffect::FactionStanding { faction_category, delta }
                if faction_category == "guild" && (*delta - 0.1).abs() < 0.001
        ));
    }

    #[test]
    fn discover_location_produces_deferred_effect() {
        let mut journey = test_journey_with_crew();
        let effects = vec![Effect::DiscoverLocation {
            name: "Hidden Signal Source".into(),
            description: Some("A faint signal among the asteroids.".into()),
        }];
        let report = apply_effects(&effects, &mut journey, "Discovery");
        assert_eq!(report.deferred.len(), 1);
        assert!(matches!(
            &report.deferred[0],
            DeferredEffect::DiscoverLocation { name, .. } if name == "Hidden Signal Source"
        ));
    }

    // --- Full pipeline ---

    #[test]
    fn full_pipeline_def_to_application() {
        let mut journey = test_journey_with_crew();
        let initial_fuel = journey.ship.fuel;
        let initial_resources = journey.resources;

        let defs = vec![
            EffectDef::Fuel { delta: 20.0 },
            EffectDef::Resources { delta: -30.0 },
            EffectDef::CrewStress { delta: -0.05 },
            EffectDef::Narrative {
                text: "A small kindness at a quiet refueling stop.".into(),
            },
        ];
        let effects = convert_effects(&defs);
        let report = apply_effects(&effects, &mut journey, "buy_fuel_and_talk");

        assert_eq!(journey.ship.fuel, initial_fuel + 20.0);
        assert_eq!(journey.resources, initial_resources - 30.0);
        assert!(report.log_entry.contains("small kindness"));
    }

    #[test]
    fn full_pipeline_compound_effects() {
        let mut journey = test_journey_with_crew();
        let defs = vec![
            EffectDef::Resources { delta: -300.0 },
            EffectDef::Hull { delta: 0.3 },
            EffectDef::RepairModule {
                module: "engine".into(),
                amount: 0.3,
            },
            EffectDef::RepairModule {
                module: "sensors".into(),
                amount: 0.2,
            },
        ];
        let effects = convert_effects(&defs);
        apply_effects(&effects, &mut journey, "Full repair");

        assert!(journey.resources < 500.0);
        assert!(journey.ship.hull_condition > 0.9);
        assert!(journey.ship.modules.engine.condition > 0.9);
    }

    #[test]
    fn effect_def_json_round_trip() {
        let defs = vec![
            EffectDef::Fuel { delta: 20.0 },
            EffectDef::SpawnThread {
                thread_type: "mystery".into(),
                description: "Something found.".into(),
            },
            EffectDef::CrewMood {
                mood: "hopeful".into(),
                all: true,
            },
            EffectDef::Pass {},
        ];
        let json = serde_json::to_string_pretty(&defs).unwrap();
        let parsed: Vec<EffectDef> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0], EffectDef::Fuel { delta: 20.0 });
    }

    #[test]
    fn new_effect_def_json_round_trip() {
        let defs = vec![
            EffectDef::FactionStanding {
                faction_category: "guild".into(),
                delta: 0.1,
            },
            EffectDef::ResolveThread {
                thread_type: "mystery".into(),
                keyword: "signal".into(),
                to_state: "resolved".into(),
            },
            EffectDef::TimeCost { hours: 6.0 },
            EffectDef::DiscoverLocation {
                name: "Hidden Outpost".into(),
                description: Some("Beneath the ice.".into()),
            },
        ];
        let json = serde_json::to_string_pretty(&defs).unwrap();
        let parsed: Vec<EffectDef> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 4);
        assert_eq!(
            parsed[0],
            EffectDef::FactionStanding {
                faction_category: "guild".into(),
                delta: 0.1
            }
        );
    }
}