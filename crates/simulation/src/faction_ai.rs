// file: crates/simulation/src/faction_ai.rs
//! Faction behavior trees — ethos-weighted decision-making.
//!
//! Phase 2: Making the galaxy live.
//!
//! Each galactic tick (~1 year), every faction evaluates a priority
//! queue of possible actions. Priorities are weighted by ethos
//! (what the faction *wants*) and gated by capabilities (what it
//! *can* do). The highest-priority feasible action is selected.
//!
//! Design principle: simple rules, emergent behavior. Two factions
//! with different ethos values should produce recognizably different
//! galactic histories from the same starting conditions.

use rand::rngs::StdRng;
use rand::Rng;
use uuid::Uuid;

use starbound_core::galaxy::*;

// ---------------------------------------------------------------------------
// Actions a faction can take during a tick
// ---------------------------------------------------------------------------

/// A concrete action a faction has decided to take.
#[derive(Debug, Clone)]
pub enum FactionAction {
    /// Claim an adjacent unclaimed system.
    Expand {
        faction_id: Uuid,
        target_system: Uuid,
    },
    /// Improve infrastructure in an owned system.
    Consolidate {
        faction_id: Uuid,
        target_system: Uuid,
    },
    /// Improve economic/diplomatic ties with another faction.
    Diplomacy {
        faction_id: Uuid,
        target_faction: Uuid,
    },
    /// Apply pressure (diplomatic or military) to a rival.
    Pressure {
        faction_id: Uuid,
        target_faction: Uuid,
    },
    /// Build military capability.
    Militarize {
        faction_id: Uuid,
    },
    /// Address internal instability.
    Stabilize {
        faction_id: Uuid,
    },
    /// Nothing worth doing this tick.
    Idle {
        faction_id: Uuid,
    },
}

