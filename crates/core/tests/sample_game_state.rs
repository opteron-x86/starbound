// file: crates/core/tests/sample_game_state.rs
//! Day One validation: create sample entities, serialize to JSON,
//! verify the data model is coherent and the output looks sensible.

use std::collections::HashMap;
use uuid::Uuid;

use starbound_core::crew::*;
use starbound_core::galaxy::*;
use starbound_core::journey::Journey;
use starbound_core::mission::*;
use starbound_core::narrative::*;
use starbound_core::ship::*;
use starbound_core::time::Timestamp;

/// Build a small but representative game state and round-trip through JSON.
#[test]
fn create_and_serialize_sample_game_state() {
    // -- Factions --
    let hegemony_id = Uuid::new_v4();
    let freehold_id = Uuid::new_v4();

    let hegemony = Faction {
        id: hegemony_id,
        name: "Terran Hegemony".into(),
        ethos: FactionEthos {
            expansionist: 0.8,
            isolationist: 0.1,
            militaristic: 0.7,
            diplomatic: 0.3,
            theocratic: 0.1,
            mercantile: 0.5,
            technocratic: 0.6,
            communal: 0.2,
        },
        capabilities: FactionCapabilities {
            size: 0.9,
            wealth: 0.7,
            technology: 0.8,
            military: 0.9,
        },
        relationships: {
            let mut r = HashMap::new();
            r.insert(
                freehold_id,
                FactionDisposition {
                    diplomatic: -0.3,
                    economic: 0.4,
                    military: -0.2,
                },
            );
            r
        },
        internal_dynamics: InternalDynamics {
            stability: 0.6,
            pressures: vec![
                "Reform movement in outer colonies".into(),
                "Military hardliners pushing for expansion".into(),
            ],
        },
    };

    // -- Star Systems --
    let sol_id = Uuid::new_v4();
    let cygnus_id = Uuid::new_v4();

    let sol = StarSystem {
        id: sol_id,
        name: "Sol".into(),
        position: (0.0, 0.0),
        star_type: StarType::YellowDwarf,
        planetary_bodies: vec![PlanetaryBody {
            name: "Earth".into(),
            body_type: BodyType::Gaia,
            features: vec!["Birthworld".into(), "Hegemony capital".into()],
        }],
        controlling_faction: Some(hegemony_id),
        infrastructure_level: InfrastructureLevel::Capital,
        history: vec![HistoryEntry {
            timestamp: Timestamp::zero(),
            description: "Origin of the Terran Hegemony".into(),
        }],
        active_threads: vec![],
    };

    let cygnus_gate = StarSystem {
        id: cygnus_id,
        name: "Cygnus Gate".into(),
        position: (8.3, 2.1),
        star_type: StarType::Binary,
        planetary_bodies: vec![
            PlanetaryBody {
                name: "Cygnus Gate Station".into(),
                body_type: BodyType::Artificial,
                features: vec!["Trade hub".into(), "Contested border zone".into()],
            },
            PlanetaryBody {
                name: "Cygnus III".into(),
                body_type: BodyType::IceWorld,
                features: vec!["Ancient ruins reported".into()],
            },
        ],
        controlling_faction: None,
        infrastructure_level: InfrastructureLevel::Established,
        history: vec![],
        active_threads: vec![],
    };

    // -- Sector --
    let sector = Sector {
        id: Uuid::new_v4(),
        name: "The Near Reach".into(),
        description: "The first settled systems beyond Sol. Old colonies, older grudges.".into(),
        system_ids: vec![sol_id, cygnus_id],
    };

    // -- Connection --
    let connection = Connection {
        system_a: sol_id,
        system_b: cygnus_id,
        distance_ly: 8.6,
        route_type: RouteType::Corridor,
    };

    // -- Ship --
    let ship = Ship {
        name: "The Quiet Reach".into(),
        hull_condition: 0.85,
        fuel: 75.0,
        fuel_capacity: 100.0,
        cargo: {
            let mut c = HashMap::new();
            c.insert("medical supplies".into(), 12);
            c.insert("sealed data cores".into(), 3);
            c
        },
        cargo_capacity: 50,
        modules: ShipModules {
            engine: Module::standard("Kessler-IV Sublight Drive"),
            sensors: Module::standard("Broadband Array"),
            comms: Module::standard("Standard Ansible Relay"),
            weapons: Module {
                variant: "Point Defense Grid".into(),
                condition: 0.7,
                notes: vec!["Port array damaged in Cygnus Gate incident".into()],
            },
            life_support: Module::standard("Closed-Loop Atmospheric"),
        },
    };

    // -- Crew --
    let nav_id = Uuid::new_v4();
    let eng_id = Uuid::new_v4();

    let navigator = CrewMember {
        id: nav_id,
        name: "Lena Vasquez".into(),
        role: CrewRole::Navigator,
        drives: PersonalityDrives {
            security: 0.3,
            freedom: 0.5,
            purpose: 0.7,
            connection: 0.6,
            knowledge: 0.8,
            justice: 0.4,
        },
        trust: Trust::starting_crew(),
        relationships: {
            let mut r = HashMap::new();
            r.insert(
                eng_id,
                CrewRelationship {
                    rapport: 0.4,
                    dynamic: RelationshipDynamic::Friends,
                    notes: vec!["Bonded over shared watch rotations".into()],
                },
            );
            r
        },
        background: "Former cartographer for the Hegemony Survey Corps. Left after \
            a mapping expedition was quietly reclassified as a military operation. \
            Doesn't talk about the details but sometimes stares at the sector charts \
            longer than the job requires."
            .into(),
        state: CrewState {
            mood: Mood::Determined,
            stress: 0.3,
            active_concerns: vec!["Curious about the anomalous readings at Cygnus III".into()],
        },
        origin: CrewOrigin::Starting,
    };

    let engineer = CrewMember {
        id: eng_id,
        name: "Tomás Achebe".into(),
        role: CrewRole::Engineer,
        drives: PersonalityDrives {
            security: 0.7,
            freedom: 0.3,
            purpose: 0.8,
            connection: 0.5,
            knowledge: 0.6,
            justice: 0.5,
        },
        trust: Trust::starting_crew(),
        relationships: {
            let mut r = HashMap::new();
            r.insert(
                nav_id,
                CrewRelationship {
                    rapport: 0.4,
                    dynamic: RelationshipDynamic::Friends,
                    notes: vec![],
                },
            );
            r
        },
        background: "Third-generation station mechanic from Cygnus Gate. \
            Knows the old station better than anyone alive. Joined the crew \
            because the station stopped feeling like home after the Hegemony \
            garrison arrived."
            .into(),
        state: CrewState {
            mood: Mood::Content,
            stress: 0.2,
            active_concerns: vec!["Worrying about the port weapons array".into()],
        },
        origin: CrewOrigin::Starting,
    };

    // -- Mission --
    let clue_a = Uuid::new_v4();
    let clue_b = Uuid::new_v4();
    let clue_c = Uuid::new_v4();

    let mission = MissionState {
        mission_type: MissionType::Search,
        core_truth: "The signal originates from a structure that predates all known \
            civilizations. It is not a beacon. It is a warning."
            .into(),
        knowledge_nodes: vec![
            KnowledgeNode {
                id: clue_a,
                node_type: KnowledgeNodeType::Concrete,
                description: "An ancient relay station in the Cygnus system \
                    broadcasting on a frequency no current civilization uses."
                    .into(),
                discovery_state: DiscoveryState::Discovered,
                dependencies: vec![],
                access_points: vec![
                    "Detected by ship sensors on approach".into(),
                    "Referenced in Cygnus Gate station historical archives".into(),
                ],
                relevance: Relevance::Central,
            },
            KnowledgeNode {
                id: clue_b,
                node_type: KnowledgeNodeType::Conceptual,
                description: "The signal encodes a mathematical structure that \
                    maps to known spatial coordinates — but some coordinates \
                    point to empty space."
                    .into(),
                discovery_state: DiscoveryState::Unknown,
                dependencies: vec![clue_a],
                access_points: vec![
                    "Crew science officer analysis".into(),
                    "Freehold cryptographers (if contacted)".into(),
                ],
                relevance: Relevance::Central,
            },
            KnowledgeNode {
                id: clue_c,
                node_type: KnowledgeNodeType::Relational,
                description: "The 'empty' coordinates once held star systems. \
                    They were consumed. The signal is a list of the dead."
                    .into(),
                discovery_state: DiscoveryState::Unknown,
                dependencies: vec![clue_b],
                access_points: vec![
                    "Cross-reference with ancient stellar catalogs".into(),
                    "A dying alien scholar's final transmission".into(),
                ],
                relevance: Relevance::Central,
            },
        ],
    };

    // -- Threads --
    let thread = Thread {
        id: Uuid::new_v4(),
        thread_type: ThreadType::Mystery,
        associated_entities: vec![cygnus_id, clue_a],
        tension: 0.7,
        created_at: Timestamp::zero(),
        last_touched: Timestamp {
            personal_days: 14.0,
            galactic_days: 14.0,
        },
        resolution: ResolutionState::Open,
        description: "The signal from Cygnus III — what is it, and who built the relay?".into(),
    };

    // -- Journey (complete player state) --
    let journey = Journey {
        ship,
        current_system: cygnus_id,
        time: Timestamp {
            personal_days: 180.0,
            galactic_days: 14_600.0, // ~40 years galactic, 6 months personal
        },
        resources: 2400.0,
        mission,
        crew: vec![navigator, engineer],
        threads: vec![thread],
        event_log: vec![GameEvent {
            timestamp: Timestamp {
                personal_days: 180.0,
                galactic_days: 14_600.0,
            },
            category: EventCategory::Travel,
            description: "Arrived at Cygnus Gate after sublight transit from Sol.".into(),
            associated_entities: vec![sol_id, cygnus_id],
            consequences: vec!["40 galactic years elapsed during transit".into()],
        }],
    };

    // -- Serialize and verify --
    let json = serde_json::to_string_pretty(&journey).expect("Journey should serialize to JSON");

    // Basic sanity checks.
    assert!(json.contains("The Quiet Reach"), "Ship name should appear in JSON");
    assert!(json.contains("Lena Vasquez"), "Crew name should appear");
    assert!(json.contains("Cygnus Gate"), "System name should appear");
    assert!(json.contains("The signal originates"), "Mission truth should appear");

    // Verify we can round-trip.
    let deserialized: Journey =
        serde_json::from_str(&json).expect("JSON should deserialize back to Journey");
    assert_eq!(deserialized.crew.len(), 2);
    assert_eq!(deserialized.mission.discovered_count(), 1);
    assert!(deserialized.time.dilation_ratio() > 50.0, "Dilation should reflect sublight travel");
    assert_eq!(deserialized.ship.name, "The Quiet Reach");

    // Print a snippet for human inspection.
    println!("\n=== Sample Journey State (excerpt) ===\n");
    println!("Ship: {}", deserialized.ship.name);
    println!(
        "Time: {:.1} personal years / {:.1} galactic years (dilation ratio: {:.1}x)",
        deserialized.time.personal_years(),
        deserialized.time.galactic_years(),
        deserialized.time.dilation_ratio()
    );
    println!("Crew: {}", deserialized.crew.len());
    for member in &deserialized.crew {
        println!("  - {} ({})", member.name, member.role);
    }
    println!("Threads: {} open", deserialized.threads.len());
    println!(
        "Mission: {} — {}/{} nodes discovered",
        deserialized.mission.mission_type,
        deserialized.mission.discovered_count(),
        deserialized.mission.knowledge_nodes.len()
    );
    println!("\n=== Full JSON ({} bytes) ===\n", json.len());

    // Also verify the galaxy types independently.
    let galaxy_json =
        serde_json::to_string_pretty(&vec![&sol, &cygnus_gate]).expect("Systems should serialize");
    assert!(galaxy_json.contains("yellow_dwarf"));
    assert!(galaxy_json.contains("binary"));

    let faction_json =
        serde_json::to_string_pretty(&hegemony).expect("Faction should serialize");
    assert!(faction_json.contains("Terran Hegemony"));
    assert!(faction_json.contains("expansionist"));

    let sector_json =
        serde_json::to_string_pretty(&sector).expect("Sector should serialize");
    assert!(sector_json.contains("The Near Reach"));

    let connection_json =
        serde_json::to_string_pretty(&connection).expect("Connection should serialize");
    assert!(connection_json.contains("corridor"));

    println!("All serialization checks passed.");
}

