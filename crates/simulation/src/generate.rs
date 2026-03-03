// file: crates/simulation/src/generate.rs
//! Galaxy generation — deterministic from a seed.
//!
//! One sector, ten systems, two civilizations, six factions.
//! Enough to validate the core loop. Expansion comes later.

use rand::prelude::*;
use std::collections::HashMap;
use uuid::Uuid;

use starbound_core::galaxy::*;
use starbound_core::time::Timestamp;

/// The output of galaxy generation — everything needed to start a game.
pub struct GeneratedGalaxy {
    pub sector: Sector,
    pub systems: Vec<StarSystem>,
    pub civilizations: Vec<Civilization>,
    pub factions: Vec<Faction>,
    pub connections: Vec<Connection>,
}

const SYSTEM_NAMES: [&str; 10] = [
    "Meridian",
    "Cygnus Gate",
    "Voss",
    "Thornfield",
    "Pale Harbor",
    "Acheron",
    "Sunhollow",
    "Drift",
    "Kessler's Remnant",
    "Lament",
];

pub fn generate_galaxy(seed: u64) -> GeneratedGalaxy {
    let mut rng = StdRng::seed_from_u64(seed);

    let mut civilizations = generate_civilizations(&mut rng);
    let mut systems = generate_systems(&mut rng, &civilizations);
    let connections = generate_connections(&systems, &mut rng);

    let factions = generate_factions(&mut rng, &civilizations);

    // Wire faction IDs into their parent civilizations.
    wire_factions_into_civs(&mut civilizations, &factions);

    // Wire source_faction into existing CivPressures where appropriate.
    wire_pressure_sources(&mut civilizations, &factions);

    // Populate faction_presence on every system.
    assign_faction_presence(&mut systems, &factions, &civilizations);

    let sector = Sector {
        id: Uuid::new_v4(),
        name: "The Near Reach".into(),
        description: "The first settled systems beyond the homeworld. \
            Old colonies, older grudges. Two powers and a lot of \
            empty space between them."
            .into(),
        system_ids: systems.iter().map(|s| s.id).collect(),
    };

    GeneratedGalaxy {
        sector,
        systems,
        civilizations,
        factions,
        connections,
    }
}

// ===========================================================================
// Civilization generation
// ===========================================================================

fn generate_civilizations(rng: &mut StdRng) -> Vec<Civilization> {
    let hegemony_id = Uuid::new_v4();
    let freehold_id = Uuid::new_v4();

    let hegemony = Civilization {
        id: hegemony_id,
        name: "Terran Hegemony".into(),
        ethos: CivEthos {
            expansionist: 0.7,
            isolationist: 0.1,
            militaristic: 0.6,
            diplomatic: 0.3,
            theocratic: 0.1,
            mercantile: 0.5,
            technocratic: 0.7,
            communal: 0.2,
        },
        capabilities: CivCapabilities {
            size: 0.8,
            wealth: 0.7,
            technology: 0.8,
            military: 0.7,
        },
        relationships: {
            let mut r = HashMap::new();
            r.insert(
                freehold_id,
                CivDisposition {
                    diplomatic: -0.2,
                    economic: 0.3,
                    military: -0.1,
                },
            );
            r
        },
        internal_dynamics: InternalDynamics {
            stability: 0.6,
            pressures: vec![
                CivPressure { description: "Outer colony autonomy movements gaining support".into(), source_faction: None },
                CivPressure { description: "Military faction pushing for Freehold containment".into(), source_faction: None },
            ],
        },
        faction_ids: vec![],
    };

    let freehold = Civilization {
        id: freehold_id,
        name: "The Freehold Compact".into(),
        ethos: CivEthos {
            expansionist: 0.3,
            isolationist: 0.5,
            militaristic: 0.3,
            diplomatic: 0.6,
            theocratic: 0.0,
            mercantile: 0.8,
            technocratic: 0.4,
            communal: 0.7,
        },
        capabilities: CivCapabilities {
            size: 0.4,
            wealth: 0.6,
            technology: 0.5,
            military: 0.3,
        },
        relationships: {
            let mut r = HashMap::new();
            r.insert(
                hegemony_id,
                CivDisposition {
                    diplomatic: -0.2,
                    economic: 0.3,
                    military: -0.1,
                },
            );
            r
        },
        internal_dynamics: InternalDynamics {
            stability: 0.7,
            pressures: vec![
                CivPressure { description: "Debate over accepting Hegemony trade terms".into(), source_faction: None },
            ],
        },
        faction_ids: vec![],
    };

    // Shuffle order so faction generation isn't always deterministic by position.
    if rng.gen_bool(0.5) {
        vec![hegemony, freehold]
    } else {
        vec![freehold, hegemony]
    }
}

// ===========================================================================
// Faction generation
// ===========================================================================