impl FactionAction {
    pub fn faction_id(&self) -> Uuid {
        match self {
            Self::Expand { faction_id, .. }
            | Self::Consolidate { faction_id, .. }
            | Self::Diplomacy { faction_id, .. }
            | Self::Pressure { faction_id, .. }
            | Self::Militarize { faction_id }
            | Self::Stabilize { faction_id }
            | Self::Idle { faction_id } => *faction_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Goal evaluation
// ---------------------------------------------------------------------------

/// A candidate goal with its computed priority.
struct Goal {
    priority: f64,
    action: FactionAction,
}

/// Evaluate all goals for a faction and return the highest-priority action.
///
/// The priority queue is:
/// 1. **Stabilize** — if stability is dangerously low, this dominates.
/// 2. **Expand** — claim unclaimed adjacent systems.
/// 3. **Consolidate** — improve infrastructure in owned systems.
/// 4. **Diplomacy** — improve relations with other factions.
/// 5. **Pressure** — lean on rivals diplomatically or militarily.
/// 6. **Militarize** — build military strength.
///
/// Each goal's priority = base ethos weight × situational modifier.
/// A small random factor prevents perfect predictability.
pub fn evaluate_civ(
    civ: &Civilization,
    systems: &[StarSystem],
    connections: &[Connection],
    other_civs: &[&Civilization],
    rng: &mut StdRng,
) -> FactionAction {
    let mut goals: Vec<Goal> = Vec::new();

    // --- Stabilize ---
    // Urgency increases sharply as stability drops below 0.5.
    let instability = 1.0 - civ.internal_dynamics.stability as f64;
    if instability > 0.3 {
        let priority = instability * 2.0 + jitter(rng);
        goals.push(Goal {
            priority,
            action: FactionAction::Stabilize {
                faction_id: civ.id,
            },
        });
    }

    // --- Expand ---
    // Find unclaimed systems adjacent to faction territory.
    let owned_ids: Vec<Uuid> = systems
        .iter()
        .filter(|s| s.controlling_civ == Some(faction.id))
        .map(|s| s.id)
        .collect();

    let adjacent_unclaimed = find_adjacent_unclaimed(
        &owned_ids, systems, connections,
    );

    if !adjacent_unclaimed.is_empty() {
        let priority = civ.ethos.expansionist as f64 * 1.2
            + civ.capabilities.military as f64 * 0.3
            + jitter(rng);

        // Pick the most appealing target.
        let target = best_expansion_target(&adjacent_unclaimed, systems, rng);

        goals.push(Goal {
            priority,
            action: FactionAction::Expand {
                faction_id: civ.id,
                target_system: target,
            },
        });
    }

    // --- Consolidate ---
    // Find owned systems below max infrastructure.
    let upgradeable: Vec<Uuid> = systems
        .iter()
        .filter(|s| {
            s.controlling_civ == Some(faction.id)
                && can_upgrade_infrastructure(s.infrastructure_level)
        })
        .map(|s| s.id)
        .collect();

    if !upgradeable.is_empty() {
        let priority = (civ.ethos.isolationist as f64 * 0.6
            + civ.ethos.communal as f64 * 0.5
            + civ.ethos.technocratic as f64 * 0.4)
            + jitter(rng);

        // Prefer upgrading the lowest-infrastructure system.
        let target = lowest_infrastructure_system(&upgradeable, systems);

        goals.push(Goal {
            priority,
            action: FactionAction::Consolidate {
                faction_id: civ.id,
                target_system: target,
            },
        });
    }

    // --- Diplomacy ---
    for other in other_civs {
        if let Some(disposition) = civ.relationships.get(&other.id) {
            // More inclined to diplomacy when relations are not terrible.
            if disposition.diplomatic > -0.5 {
                let priority = (civ.ethos.diplomatic as f64 * 0.8
                    + civ.ethos.mercantile as f64 * 0.5)
                    * (1.0 - disposition.diplomatic as f64 * 0.5)
                    + jitter(rng);

                goals.push(Goal {
                    priority,
                    action: FactionAction::Diplomacy {
                        faction_id: civ.id,
                        target_faction: other.id,
                    },
                });
            }
        }
    }

    // --- Pressure ---
    for other in other_civs {
        if let Some(disposition) = civ.relationships.get(&other.id) {
            if disposition.diplomatic < 0.0 || disposition.military < 0.0 {
                let tension = (-disposition.diplomatic as f64).max(0.0)
                    + (-disposition.military as f64).max(0.0);

                let priority = (civ.ethos.militaristic as f64 * 0.6
                    + civ.ethos.expansionist as f64 * 0.4)
                    * tension
                    * (civ.capabilities.military as f64 * 0.5 + 0.5)
                    + jitter(rng);

                goals.push(Goal {
                    priority,
                    action: FactionAction::Pressure {
                        faction_id: civ.id,
                        target_faction: other.id,
                    },
                });
            }
        }
    }

    // --- Militarize ---
    {
        let priority = civ.ethos.militaristic as f64 * 0.7
            * (1.0 - civ.capabilities.military as f64 * 0.5)
            + jitter(rng);

        goals.push(Goal {
            priority,
            action: FactionAction::Militarize {
                faction_id: civ.id,
            },
        });
    }

    // Pick the highest-priority goal, or idle.
    goals.sort_by(|a, b| b.priority.partial_cmp(&a.priority).unwrap());

    goals
        .into_iter()
        .next()
        .map(|g| g.action)
        .unwrap_or(FactionAction::Idle {
            faction_id: civ.id,
        })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Small random factor to prevent perfectly predictable behavior.
fn jitter(rng: &mut StdRng) -> f64 {
    rng.gen_range(-0.1..0.1)
}

/// Find unclaimed systems one connection away from faction territory.
fn find_adjacent_unclaimed(
    owned_ids: &[Uuid],
    systems: &[StarSystem],
    connections: &[Connection],
) -> Vec<Uuid> {
    let mut unclaimed = Vec::new();

    for conn in connections {
        let (a, b) = (conn.system_a, conn.system_b);

        let check = |ours: Uuid, theirs: Uuid| -> bool {
            owned_ids.contains(&ours)
                && systems
                    .iter()
                    .find(|s| s.id == theirs)
                    .map(|s| s.controlling_civ.is_none())
                    .unwrap_or(false)
        };

        if check(a, b) && !unclaimed.contains(&b) {
            unclaimed.push(b);
        }
        if check(b, a) && !unclaimed.contains(&a) {
            unclaimed.push(a);
        }
    }

    unclaimed
}

/// Pick the best expansion target — prefer some infrastructure over none.
fn best_expansion_target(
    candidates: &[Uuid],
    systems: &[StarSystem],
    rng: &mut StdRng,
) -> Uuid {
    if candidates.len() == 1 {
        return candidates[0];
    }

    let mut scored: Vec<(Uuid, f64)> = candidates
        .iter()
        .map(|&id| {
            let infra_score = systems
                .iter()
                .find(|s| s.id == id)
                .map(|s| infrastructure_value(s.infrastructure_level))
                .unwrap_or(0.0);
            (id, infra_score + rng.gen_range(0.0..0.5))
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    scored[0].0
}

/// Find the owned system with the lowest infrastructure level.
fn lowest_infrastructure_system(
    owned_ids: &[Uuid],
    systems: &[StarSystem],
) -> Uuid {
    owned_ids
        .iter()
        .min_by_key(|&&id| {
            systems
                .iter()
                .find(|s| s.id == id)
                .map(|s| infrastructure_rank(s.infrastructure_level))
                .unwrap_or(0)
        })
        .copied()
        .expect("owned_ids should not be empty")
}

fn infrastructure_value(level: InfrastructureLevel) -> f64 {
    match level {
        InfrastructureLevel::None => 0.0,
        InfrastructureLevel::Outpost => 0.2,
        InfrastructureLevel::Colony => 0.4,
        InfrastructureLevel::Established => 0.6,
        InfrastructureLevel::Hub => 0.8,
        InfrastructureLevel::Capital => 1.0,
    }
}

fn infrastructure_rank(level: InfrastructureLevel) -> u8 {
    match level {
        InfrastructureLevel::None => 0,
        InfrastructureLevel::Outpost => 1,
        InfrastructureLevel::Colony => 2,
        InfrastructureLevel::Established => 3,
        InfrastructureLevel::Hub => 4,
        InfrastructureLevel::Capital => 5,
    }
}

fn can_upgrade_infrastructure(level: InfrastructureLevel) -> bool {
    !matches!(level, InfrastructureLevel::Capital | InfrastructureLevel::Hub)
}

/// Upgrade infrastructure one step.
pub fn next_infrastructure_level(level: InfrastructureLevel) -> InfrastructureLevel {
    match level {
        InfrastructureLevel::None => InfrastructureLevel::Outpost,
        InfrastructureLevel::Outpost => InfrastructureLevel::Colony,
        InfrastructureLevel::Colony => InfrastructureLevel::Established,
        InfrastructureLevel::Established => InfrastructureLevel::Hub,
        InfrastructureLevel::Hub => InfrastructureLevel::Hub,
        InfrastructureLevel::Capital => InfrastructureLevel::Capital,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use std::collections::HashMap;

    fn test_civ(name: &str, id: Uuid, ethos: CivEthos) -> Civilization {
        Civilization {
            id,
            name: name.into(),
            ethos,
            capabilities: CivCapabilities {
                size: 0.5,
                wealth: 0.5,
                technology: 0.5,
                military: 0.5,
            },
            relationships: HashMap::new(),
            internal_dynamics: InternalDynamics {
                stability: 0.7,
                pressures: vec![],
            },
            faction_ids: vec![],
        }
    }

    fn expansionist_ethos() -> CivEthos {
        CivEthos {
            expansionist: 0.9,
            isolationist: 0.1,
            militaristic: 0.5,
            diplomatic: 0.2,
            theocratic: 0.0,
            mercantile: 0.3,
            technocratic: 0.3,
            communal: 0.2,
        }
    }

    fn diplomatic_ethos() -> CivEthos {
        CivEthos {
            expansionist: 0.2,
            isolationist: 0.3,
            militaristic: 0.1,
            diplomatic: 0.9,
            theocratic: 0.0,
            mercantile: 0.8,
            technocratic: 0.4,
            communal: 0.6,
        }
    }

    fn test_system(name: &str, faction: Option<Uuid>) -> StarSystem {
        StarSystem {
            id: Uuid::new_v4(),
            name: name.into(),
            position: (0.0, 0.0),
            star_type: StarType::YellowDwarf,
            planetary_bodies: vec![],
            controlling_civ: faction,
            infrastructure_level: InfrastructureLevel::Colony,
            history: vec![],
            active_threads: vec![],
            time_factor: 1.0,
            faction_presence: vec![],
        }
    }

    #[test]
    fn expansionist_faction_prefers_expansion() {
        let faction_id = Uuid::new_v4();
        let civ = test_civ("Expanders", faction_id, expansionist_ethos());

        let owned = test_system("Home", Some(faction_id));
        let unclaimed = test_system("Target", None);

        let conn = Connection {
            system_a: owned.id,
            system_b: unclaimed.id,
            distance_ly: 5.0,
            route_type: RouteType::Open,
        };

        let systems = vec![owned, unclaimed];
        let connections = vec![conn];

        let mut expand_count = 0;
        for seed in 0..20u64 {
            let mut r = StdRng::seed_from_u64(seed);
            let action = evaluate_civ(
                &civ, &systems, &connections, &[], &mut r,
            );
            if matches!(action, FactionAction::Expand { .. }) {
                expand_count += 1;
            }
        }

        assert!(
            expand_count > 10,
            "Expansionist faction should usually expand (got {}/20)",
            expand_count
        );
    }

    #[test]
    fn unstable_faction_prioritizes_stabilization() {
        let faction_id = Uuid::new_v4();
        let mut civ = test_civ("Shaky", faction_id, expansionist_ethos());
        civ.internal_dynamics.stability = 0.2;

        let owned = test_system("Home", Some(faction_id));
        let unclaimed = test_system("Target", None);

        let conn = Connection {
            system_a: owned.id,
            system_b: unclaimed.id,
            distance_ly: 5.0,
            route_type: RouteType::Open,
        };

        let systems = vec![owned, unclaimed];
        let connections = vec![conn];

        let mut stabilize_count = 0;
        for seed in 0..20u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let action = evaluate_civ(
                &civ, &systems, &connections, &[], &mut rng,
            );
            if matches!(action, FactionAction::Stabilize { .. }) {
                stabilize_count += 1;
            }
        }

        assert!(
            stabilize_count > 10,
            "Unstable faction should usually stabilize (got {}/20)",
            stabilize_count
        );
    }

    #[test]
    fn diplomatic_faction_with_neighbor_prefers_diplomacy() {
        let faction_id = Uuid::new_v4();
        let other_id = Uuid::new_v4();

        let mut civ = test_civ("Diplomats", faction_id, diplomatic_ethos());
        civ.relationships.insert(other_id, CivDisposition {
            diplomatic: 0.0,
            economic: 0.2,
            military: 0.0,
        });

        let owned = test_system("Home", Some(faction_id));
        let other_system = test_system("TheirHome", Some(other_id));
        let systems = vec![owned, other_system];
        let connections = vec![];

        let other_civ = test_civ("Neighbors", other_id, expansionist_ethos());

        let mut diplomacy_count = 0;
        for seed in 0..20u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let action = evaluate_civ(
                &civ, &systems, &connections, &[&other_civ], &mut rng,
            );
            if matches!(action, FactionAction::Diplomacy { .. }) {
                diplomacy_count += 1;
            }
        }

        assert!(
            diplomacy_count > 5,
            "Diplomatic faction should often choose diplomacy (got {}/20)",
            diplomacy_count
        );
    }

    #[test]
    fn find_adjacent_unclaimed_works() {
        let faction_id = Uuid::new_v4();
        let owned_id = Uuid::new_v4();
        let unclaimed_id = Uuid::new_v4();
        let far_id = Uuid::new_v4();

        let owned = StarSystem {
            id: owned_id,
            controlling_civ: Some(faction_id),
            ..test_system("Owned", Some(faction_id))
        };
        let unclaimed = StarSystem {
            id: unclaimed_id,
            controlling_civ: None,
            ..test_system("Unclaimed", None)
        };
        let far = StarSystem {
            id: far_id,
            controlling_civ: None,
            ..test_system("Far", None)
        };

        let systems = vec![owned, unclaimed, far];
        let connections = vec![Connection {
            system_a: owned_id,
            system_b: unclaimed_id,
            distance_ly: 5.0,
            route_type: RouteType::Open,
        }];

        let result = find_adjacent_unclaimed(&[owned_id], &systems, &connections);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], unclaimed_id);
    }
}
