// file: crates/simulation/src/generate.rs
//! Galaxy generation — deterministic from a seed.
//!
//! One sector, ten systems, two factions. Enough to validate
//! the core loop. Expansion comes later.

use rand::prelude::*;
use std::collections::HashMap;
use uuid::Uuid;

use starbound_core::galaxy::*;
use starbound_core::time::Timestamp;

/// The output of galaxy generation — everything needed to start a game.
pub struct GeneratedGalaxy {
    pub sector: Sector,
    pub systems: Vec<StarSystem>,
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

    let factions = generate_factions(&mut rng);
    let systems = generate_systems(&mut rng, &factions);
    let connections = generate_connections(&systems, &mut rng);

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
        factions,
        connections,
    }
}

fn generate_factions(rng: &mut StdRng) -> Vec<Faction> {
    let hegemony_id = Uuid::new_v4();
    let freehold_id = Uuid::new_v4();

    let hegemony = Faction {
        id: hegemony_id,
        name: "Terran Hegemony".into(),
        ethos: FactionEthos {
            expansionist: 0.7,
            isolationist: 0.1,
            militaristic: 0.6,
            diplomatic: 0.3,
            theocratic: 0.1,
            mercantile: 0.5,
            technocratic: 0.7,
            communal: 0.2,
        },
        capabilities: FactionCapabilities {
            size: 0.8,
            wealth: 0.7,
            technology: 0.8,
            military: 0.7,
        },
        relationships: {
            let mut r = HashMap::new();
            r.insert(
                freehold_id,
                FactionDisposition {
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
                "Outer colony autonomy movements gaining support".into(),
                "Military faction pushing for Freehold containment".into(),
            ],
        },
    };

    let freehold = Faction {
        id: freehold_id,
        name: "The Freehold Compact".into(),
        ethos: FactionEthos {
            expansionist: 0.3,
            isolationist: 0.5,
            militaristic: 0.3,
            diplomatic: 0.6,
            theocratic: 0.0,
            mercantile: 0.8,
            technocratic: 0.4,
            communal: 0.7,
        },
        capabilities: FactionCapabilities {
            size: 0.4,
            wealth: 0.6,
            technology: 0.5,
            military: 0.3,
        },
        relationships: {
            let mut r = HashMap::new();
            r.insert(
                hegemony_id,
                FactionDisposition {
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
                "Debate over accepting Hegemony trade terms".into(),
            ],
        },
    };

    // Shuffle order so faction generation isn't always deterministic by position.
    if rng.gen_bool(0.5) {
        vec![hegemony, freehold]
    } else {
        vec![freehold, hegemony]
    }
}

fn generate_systems(rng: &mut StdRng, factions: &[Faction]) -> Vec<StarSystem> {
    // Find faction IDs by name (order may be shuffled).
    let hegemony_id = factions.iter().find(|f| f.name == "Terran Hegemony").unwrap().id;
    let freehold_id = factions.iter().find(|f| f.name == "The Freehold Compact").unwrap().id;

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

    // Faction assignments: some Hegemony, some Freehold, some unclaimed.
    let faction_assignments: [Option<usize>; 10] = [
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

    let faction_ids = [hegemony_id, freehold_id];

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

        let controlling_faction = faction_assignments[i].map(|idx| faction_ids[idx]);

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
            controlling_faction,
            infrastructure_level: infrastructure_levels[i],
            history,
            active_threads: vec![],
            time_factor: time_factors[i],
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
    for (i, sys) in systems.iter().enumerate() {
        let mut nearest_idx = if i == 0 { 1 } else { 0 };
        let mut nearest_dist = distance(sys, &systems[nearest_idx]);

        for (j, other) in systems.iter().enumerate() {
            if i == j {
                continue;
            }
            let d = distance(sys, other);
            if d < nearest_dist {
                nearest_dist = d;
                nearest_idx = j;
            }
        }

        if !has_edge(&connected_pairs, sys.id, systems[nearest_idx].id) {
            let route_type = classify_route(nearest_dist, rng);
            connections.push(Connection {
                system_a: sys.id,
                system_b: systems[nearest_idx].id,
                distance_ly: nearest_dist,
                route_type,
            });
            connected_pairs.push((sys.id, systems[nearest_idx].id));
        }
    }

    // Step 2: Add edges for pairs within ~10 light-years.
    let threshold = 10.0;
    for i in 0..systems.len() {
        for j in (i + 1)..systems.len() {
            let d = distance(&systems[i], &systems[j]);
            if d <= threshold && !has_edge(&connected_pairs, systems[i].id, systems[j].id) {
                let route_type = classify_route(d, rng);
                connections.push(Connection {
                    system_a: systems[i].id,
                    system_b: systems[j].id,
                    distance_ly: d,
                    route_type,
                });
                connected_pairs.push((systems[i].id, systems[j].id));
            }
        }
    }

    // Step 3: One long-range corridor connecting the two faction capitals
    // (Meridian ↔ Pale Harbor) if not already connected.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_consistent_galaxy_from_seed() {
        let g1 = generate_galaxy(42);
        let g2 = generate_galaxy(42);

        assert_eq!(g1.systems.len(), 10);
        assert_eq!(g1.factions.len(), 2);
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
    fn faction_assignments_are_sensible() {
        let galaxy = generate_galaxy(42);
        let hegemony = galaxy.factions.iter().find(|f| f.name == "Terran Hegemony").unwrap();
        let freehold = galaxy.factions.iter().find(|f| f.name == "The Freehold Compact").unwrap();

        let hegemony_systems: Vec<_> = galaxy.systems.iter()
            .filter(|s| s.controlling_faction == Some(hegemony.id))
            .collect();
        let freehold_systems: Vec<_> = galaxy.systems.iter()
            .filter(|s| s.controlling_faction == Some(freehold.id))
            .collect();
        let unclaimed: Vec<_> = galaxy.systems.iter()
            .filter(|s| s.controlling_faction.is_none())
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
}