/// Create the starter factions for the Near Reach.
///
/// Six factions spanning different categories, scopes, and allegiances:
///
/// 1. **Hegemony Military Command** — CivInternal(Hegemony), Military.
///    The Hegemony's intelligence and enforcement arm. Loyalist, aggressive,
///    insular. Present wherever the Hegemony has territory.
///
/// 2. **The Corridor Guild** — Transnational, Economic.
///    Merchant guild operating across both civilizations. Pragmatic,
///    welcoming, diplomatic. Follows trade routes.
///
/// 3. **Spacers' Collective** — Independent, Guild.
///    Pilots' and engineers' union with no civ allegiance. Slightly
///    anti-authority, very open, non-aggressive. Present at every
///    port and outpost.
///
/// 4. **Order of the Quiet Star** — Transnational, Religious.
///    Contemplative order drawn to anomalies and distorted space.
///    Neutral alignment, moderate openness, very subtle methods.
///
/// 5. **Ashfall Salvage** — Independent, Criminal.
///    Frontier salvage outfit operating in the grey area between
///    archaeology and piracy. Anti-authority, moderately open,
///    moderately aggressive.
///
/// 6. **The Lattice** — Transnational, Criminal.
///    Shadowy information broker network. Sells intelligence to
///    anyone who can pay. Deeply pragmatic, insular, extremely subtle.
fn generate_factions(_rng: &mut StdRng, civs: &[Civilization]) -> Vec<Faction> {
    let hegemony_id = civs.iter().find(|c| c.name == "Terran Hegemony").unwrap().id;
    let freehold_id = civs.iter().find(|c| c.name == "The Freehold Compact").unwrap().id;

    vec![
        // 1. Hegemony Military Command
        Faction {
            id: Uuid::new_v4(),
            name: "Hegemony Military Command".into(),
            category: FactionCategory::Military,
            scope: FactionScope::CivInternal { civ_id: hegemony_id },
            ethos: FactionEthos {
                alignment: 0.9,   // loyalist
                openness: 0.2,    // insular
                aggression: 0.8,  // direct/forceful
            },
            influence: {
                let mut m = HashMap::new();
                m.insert(hegemony_id, 0.7);
                m
            },
            player_standing: FactionStanding::unknown(),
            description: "The enforcement and intelligence arm of the Terran Hegemony. \
                Runs border patrols, military installations, and classified research \
                programs. Answers to Hegemony Central Command but operates with \
                considerable autonomy in frontier systems. Known for thoroughness, \
                institutional paranoia, and a tendency to classify everything."
                .into(),
            notable_assets: vec![
                "Meridian Naval Yards".into(),
                "Classified deep-space listening posts".into(),
                "Agent network in contested systems".into(),
            ],
        },

        // 2. The Corridor Guild
        Faction {
            id: Uuid::new_v4(),
            name: "The Corridor Guild".into(),
            category: FactionCategory::Economic,
            scope: FactionScope::Transnational {
                civ_ids: vec![hegemony_id, freehold_id],
            },
            ethos: FactionEthos {
                alignment: 0.1,   // pragmatic, slightly anti-establishment
                openness: 0.8,    // welcoming — traders welcome everyone
                aggression: 0.2,  // diplomatic, deal-makers
            },
            influence: {
                let mut m = HashMap::new();
                m.insert(hegemony_id, 0.3);
                m.insert(freehold_id, 0.5);
                m
            },
            player_standing: FactionStanding::unknown(),
            description: "The dominant merchant guild of the Near Reach. Operates \
                trade posts, negotiates tariffs, and maintains the commercial \
                infrastructure that keeps both civilizations fed and supplied. \
                Officially neutral in Hegemony-Freehold politics; practically, \
                they lean toward whoever offers better terms. Their real power \
                is that both sides need them more than they need either side."
                .into(),
            notable_assets: vec![
                "Cygnus Gate Trading Post".into(),
                "Interstellar cargo fleet".into(),
                "Trade route maps and tariff agreements".into(),
            ],
        },

        // 3. Spacers' Collective
        Faction {
            id: Uuid::new_v4(),
            name: "Spacers' Collective".into(),
            category: FactionCategory::Guild,
            scope: FactionScope::Independent,
            ethos: FactionEthos {
                alignment: -0.3,  // mildly anti-authority
                openness: 0.9,    // very welcoming — solidarity-minded
                aggression: 0.1,  // non-aggressive, cooperative
            },
            influence: {
                let mut m = HashMap::new();
                // Present in both civs but as an outsider voice, not a power player.
                m.insert(hegemony_id, 0.1);
                m.insert(freehold_id, 0.2);
                m
            },
            player_standing: FactionStanding::unknown(),
            description: "A loose professional union of pilots, engineers, and \
                independent spacers. No headquarters, no hierarchy worth mentioning, \
                just a network of mutual aid and shared expertise. They maintain \
                repair yards, swap route intelligence, and look after their own. \
                The kind of organization that exists because space is hard and \
                nobody else will help you when your life support fails between \
                systems."
                .into(),
            notable_assets: vec![
                "Repair yards at major ports".into(),
                "Informal route intelligence network".into(),
                "Emergency beacon response protocol".into(),
            ],
        },

        // 4. Order of the Quiet Star
        Faction {
            id: Uuid::new_v4(),
            name: "Order of the Quiet Star".into(),
            category: FactionCategory::Religious,
            scope: FactionScope::Transnational {
                civ_ids: vec![hegemony_id, freehold_id],
            },
            ethos: FactionEthos {
                alignment: 0.0,   // truly neutral — answers to something beyond politics
                openness: 0.5,    // neither secretive nor evangelical
                aggression: 0.05, // almost entirely non-violent, contemplative
            },
            influence: {
                let mut m = HashMap::new();
                m.insert(hegemony_id, 0.15);
                m.insert(freehold_id, 0.2);
                m
            },
            player_standing: FactionStanding::unknown(),
            description: "A contemplative religious order that believes time \
                distortion is evidence of something greater — a pattern in \
                the fabric of spacetime that rewards careful observation. \
                Their monasteries tend to appear in systems with high time \
                factors. Quiet, patient, occasionally unsettling. They know \
                things about distorted space that nobody else has bothered \
                to learn."
                .into(),
            notable_assets: vec![
                "Monastery at Drift".into(),
                "Extensive records of time-distortion phenomena".into(),
                "Meditation techniques that mitigate temporal disorientation".into(),
            ],
        },

        // 5. Ashfall Salvage
        Faction {
            id: Uuid::new_v4(),
            name: "Ashfall Salvage".into(),
            category: FactionCategory::Criminal,
            scope: FactionScope::Independent,
            ethos: FactionEthos {
                alignment: -0.5,  // anti-authority — operates outside the law
                openness: 0.5,    // will work with anyone who has credits
                aggression: 0.5,  // pragmatic violence — not seeking fights, not avoiding them
            },
            influence: {
                // No formal influence in either civ — they're tolerated, not welcomed.
                HashMap::new()
            },
            player_standing: FactionStanding::unknown(),
            description: "Frontier salvage outfit that picks over derelicts, \
                abandoned stations, and anything else the civilizations left \
                behind. The line between salvage and piracy is a legal distinction \
                they don't spend much time worrying about. Good people to know \
                if you need parts, repairs, or passage through places that don't \
                officially exist on anyone's charts."
                .into(),
            notable_assets: vec![
                "Hidden depot in Acheron system".into(),
                "Salvage fleet — three modified haulers".into(),
                "Black market contacts across the frontier".into(),
            ],
        },

        // 6. The Lattice
        Faction {
            id: Uuid::new_v4(),
            name: "The Lattice".into(),
            category: FactionCategory::Criminal,
            scope: FactionScope::Transnational {
                civ_ids: vec![hegemony_id, freehold_id],
            },
            ethos: FactionEthos {
                alignment: 0.0,   // pragmatic — no ideology, just business
                openness: 0.15,   // deeply insular — you don't find them, they find you
                aggression: 0.1,  // subtle — information is their weapon
            },
            influence: {
                let mut m = HashMap::new();
                m.insert(hegemony_id, 0.2);
                m.insert(freehold_id, 0.15);
                m
            },
            player_standing: FactionStanding::unknown(),
            description: "An information broker network that sells intelligence \
                to anyone who can pay. Nobody knows who runs it. Nobody knows \
                how many nodes it has. What everyone knows is that if you need \
                to find something out — a shipping manifest, a classified patrol \
                route, a person who doesn't want to be found — the Lattice can \
                probably help. For a price. They have a reputation for accuracy \
                and discretion, which is the only currency that matters in their \
                line of work."
                .into(),
            notable_assets: vec![
                "Dead drop network across the Near Reach".into(),
                "Encrypted communications infrastructure".into(),
                "Dossiers on key figures in both civilizations".into(),
            ],
        },
    ]
}