/// Verify timestamp arithmetic behaves correctly.
#[test]
fn timestamp_dilation_math() {
    let ts = Timestamp {
        personal_days: 182.5, // ~6 months
        galactic_days: 14_610.0, // ~40 years
    };
    assert!((ts.personal_years() - 0.5).abs() < 0.01);
    assert!((ts.galactic_years() - 40.0).abs() < 0.1);
    assert!(ts.dilation_ratio() > 79.0);
    assert!(ts.dilation_ratio() < 81.0);
}

/// Verify mission progress calculation.
#[test]
fn mission_progress_tracking() {
    let mission = MissionState {
        mission_type: MissionType::Search,
        core_truth: "test".into(),
        knowledge_nodes: vec![
            KnowledgeNode {
                id: Uuid::new_v4(),
                node_type: KnowledgeNodeType::Concrete,
                description: "First clue".into(),
                discovery_state: DiscoveryState::Connected,
                dependencies: vec![],
                access_points: vec![],
                relevance: Relevance::Central,
            },
            KnowledgeNode {
                id: Uuid::new_v4(),
                node_type: KnowledgeNodeType::Conceptual,
                description: "Second clue".into(),
                discovery_state: DiscoveryState::Discovered,
                dependencies: vec![],
                access_points: vec![],
                relevance: Relevance::Supporting,
            },
            KnowledgeNode {
                id: Uuid::new_v4(),
                node_type: KnowledgeNodeType::Relational,
                description: "Third clue".into(),
                discovery_state: DiscoveryState::Unknown,
                dependencies: vec![],
                access_points: vec![],
                relevance: Relevance::Central,
            },
        ],
    };

    assert_eq!(mission.discovered_count(), 2);
    assert!((mission.progress() - 1.0 / 3.0).abs() < 0.01);
}