// ===========================================================================
// Wiring factions into civilizations
// ===========================================================================

/// Push faction IDs into each civilization's `faction_ids` list.
/// CivInternal factions go into their parent civ.
/// Transnational factions go into every civ they operate in.
/// Independent factions don't appear in any civ's list.
fn wire_factions_into_civs(civs: &mut [Civilization], factions: &[Faction]) {
    for faction in factions {
        match &faction.scope {
            FactionScope::CivInternal { civ_id } => {
                if let Some(civ) = civs.iter_mut().find(|c| c.id == *civ_id) {
                    civ.faction_ids.push(faction.id);
                }
            }
            FactionScope::Transnational { civ_ids } => {
                for civ_id in civ_ids {
                    if let Some(civ) = civs.iter_mut().find(|c| c.id == *civ_id) {
                        civ.faction_ids.push(faction.id);
                    }
                }
            }
            FactionScope::Independent => {
                // Independent factions aren't claimed by any civ.
            }
        }
    }
}

/// Link existing CivPressure entries to their corresponding factions
/// where the description clearly matches a faction's domain.
fn wire_pressure_sources(civs: &mut [Civilization], factions: &[Faction]) {
    let mil_command_id = factions.iter()
        .find(|f| f.name == "Hegemony Military Command")
        .map(|f| f.id);

    let corridor_guild_id = factions.iter()
        .find(|f| f.name == "The Corridor Guild")
        .map(|f| f.id);

    for civ in civs.iter_mut() {
        for pressure in &mut civ.internal_dynamics.pressures {
            // "Military faction pushing for Freehold containment" → Hegemony Military Command
            if pressure.description.contains("Military faction pushing") {
                pressure.source_faction = mil_command_id;
            }
            // "Debate over accepting Hegemony trade terms" → The Corridor Guild
            if pressure.description.contains("accepting Hegemony trade terms") {
                pressure.source_faction = corridor_guild_id;
            }
        }
    }
}

// ===========================================================================
// Faction presence on systems
// ===========================================================================

/// Distribute factions across star systems based on category, scope,
/// and the character of each system.
///
/// The distribution logic:
///
/// - **Hegemony Military Command**: Strong at Meridian (capital), moderate at
///   Voss (colony), low-visibility presence at Cygnus Gate and Acheron
///   (frontier intelligence).
///
/// - **The Corridor Guild**: Strong at Cygnus Gate (trade hub), moderate at
///   Meridian and Pale Harbor (capitals), present at Thornfield and Sunhollow
///   (Freehold systems with commerce).
///
/// - **Spacers' Collective**: Present everywhere there's a port. Strength
///   correlates with infrastructure. Absent from uninhabited systems.
///
/// - **Order of the Quiet Star**: Drawn to distorted space. Present at
///   Drift (time_factor 2.0) and Kessler's Remnant (8.0). Faint presence
///   at Acheron (1.5). Absent from normal-time systems.
///
/// - **Ashfall Salvage**: Frontier only. Present at Acheron (base of
///   operations), Drift (salvage territory), faint at Kessler's Remnant.
///   Invisible in civilized space.
///
/// - **The Lattice**: Low-visibility everywhere civilized, absent from
///   deep frontier. Slightly stronger at trade hubs and capitals.
fn assign_faction_presence(
    systems: &mut [StarSystem],
    factions: &[Faction],
    _civs: &[Civilization],
) {
    // Look up faction IDs by name.
    let faction_id = |name: &str| -> Uuid {
        factions.iter().find(|f| f.name == name).unwrap().id
    };

    let mil_cmd = faction_id("Hegemony Military Command");
    let corridor = faction_id("The Corridor Guild");
    let spacers = faction_id("Spacers' Collective");
    let quiet_star = faction_id("Order of the Quiet Star");
    let ashfall = faction_id("Ashfall Salvage");
    let lattice = faction_id("The Lattice");

    for system in systems.iter_mut() {
        let mut presence = Vec::new();

        match system.name.as_str() {
            // ---------------------------------------------------------------
            // Meridian — Hegemony capital
            // ---------------------------------------------------------------
            "Meridian" => {
                presence.push(FactionPresence {
                    faction_id: mil_cmd,
                    strength: 0.9,
                    visibility: 1.0,
                    services: vec![FactionService::Missions, FactionService::Intelligence, FactionService::Repair],
                });
                presence.push(FactionPresence {
                    faction_id: corridor,
                    strength: 0.5,
                    visibility: 0.8,
                    services: vec![FactionService::Trade],
                });
                presence.push(FactionPresence {
                    faction_id: spacers,
                    strength: 0.3,
                    visibility: 0.6,
                    services: vec![FactionService::Repair, FactionService::Trade],
                });
                presence.push(FactionPresence {
                    faction_id: lattice,
                    strength: 0.3,
                    visibility: 0.1,
                    services: vec![FactionService::Intelligence],
                });
            }

            // ---------------------------------------------------------------
            // Cygnus Gate — contested trade hub
            // ---------------------------------------------------------------
            "Cygnus Gate" => {
                presence.push(FactionPresence {
                    faction_id: corridor,
                    strength: 0.8,
                    visibility: 1.0,
                    services: vec![FactionService::Trade, FactionService::Missions, FactionService::Repair],
                });
                presence.push(FactionPresence {
                    faction_id: spacers,
                    strength: 0.5,
                    visibility: 0.7,
                    services: vec![FactionService::Repair, FactionService::Trade, FactionService::Training],
                });
                presence.push(FactionPresence {
                    faction_id: mil_cmd,
                    strength: 0.2,
                    visibility: 0.15,
                    services: vec![FactionService::Intelligence],
                });
                presence.push(FactionPresence {
                    faction_id: lattice,
                    strength: 0.4,
                    visibility: 0.1,
                    services: vec![FactionService::Intelligence, FactionService::Smuggling],
                });
            }

            // ---------------------------------------------------------------
            // Voss — Hegemony colony
            // ---------------------------------------------------------------
            "Voss" => {
                presence.push(FactionPresence {
                    faction_id: mil_cmd,
                    strength: 0.5,
                    visibility: 0.8,
                    services: vec![FactionService::Missions, FactionService::Repair],
                });
                presence.push(FactionPresence {
                    faction_id: spacers,
                    strength: 0.2,
                    visibility: 0.5,
                    services: vec![FactionService::Repair],
                });
                presence.push(FactionPresence {
                    faction_id: lattice,
                    strength: 0.15,
                    visibility: 0.05,
                    services: vec![FactionService::Intelligence],
                });
            }

            // ---------------------------------------------------------------
            // Thornfield — Freehold, established
            // ---------------------------------------------------------------
            "Thornfield" => {
                presence.push(FactionPresence {
                    faction_id: corridor,
                    strength: 0.4,
                    visibility: 0.7,
                    services: vec![FactionService::Trade],
                });
                presence.push(FactionPresence {
                    faction_id: spacers,
                    strength: 0.3,
                    visibility: 0.6,
                    services: vec![FactionService::Repair, FactionService::Trade],
                });
                presence.push(FactionPresence {
                    faction_id: lattice,
                    strength: 0.2,
                    visibility: 0.05,
                    services: vec![FactionService::Intelligence],
                });
            }

            // ---------------------------------------------------------------
            // Pale Harbor — Freehold capital
            // ---------------------------------------------------------------
            "Pale Harbor" => {
                presence.push(FactionPresence {
                    faction_id: corridor,
                    strength: 0.6,
                    visibility: 0.9,
                    services: vec![FactionService::Trade, FactionService::Missions],
                });
                presence.push(FactionPresence {
                    faction_id: spacers,
                    strength: 0.4,
                    visibility: 0.7,
                    services: vec![FactionService::Repair, FactionService::Trade, FactionService::Training],
                });
                presence.push(FactionPresence {
                    faction_id: lattice,
                    strength: 0.3,
                    visibility: 0.1,
                    services: vec![FactionService::Intelligence],
                });
            }

            // ---------------------------------------------------------------
            // Acheron — frontier outpost, mild time drift
            // ---------------------------------------------------------------
            "Acheron" => {
                presence.push(FactionPresence {
                    faction_id: ashfall,
                    strength: 0.7,
                    visibility: 0.4,
                    services: vec![FactionService::Trade, FactionService::Repair, FactionService::Smuggling, FactionService::Shelter],
                });
                presence.push(FactionPresence {
                    faction_id: spacers,
                    strength: 0.3,
                    visibility: 0.5,
                    services: vec![FactionService::Repair],
                });
                presence.push(FactionPresence {
                    faction_id: mil_cmd,
                    strength: 0.15,
                    visibility: 0.05,
                    services: vec![FactionService::Intelligence],
                });
                presence.push(FactionPresence {
                    faction_id: quiet_star,
                    strength: 0.15,
                    visibility: 0.3,
                    services: vec![FactionService::Training],
                });
            }

            // ---------------------------------------------------------------
            // Sunhollow — Freehold border colony
            // ---------------------------------------------------------------
            "Sunhollow" => {
                presence.push(FactionPresence {
                    faction_id: corridor,
                    strength: 0.3,
                    visibility: 0.6,
                    services: vec![FactionService::Trade],
                });
                presence.push(FactionPresence {
                    faction_id: spacers,
                    strength: 0.25,
                    visibility: 0.5,
                    services: vec![FactionService::Repair],
                });
            }

            // ---------------------------------------------------------------
            // Drift — unclaimed frontier, time_factor 2.0
            // ---------------------------------------------------------------
            "Drift" => {
                presence.push(FactionPresence {
                    faction_id: quiet_star,
                    strength: 0.6,
                    visibility: 0.7,
                    services: vec![FactionService::Shelter, FactionService::Training, FactionService::Intelligence],
                });
                presence.push(FactionPresence {
                    faction_id: ashfall,
                    strength: 0.4,
                    visibility: 0.3,
                    services: vec![FactionService::Trade, FactionService::Smuggling, FactionService::Repair],
                });
                presence.push(FactionPresence {
                    faction_id: spacers,
                    strength: 0.15,
                    visibility: 0.4,
                    services: vec![FactionService::Repair],
                });
            }

            // ---------------------------------------------------------------
            // Kessler's Remnant — deep frontier, time_factor 8.0
            // ---------------------------------------------------------------
            "Kessler's Remnant" => {
                presence.push(FactionPresence {
                    faction_id: quiet_star,
                    strength: 0.4,
                    visibility: 0.6,
                    services: vec![FactionService::Shelter, FactionService::Training],
                });
                presence.push(FactionPresence {
                    faction_id: ashfall,
                    strength: 0.2,
                    visibility: 0.15,
                    services: vec![FactionService::Trade, FactionService::Smuggling],
                });
            }

            // ---------------------------------------------------------------
            // Lament — edge of known space, time_factor 25.0
            // ---------------------------------------------------------------
            "Lament" => {
                // Almost nobody comes here. The Order has a lone hermitage.
                presence.push(FactionPresence {
                    faction_id: quiet_star,
                    strength: 0.2,
                    visibility: 0.4,
                    services: vec![FactionService::Shelter],
                });
            }

            _ => {}
        }

        system.faction_presence = presence;
    }
}

// ===========================================================================
// System generation
// ===========================================================================

fn generate_systems(rng: &mut StdRng, civs: &[Civilization]) -> Vec<StarSystem> {
    // Find civ IDs by name (order may be shuffled).
    let hegemony_id = civs.iter().find(|f| f.name == "Terran Hegemony").unwrap().id;
    let freehold_id = civs.iter().find(|f| f.name == "The Freehold Compact").unwrap().id;

    let star_types = [
        StarType::YellowDwarf,
        StarType::Binary,
        StarType::RedDwarf,
        StarType::RedDwarf,
        StarType::BlueGiant,
        StarType::WhiteDwarf,
        StarType::YellowDwarf,
        StarType::RedDwarf,
        StarType::Neutron,
        StarType::Anomalous,
    ];

    // Position systems in a rough cluster — spread across ~30 light-year region
    // with some structure: Hegemony systems left, Freehold right, contested middle.
    let base_positions: [(f64, f64); 10] = [
        (2.0, 5.0),    // Meridian — Hegemony capital
        (8.0, 3.0),    // Cygnus Gate — contested trade hub
        (5.0, 8.0),    // Voss — Hegemony border
        (12.0, 6.0),   // Thornfield — Freehold
        (18.0, 4.0),   // Pale Harbor — Freehold capital
        (6.0, -2.0),   // Acheron — frontier
        (15.0, 9.0),   // Sunhollow — Freehold border
        (10.0, -1.0),  // Drift — unclaimed frontier
        (20.0, 0.0),   // Kessler's Remnant — deep frontier
        (25.0, 7.0),   // Lament — edge of known space
    ];

    // Civ assignments: some Hegemony, some Freehold, some unclaimed.
    let civ_assignments: [Option<usize>; 10] = [
        Some(0), // Meridian → Hegemony
        None,    // Cygnus Gate → contested
        Some(0), // Voss → Hegemony
        Some(1), // Thornfield → Freehold
        Some(1), // Pale Harbor → Freehold
        None,    // Acheron → unclaimed
        Some(1), // Sunhollow → Freehold
        None,    // Drift → unclaimed
        None,    // Kessler's Remnant → unclaimed
        None,    // Lament → unclaimed
    ];

    let civ_ids = [hegemony_id, freehold_id];

    let infrastructure_levels = [
        InfrastructureLevel::Capital,
        InfrastructureLevel::Hub,
        InfrastructureLevel::Colony,
        InfrastructureLevel::Established,
        InfrastructureLevel::Capital,
        InfrastructureLevel::Outpost,
        InfrastructureLevel::Colony,
        InfrastructureLevel::Outpost,
        InfrastructureLevel::None,
        InfrastructureLevel::None,
    ];

    // Time distortion factors — tied to star type and narrative role.
    //
    // Settled space is normal. Frontier gets weird. The edge is dangerous.
    // The mission leads toward Lament; every step further costs more time.
    //
    // Factor | Feel
    // -------|------
    //   1.0  | Normal — a day is a day. Your Marco Polo circuit.
    //   1.5  | Mild drift — spend a week, lose 10 days. Noticeable.
    //   2.0  | Frontier weird — a week costs two weeks elsewhere.
    //   8.0  | Serious — a week costs nearly two months. Plan carefully.
    //  25.0  | Extreme — a week costs half a year. The granddaughter moment.
    let time_factors: [f64; 10] = [
        1.0,    // Meridian — Hegemony capital, stable yellow dwarf
        1.0,    // Cygnus Gate — trade hub, binary but well-studied
        1.0,    // Voss — Hegemony colony, normal red dwarf
        1.0,    // Thornfield — Freehold, normal red dwarf
        1.0,    // Pale Harbor — Freehold capital, blue giant but compensated
        1.5,    // Acheron — frontier outpost, white dwarf, mild drift
        1.0,    // Sunhollow — Freehold border, normal yellow dwarf
        2.0,    // Drift — unclaimed frontier, red dwarf near a dense remnant
        8.0,    // Kessler's Remnant — neutron star, serious distortion
        25.0,   // Lament — anomalous, extreme distortion, edge of known space
    ];

    let mut systems = Vec::with_capacity(10);

    for i in 0..10 {
        // Add small random jitter to positions.
        let jitter_x: f64 = rng.gen_range(-1.0..1.0);
        let jitter_y: f64 = rng.gen_range(-1.0..1.0);
        let pos = (
            base_positions[i].0 + jitter_x,
            base_positions[i].1 + jitter_y,
        );

        let controlling_civ = civ_assignments[i].map(|idx| civ_ids[idx]);

        let planets = generate_planets(SYSTEM_NAMES[i], star_types[i], rng);

        let history = if infrastructure_levels[i] != InfrastructureLevel::None {
            vec![HistoryEntry {
                timestamp: Timestamp::zero(),
                description: format!("{} founded.", SYSTEM_NAMES[i]),
            }]
        } else {
            vec![]
        };

        systems.push(StarSystem {
            id: Uuid::new_v4(),
            name: SYSTEM_NAMES[i].into(),
            position: pos,
            star_type: star_types[i],
            planetary_bodies: planets,
            controlling_civ,
            infrastructure_level: infrastructure_levels[i],
            history,
            active_threads: vec![],
            time_factor: time_factors[i],
            // Faction presence is assigned after system creation
            // by assign_faction_presence().
            faction_presence: vec![],
        });
    }

    systems
}

fn generate_planets(system_name: &str, star_type: StarType, rng: &mut StdRng) -> Vec<PlanetaryBody> {
    let count = match star_type {
        StarType::Neutron | StarType::BlackHole => rng.gen_range(0..=1),
        StarType::BlueGiant => rng.gen_range(1..=3),
        StarType::Anomalous => rng.gen_range(0..=2),
        _ => rng.gen_range(1..=5),
    };

    let body_types = [
        BodyType::Terrestrial,
        BodyType::GasGiant,
        BodyType::IceWorld,
        BodyType::Barren,
        BodyType::Oceanic,
    ];

    (0..count)
        .map(|i| {
            let body_type = body_types[rng.gen_range(0..body_types.len())];
            PlanetaryBody {
                name: format!("{} {}", system_name, roman_numeral(i + 1)),
                body_type,
                features: vec![],
            }
        })
        .collect()
}

// ===========================================================================
// Connection generation
// ===========================================================================

/// Connect systems based on proximity. Every system connects to its
/// nearest neighbor (so the graph is connected), plus additional edges
/// for systems within a threshold distance.
fn generate_connections(systems: &[StarSystem], rng: &mut StdRng) -> Vec<Connection> {
    let mut connections: Vec<Connection> = Vec::new();
    let mut connected_pairs: Vec<(Uuid, Uuid)> = Vec::new();

    let has_edge = |pairs: &[(Uuid, Uuid)], a: Uuid, b: Uuid| -> bool {
        pairs.iter().any(|(x, y)| (*x == a && *y == b) || (*x == b && *y == a))
    };

    // Step 1: Connect each system to its nearest neighbor (ensures connectivity).
    for i in 0..systems.len() {
        let mut nearest_idx = if i == 0 { 1 } else { 0 };
        let mut nearest_dist = distance(&systems[i], &systems[nearest_idx]);

        for j in 0..systems.len() {
            if j == i { continue; }
            let d = distance(&systems[i], &systems[j]);
            if d < nearest_dist {
                nearest_dist = d;
                nearest_idx = j;
            }
        }

        if !has_edge(&connected_pairs, systems[i].id, systems[nearest_idx].id) {
            let route = classify_route(nearest_dist, rng);
            connections.push(Connection {
                system_a: systems[i].id,
                system_b: systems[nearest_idx].id,
                distance_ly: nearest_dist,
                route_type: route,
            });
            connected_pairs.push((systems[i].id, systems[nearest_idx].id));
        }
    }

    // Step 2: Add edges for systems within 12 light-years of each other.
    let threshold = 12.0;
    for i in 0..systems.len() {
        for j in (i + 1)..systems.len() {
            if has_edge(&connected_pairs, systems[i].id, systems[j].id) {
                continue;
            }
            let d = distance(&systems[i], &systems[j]);
            if d <= threshold {
                let route = classify_route(d, rng);
                connections.push(Connection {
                    system_a: systems[i].id,
                    system_b: systems[j].id,
                    distance_ly: d,
                    route_type: route,
                });
                connected_pairs.push((systems[i].id, systems[j].id));
            }
        }
    }

    // Step 3: Ensure at least one long-range corridor between the two
    // capital systems (Meridian and Pale Harbor) for narrative purposes.
    let meridian = &systems[0];
    let pale_harbor = &systems[4];
    if !has_edge(&connected_pairs, meridian.id, pale_harbor.id) {
        let d = distance(meridian, pale_harbor);
        connections.push(Connection {
            system_a: meridian.id,
            system_b: pale_harbor.id,
            distance_ly: d,
            route_type: RouteType::Corridor,
        });
    }

    connections
}

fn distance(a: &StarSystem, b: &StarSystem) -> f64 {
    let dx = a.position.0 - b.position.0;
    let dy = a.position.1 - b.position.1;
    (dx * dx + dy * dy).sqrt()
}

fn classify_route(distance_ly: f64, rng: &mut StdRng) -> RouteType {
    if distance_ly > 15.0 {
        RouteType::Corridor
    } else if rng.gen_bool(0.15) {
        RouteType::Hazardous
    } else {
        RouteType::Open
    }
}

fn roman_numeral(n: usize) -> &'static str {
    match n {
        1 => "I",
        2 => "II",
        3 => "III",
        4 => "IV",
        5 => "V",
        _ => "VI",
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // -----------------------------------------------------------------------
    // Existing tests (preserved)
    // -----------------------------------------------------------------------

    #[test]
    fn generates_consistent_galaxy_from_seed() {
        let g1 = generate_galaxy(42);
        let g2 = generate_galaxy(42);

        assert_eq!(g1.systems.len(), 10);
        assert_eq!(g1.civilizations.len(), 2);
        assert_eq!(g1.systems.len(), g2.systems.len());

        // Same seed → same system names in same order.
        for (a, b) in g1.systems.iter().zip(g2.systems.iter()) {
            assert_eq!(a.name, b.name);
        }
    }

    #[test]
    fn all_systems_have_at_least_one_connection() {
        let galaxy = generate_galaxy(123);
        for sys in &galaxy.systems {
            let has_conn = galaxy.connections.iter().any(|c| {
                c.system_a == sys.id || c.system_b == sys.id
            });
            assert!(has_conn, "System {} has no connections", sys.name);
        }
    }

    #[test]
    fn civ_assignments_are_sensible() {
        let galaxy = generate_galaxy(42);
        let hegemony = galaxy.civilizations.iter().find(|f| f.name == "Terran Hegemony").unwrap();
        let freehold = galaxy.civilizations.iter().find(|f| f.name == "The Freehold Compact").unwrap();

        let hegemony_systems: Vec<_> = galaxy.systems.iter()
            .filter(|s| s.controlling_civ == Some(hegemony.id))
            .collect();
        let freehold_systems: Vec<_> = galaxy.systems.iter()
            .filter(|s| s.controlling_civ == Some(freehold.id))
            .collect();
        let unclaimed: Vec<_> = galaxy.systems.iter()
            .filter(|s| s.controlling_civ.is_none())
            .collect();

        assert_eq!(hegemony_systems.len(), 2, "Hegemony should control 2 systems");
        assert_eq!(freehold_systems.len(), 3, "Freehold should control 3 systems");
        assert_eq!(unclaimed.len(), 5, "5 systems should be unclaimed");
    }

    #[test]
    fn different_seeds_produce_different_positions() {
        let g1 = generate_galaxy(1);
        let g2 = generate_galaxy(2);

        // Positions should differ due to jitter (names are the same).
        let pos_differ = g1.systems.iter().zip(g2.systems.iter())
            .any(|(a, b)| a.position != b.position);
        assert!(pos_differ, "Different seeds should produce different positions");
    }

    #[test]
    fn time_factors_assigned_correctly() {
        let galaxy = generate_galaxy(42);

        // Settled systems should be normal time.
        let meridian = galaxy.systems.iter().find(|s| s.name == "Meridian").unwrap();
        assert_eq!(meridian.time_factor, 1.0);

        let cygnus = galaxy.systems.iter().find(|s| s.name == "Cygnus Gate").unwrap();
        assert_eq!(cygnus.time_factor, 1.0);

        // Frontier should have mild distortion.
        let acheron = galaxy.systems.iter().find(|s| s.name == "Acheron").unwrap();
        assert!(acheron.time_factor > 1.0, "Acheron should have time distortion");

        // Edge systems should have serious distortion.
        let kessler = galaxy.systems.iter().find(|s| s.name == "Kessler's Remnant").unwrap();
        assert!(kessler.time_factor >= 8.0, "Kessler's Remnant should have serious distortion");

        let lament = galaxy.systems.iter().find(|s| s.name == "Lament").unwrap();
        assert!(lament.time_factor >= 25.0, "Lament should have extreme distortion");
    }

    // -----------------------------------------------------------------------
    // Phase B: Faction generation tests
    // -----------------------------------------------------------------------

    #[test]
    fn generates_six_factions() {
        let galaxy = generate_galaxy(42);
        assert_eq!(galaxy.factions.len(), 6, "Should generate exactly 6 factions");
    }

    #[test]
    fn faction_generation_is_deterministic() {
        let g1 = generate_galaxy(42);
        let g2 = generate_galaxy(42);

        assert_eq!(g1.factions.len(), g2.factions.len());

        // Same seed → same faction names in same order.
        for (a, b) in g1.factions.iter().zip(g2.factions.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.category, b.category);
        }
    }

    #[test]
    fn all_faction_ids_are_unique() {
        let galaxy = generate_galaxy(42);
        let ids: HashSet<Uuid> = galaxy.factions.iter().map(|f| f.id).collect();
        assert_eq!(ids.len(), galaxy.factions.len(), "All faction IDs should be unique");
    }

    #[test]
    fn faction_categories_are_diverse() {
        let galaxy = generate_galaxy(42);
        let categories: HashSet<FactionCategory> = galaxy.factions.iter().map(|f| f.category).collect();
        // We have Military, Economic, Guild, Religious, Criminal (x2).
        assert!(categories.len() >= 4, "Factions should span at least 4 categories, got {}", categories.len());
    }

    #[test]
    fn faction_ethos_values_in_range() {
        let galaxy = generate_galaxy(42);
        for faction in &galaxy.factions {
            assert!(
                faction.ethos.alignment >= -1.0 && faction.ethos.alignment <= 1.0,
                "Faction {} alignment out of range: {}", faction.name, faction.ethos.alignment,
            );
            assert!(
                faction.ethos.openness >= 0.0 && faction.ethos.openness <= 1.0,
                "Faction {} openness out of range: {}", faction.name, faction.ethos.openness,
            );
            assert!(
                faction.ethos.aggression >= 0.0 && faction.ethos.aggression <= 1.0,
                "Faction {} aggression out of range: {}", faction.name, faction.ethos.aggression,
            );
        }
    }

    #[test]
    fn faction_influence_references_valid_civ_ids() {
        let galaxy = generate_galaxy(42);
        let civ_ids: HashSet<Uuid> = galaxy.civilizations.iter().map(|c| c.id).collect();

        for faction in &galaxy.factions {
            for civ_id in faction.influence.keys() {
                assert!(
                    civ_ids.contains(civ_id),
                    "Faction {} has influence entry for non-existent civ {}",
                    faction.name, civ_id,
                );
            }
        }
    }

    #[test]
    fn faction_influence_values_in_range() {
        let galaxy = generate_galaxy(42);
        for faction in &galaxy.factions {
            for (&civ_id, &influence) in &faction.influence {
                assert!(
                    influence >= 0.0 && influence <= 1.0,
                    "Faction {} has out-of-range influence {} in civ {}",
                    faction.name, influence, civ_id,
                );
            }
        }
    }

    #[test]
    fn factions_wired_into_civilizations() {
        let galaxy = generate_galaxy(42);
        let hegemony = galaxy.civilizations.iter().find(|c| c.name == "Terran Hegemony").unwrap();
        let freehold = galaxy.civilizations.iter().find(|c| c.name == "The Freehold Compact").unwrap();

        // Hegemony should have Military Command (internal) plus transnational factions.
        assert!(
            !hegemony.faction_ids.is_empty(),
            "Hegemony should have faction IDs",
        );

        // Freehold should have transnational factions.
        assert!(
            !freehold.faction_ids.is_empty(),
            "Freehold should have faction IDs",
        );

        // All faction IDs in civ lists should reference actual factions.
        let faction_ids: HashSet<Uuid> = galaxy.factions.iter().map(|f| f.id).collect();
        for civ in &galaxy.civilizations {
            for fid in &civ.faction_ids {
                assert!(
                    faction_ids.contains(fid),
                    "Civ {} references non-existent faction {}",
                    civ.name, fid,
                );
            }
        }
    }

    #[test]
    fn independent_factions_not_in_any_civ() {
        let galaxy = generate_galaxy(42);
        let all_civ_faction_ids: HashSet<Uuid> = galaxy.civilizations.iter()
            .flat_map(|c| c.faction_ids.iter())
            .copied()
            .collect();

        for faction in &galaxy.factions {
            if matches!(faction.scope, FactionScope::Independent) {
                assert!(
                    !all_civ_faction_ids.contains(&faction.id),
                    "Independent faction {} should not appear in any civ's faction_ids",
                    faction.name,
                );
            }
        }
    }

    #[test]
    fn civ_internal_faction_only_in_parent_civ() {
        let galaxy = generate_galaxy(42);

        for faction in &galaxy.factions {
            if let FactionScope::CivInternal { civ_id } = &faction.scope {
                // Should be in the parent civ's list.
                let parent = galaxy.civilizations.iter().find(|c| c.id == *civ_id).unwrap();
                assert!(
                    parent.faction_ids.contains(&faction.id),
                    "CivInternal faction {} should be in parent civ {}",
                    faction.name, parent.name,
                );

                // Should NOT be in any other civ's list.
                for civ in &galaxy.civilizations {
                    if civ.id != *civ_id {
                        assert!(
                            !civ.faction_ids.contains(&faction.id),
                            "CivInternal faction {} should NOT be in non-parent civ {}",
                            faction.name, civ.name,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn transnational_factions_in_all_listed_civs() {
        let galaxy = generate_galaxy(42);

        for faction in &galaxy.factions {
            if let FactionScope::Transnational { civ_ids } = &faction.scope {
                for civ_id in civ_ids {
                    let civ = galaxy.civilizations.iter().find(|c| c.id == *civ_id).unwrap();
                    assert!(
                        civ.faction_ids.contains(&faction.id),
                        "Transnational faction {} should be in civ {}",
                        faction.name, civ.name,
                    );
                }
            }
        }
    }

    #[test]
    fn pressure_sources_wired_to_valid_factions() {
        let galaxy = generate_galaxy(42);
        let faction_ids: HashSet<Uuid> = galaxy.factions.iter().map(|f| f.id).collect();

        for civ in &galaxy.civilizations {
            for pressure in &civ.internal_dynamics.pressures {
                if let Some(source_id) = pressure.source_faction {
                    assert!(
                        faction_ids.contains(&source_id),
                        "Pressure '{}' in {} references non-existent faction {}",
                        pressure.description, civ.name, source_id,
                    );
                }
            }
        }
    }

    #[test]
    fn some_pressures_have_faction_sources() {
        let galaxy = generate_galaxy(42);
        let sourced: usize = galaxy.civilizations.iter()
            .flat_map(|c| c.internal_dynamics.pressures.iter())
            .filter(|p| p.source_faction.is_some())
            .count();
        assert!(
            sourced >= 2,
            "At least 2 pressures should be linked to factions (got {})",
            sourced,
        );
    }

    // -----------------------------------------------------------------------
    // Phase B: Faction presence tests
    // -----------------------------------------------------------------------

    #[test]
    fn faction_presence_references_valid_faction_ids() {
        let galaxy = generate_galaxy(42);
        let faction_ids: HashSet<Uuid> = galaxy.factions.iter().map(|f| f.id).collect();

        for system in &galaxy.systems {
            for fp in &system.faction_presence {
                assert!(
                    faction_ids.contains(&fp.faction_id),
                    "System {} has presence for non-existent faction {}",
                    system.name, fp.faction_id,
                );
            }
        }
    }

    #[test]
    fn faction_presence_strength_and_visibility_in_range() {
        let galaxy = generate_galaxy(42);
        for system in &galaxy.systems {
            for fp in &system.faction_presence {
                assert!(
                    fp.strength >= 0.0 && fp.strength <= 1.0,
                    "System {} faction presence strength out of range: {}",
                    system.name, fp.strength,
                );
                assert!(
                    fp.visibility >= 0.0 && fp.visibility <= 1.0,
                    "System {} faction presence visibility out of range: {}",
                    system.name, fp.visibility,
                );
            }
        }
    }

    #[test]
    fn every_system_has_faction_presence() {
        let galaxy = generate_galaxy(42);

        // Every system should have at least one faction present.
        for system in &galaxy.systems {
            assert!(
                !system.faction_presence.is_empty(),
                "System {} has no faction presence",
                system.name,
            );
        }
    }

    #[test]
    fn factions_not_all_piled_into_one_system() {
        let galaxy = generate_galaxy(42);

        // Count how many systems each faction appears in.
        let mut faction_system_count: HashMap<Uuid, usize> = HashMap::new();
        for system in &galaxy.systems {
            for fp in &system.faction_presence {
                *faction_system_count.entry(fp.faction_id).or_insert(0) += 1;
            }
        }

        // Every faction should appear in at least one system.
        for faction in &galaxy.factions {
            let count = faction_system_count.get(&faction.id).copied().unwrap_or(0);
            assert!(
                count >= 1,
                "Faction {} has no system presence at all",
                faction.name,
            );
        }

        // No faction should be in all 10 systems — that would be boring.
        for faction in &galaxy.factions {
            let count = faction_system_count.get(&faction.id).copied().unwrap_or(0);
            assert!(
                count < 10,
                "Faction {} is in all {} systems — should be more selective",
                faction.name, count,
            );
        }
    }

    #[test]
    fn no_duplicate_faction_presence_in_system() {
        let galaxy = generate_galaxy(42);
        for system in &galaxy.systems {
            let ids: Vec<Uuid> = system.faction_presence.iter().map(|fp| fp.faction_id).collect();
            let unique: HashSet<Uuid> = ids.iter().copied().collect();
            assert_eq!(
                ids.len(), unique.len(),
                "System {} has duplicate faction presence entries",
                system.name,
            );
        }
    }

    #[test]
    fn meridian_has_strong_military_presence() {
        let galaxy = generate_galaxy(42);
        let meridian = galaxy.systems.iter().find(|s| s.name == "Meridian").unwrap();
        let mil_cmd = galaxy.factions.iter().find(|f| f.name == "Hegemony Military Command").unwrap();

        let mil_presence = meridian.faction_presence.iter()
            .find(|fp| fp.faction_id == mil_cmd.id);
        assert!(mil_presence.is_some(), "Meridian should have Military Command presence");
        assert!(
            mil_presence.unwrap().strength >= 0.8,
            "Military Command at Meridian should be strong",
        );
    }

    #[test]
    fn cygnus_gate_has_strong_trade_presence() {
        let galaxy = generate_galaxy(42);
        let cygnus = galaxy.systems.iter().find(|s| s.name == "Cygnus Gate").unwrap();
        let corridor = galaxy.factions.iter().find(|f| f.name == "The Corridor Guild").unwrap();

        let trade_presence = cygnus.faction_presence.iter()
            .find(|fp| fp.faction_id == corridor.id);
        assert!(trade_presence.is_some(), "Cygnus Gate should have Corridor Guild presence");
        assert!(
            trade_presence.unwrap().strength >= 0.7,
            "Corridor Guild at Cygnus Gate should be strong",
        );
    }

    #[test]
    fn acheron_has_salvage_and_faint_intel() {
        let galaxy = generate_galaxy(42);
        let acheron = galaxy.systems.iter().find(|s| s.name == "Acheron").unwrap();
        let ashfall = galaxy.factions.iter().find(|f| f.name == "Ashfall Salvage").unwrap();
        let mil_cmd = galaxy.factions.iter().find(|f| f.name == "Hegemony Military Command").unwrap();

        let salvage = acheron.faction_presence.iter()
            .find(|fp| fp.faction_id == ashfall.id);
        assert!(salvage.is_some(), "Acheron should have Ashfall Salvage");

        let intel = acheron.faction_presence.iter()
            .find(|fp| fp.faction_id == mil_cmd.id);
        assert!(intel.is_some(), "Acheron should have faint Military Command presence");
        assert!(
            intel.unwrap().visibility < 0.2,
            "Military Command at Acheron should be low-visibility",
        );
    }

    #[test]
    fn order_drawn_to_distorted_space() {
        let galaxy = generate_galaxy(42);
        let order = galaxy.factions.iter().find(|f| f.name == "Order of the Quiet Star").unwrap();

        // The Order should be present at Drift (2.0), Kessler's Remnant (8.0),
        // and Lament (25.0) — all distorted systems.
        for name in &["Drift", "Kessler's Remnant", "Lament"] {
            let system = galaxy.systems.iter().find(|s| s.name == *name).unwrap();
            let has_order = system.faction_presence.iter()
                .any(|fp| fp.faction_id == order.id);
            assert!(
                has_order,
                "Order of the Quiet Star should be present in distorted system {}",
                name,
            );
        }

        // The Order should NOT be at Meridian (time_factor 1.0, normal space).
        let meridian = galaxy.systems.iter().find(|s| s.name == "Meridian").unwrap();
        let order_at_meridian = meridian.faction_presence.iter()
            .any(|fp| fp.faction_id == order.id);
        assert!(
            !order_at_meridian,
            "Order of the Quiet Star should NOT be at Meridian (normal space)",
        );
    }

    #[test]
    fn lattice_absent_from_deep_frontier() {
        let galaxy = generate_galaxy(42);
        let lattice = galaxy.factions.iter().find(|f| f.name == "The Lattice").unwrap();

        // The Lattice should NOT be at Kessler's Remnant or Lament —
        // information brokers need civilization to operate.
        for name in &["Kessler's Remnant", "Lament"] {
            let system = galaxy.systems.iter().find(|s| s.name == *name).unwrap();
            let has_lattice = system.faction_presence.iter()
                .any(|fp| fp.faction_id == lattice.id);
            assert!(
                !has_lattice,
                "The Lattice should NOT be present in deep frontier system {}",
                name,
            );
        }
    }

    #[test]
    fn every_faction_presence_has_services() {
        let galaxy = generate_galaxy(42);
        for system in &galaxy.systems {
            for fp in &system.faction_presence {
                assert!(
                    !fp.services.is_empty(),
                    "Faction presence in {} has no services",
                    system.name,
                );
            }
        }
    }

    #[test]
    fn faction_scope_civ_ids_reference_valid_civs() {
        let galaxy = generate_galaxy(42);
        let civ_ids: HashSet<Uuid> = galaxy.civilizations.iter().map(|c| c.id).collect();

        for faction in &galaxy.factions {
            match &faction.scope {
                FactionScope::CivInternal { civ_id } => {
                    assert!(
                        civ_ids.contains(civ_id),
                        "Faction {} CivInternal scope references non-existent civ",
                        faction.name,
                    );
                }
                FactionScope::Transnational { civ_ids: scope_ids } => {
                    for civ_id in scope_ids {
                        assert!(
                            civ_ids.contains(civ_id),
                            "Faction {} Transnational scope references non-existent civ",
                            faction.name,
                        );
                    }
                }
                FactionScope::Independent => {}
            }
        }
    }
}