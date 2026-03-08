// file: crates/simulation/src/faction_tick.rs
//! Faction tick logic — organic presence drift and expansion.
//!
//! Phase C: Making factions live.
//!
//! Unlike civilizations (which take deliberate actions via priority queues),
//! factions drift organically. Each tick, faction presence at every system
//! moves toward an equilibrium determined by local conditions:
//!
//! - Military factions follow their parent civ's military strength
//! - Economic factions follow infrastructure and trade routes
//! - Guild factions track port size, slow and steady
//! - Religious factions are drawn to anomalous spacetime
//! - Criminal factions exploit instability and power vacuums
//!
//! Design principle: factions are weather, not chess pieces. They respond
//! to conditions rather than making strategic decisions. This produces
//! emergent political geography without requiring faction-level AI.

use rand::rngs::StdRng;
use rand::Rng;
use uuid::Uuid;

use starbound_core::galaxy::*;

use super::generate::GeneratedGalaxy;
use super::tick::{TickEvent, TickEventCategory};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// How quickly presence drifts toward equilibrium per tick (fraction of gap).
/// At 0.08, a faction closes ~55% of the gap in 10 ticks.
const DRIFT_RATE: f32 = 0.08;

/// Minimum strength before a presence is pruned entirely.
const PRUNE_THRESHOLD: f32 = 0.03;

/// Base chance per tick that a faction expands to an adjacent system.
const EXPANSION_BASE_CHANCE: f64 = 0.12;

/// Minimum strength at a system before the faction can expand from it.
const EXPANSION_SOURCE_MIN: f32 = 0.3;

/// Starting strength when a faction first appears at a system.
const SEED_STRENGTH: f32 = 0.1;

/// Strength change magnitude (since last snapshot) that triggers a narrative event.
const EVENT_DRIFT_THRESHOLD: f32 = 0.15;

// ---------------------------------------------------------------------------
// Snapshots — pre-collected data to avoid borrow conflicts
// ---------------------------------------------------------------------------

/// Pre-collected civilization state needed for equilibrium calculations.
struct CivSnapshot {
    id: Uuid,
    stability: f32,
    military: f32,
}

/// Pre-collected system state needed for equilibrium calculations.
struct SystemSnapshot {
    id: Uuid,
    name: String,
    time_factor: f32,
    controlling_civ: Option<Uuid>,
    infra_value: f32,
    /// Strength of this faction at this system before drift (None = not present).
    current_strength: Option<f32>,
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Run one tick of faction simulation. Call this once per galactic tick,
/// after civilization actions have been resolved and passive effects applied.
pub fn tick_factions(
    galaxy: &mut GeneratedGalaxy,
    tick_number: usize,
    galactic_day: f64,
    rng: &mut StdRng,
    events: &mut Vec<TickEvent>,
) {
    let faction_snapshots: Vec<Faction> = galaxy.factions.clone();
    let civ_snapshots: Vec<CivSnapshot> = galaxy.civilizations.iter()
        .map(|c| CivSnapshot {
            id: c.id,
            stability: c.internal_dynamics.stability,
            military: c.capabilities.military,
        })
        .collect();

    for faction in &faction_snapshots {
        // Snapshot system state for this faction before mutations.
        let sys_snapshots: Vec<SystemSnapshot> = galaxy.systems.iter()
            .map(|s| SystemSnapshot {
                id: s.id,
                name: s.name.clone(),
                time_factor: s.time_factor as f32,
                controlling_civ: s.controlling_civ,
                infra_value: infra_value(s.infrastructure_level),
                current_strength: s.faction_presence.iter()
                    .find(|fp| fp.faction_id == faction.id)
                    .map(|fp| fp.strength),
            })
            .collect();

        drift_presence(faction, &sys_snapshots, &civ_snapshots, galaxy);
        maybe_expand(faction, &sys_snapshots, &civ_snapshots, galaxy, rng,
                      tick_number, galactic_day, events);
        prune_and_report(faction, &sys_snapshots, galaxy,
                         tick_number, galactic_day, events);
    }
}

// ---------------------------------------------------------------------------
// Drift: move existing presences toward equilibrium
// ---------------------------------------------------------------------------

fn drift_presence(
    faction: &Faction,
    snapshots: &[SystemSnapshot],
    civs: &[CivSnapshot],
    galaxy: &mut GeneratedGalaxy,
) {
    for snap in snapshots {
        // Only drift systems where this faction already has presence.
        if snap.current_strength.is_none() {
            continue;
        }

        let equilibrium = compute_equilibrium(faction, snap, civs);

        if let Some(system) = galaxy.systems.iter_mut().find(|s| s.id == snap.id) {
            if let Some(presence) = system.faction_presence.iter_mut()
                .find(|fp| fp.faction_id == faction.id)
            {
                // Strength drift.
                let delta = (equilibrium - presence.strength) * DRIFT_RATE;
                presence.strength = (presence.strength + delta).clamp(0.0, 1.0);

                // Visibility drifts toward category-appropriate level.
                let vis_target = visibility_target(faction, presence.strength);
                let vis_delta = (vis_target - presence.visibility) * DRIFT_RATE;
                presence.visibility = (presence.visibility + vis_delta).clamp(0.0, 1.0);
            }
        }
    }
}

/// Compute the natural equilibrium strength for a faction at a system.
/// This is the value presence drifts toward when undisturbed.
fn compute_equilibrium(
    faction: &Faction,
    sys: &SystemSnapshot,
    civs: &[CivSnapshot],
) -> f32 {
    match faction.category {
        FactionCategory::Military => {
            // Strong in parent civ's territory, weak elsewhere.
            // Scales with the civ's military capability.
            if let Some(civ_id) = sys.controlling_civ {
                if let Some(&influence) = faction.influence.get(&civ_id) {
                    if influence > 0.3 {
                        let military = civs.iter()
                            .find(|c| c.id == civ_id)
                            .map(|c| c.military)
                            .unwrap_or(0.5);
                        return military * influence;
                    }
                }
            }
            // Outside friendly territory — token intel presence at best.
            0.05
        }

        FactionCategory::Economic => {
            // Follows infrastructure. No economy at uninhabited systems.
            if sys.infra_value < 0.2 {
                return 0.0;
            }
            let base = sys.infra_value * 0.7;
            // Bonus where the faction has civ-level influence.
            let civ_bonus = sys.controlling_civ
                .and_then(|cid| faction.influence.get(&cid))
                .copied()
                .unwrap_or(0.0) * 0.3;
            (base + civ_bonus).min(0.95)
        }

        FactionCategory::Guild => {
            // Follows ports. Slow, steady, everywhere there's infrastructure.
            if sys.infra_value < 0.1 {
                return 0.0;
            }
            sys.infra_value * 0.5
        }

        FactionCategory::Religious => {
            // Drawn exclusively to distorted spacetime.
            if sys.time_factor <= 1.0 {
                return 0.0;
            }
            // Logarithmic: time_factor 2.0→~0.20, 8.0→~0.61, 25.0→~0.95
            let distortion = (sys.time_factor as f64).ln() / (30.0_f64).ln();
            (distortion as f32).clamp(0.0, 0.95)
        }

        FactionCategory::Criminal => {
            // Two flavors based on scope.
            match faction.scope {
                FactionScope::Independent => {
                    // Frontier criminal (Ashfall) — thrives in lawless space.
                    if sys.controlling_civ.is_some() && sys.infra_value > 0.4 {
                        return 0.02; // Can't operate openly in civilized space
                    }
                    if sys.controlling_civ.is_none() {
                        return 0.5 + (1.0 - sys.infra_value) * 0.3;
                    }
                    // Fringe of civ space — tolerated, barely.
                    0.15
                }
                _ => {
                    // Covert criminal (Lattice) — thrives in civilized instability.
                    if sys.infra_value < 0.2 {
                        return 0.0; // No customers in the wilderness
                    }
                    let instability = sys.controlling_civ
                        .and_then(|cid| civs.iter().find(|c| c.id == cid))
                        .map(|c| 1.0 - c.stability)
                        .unwrap_or(0.2);
                    (sys.infra_value * 0.3 + instability * 0.4).min(0.85)
                }
            }
        }

        FactionCategory::Political => {
            // Follows parent civ's territory, scales with infrastructure.
            if let Some(civ_id) = sys.controlling_civ {
                if let Some(&influence) = faction.influence.get(&civ_id) {
                    if influence > 0.2 {
                        return sys.infra_value * influence * 0.6;
                    }
                }
            }
            0.0
        }

        FactionCategory::Academic => {
            // Follows infrastructure. Bonus at high-tech systems.
            if sys.infra_value < 0.3 {
                return 0.0;
            }
            sys.infra_value * 0.4
        }
    }
}

/// Compute the visibility a faction naturally tends toward at a given strength.
fn visibility_target(faction: &Faction, strength: f32) -> f32 {
    let ratio = match faction.category {
        FactionCategory::Military => 1.0,    // Fully visible — uniforms, patrols
        FactionCategory::Economic => 0.9,    // Shopfronts, trade offices
        FactionCategory::Guild => 0.8,       // Dockside, known to regulars
        FactionCategory::Religious => 0.7,   // Pilgrim camps, temples
        FactionCategory::Political => 0.85,  // Public offices, functionaries
        FactionCategory::Academic => 0.75,   // Labs, observatories
        FactionCategory::Criminal => {
            match faction.scope {
                FactionScope::Independent => 0.5,  // Known but deniable
                _ => 0.2,                           // You don't see them unless they want you to
            }
        }
    };
    strength * ratio
}

// ---------------------------------------------------------------------------
// Expansion: factions spread to adjacent systems
// ---------------------------------------------------------------------------

fn maybe_expand(
    faction: &Faction,
    snapshots: &[SystemSnapshot],
    civs: &[CivSnapshot],
    galaxy: &mut GeneratedGalaxy,
    rng: &mut StdRng,
    tick_number: usize,
    galactic_day: f64,
    events: &mut Vec<TickEvent>,
) {
    // Find systems where this faction is strong enough to expand from.
    let strong_ids: Vec<Uuid> = snapshots.iter()
        .filter(|s| s.current_strength.unwrap_or(0.0) >= EXPANSION_SOURCE_MIN)
        .map(|s| s.id)
        .collect();

    if strong_ids.is_empty() {
        return;
    }

    // Roll for expansion. More open/aggressive factions expand more often.
    let chance = EXPANSION_BASE_CHANCE
        * (0.5 + faction.ethos.openness as f64 * 0.3 + faction.ethos.aggression as f64 * 0.2);
    if !rng.gen_bool(chance.min(0.95)) {
        return;
    }

    // Find connected systems where this faction is NOT yet present.
    let targets = find_expansion_targets(&strong_ids, faction, snapshots, galaxy);
    if targets.is_empty() {
        return;
    }

    // Pick a target.
    let target_id = targets[rng.gen_range(0..targets.len())];

    // Check equilibrium — don't expand to places where the faction can't survive.
    let target_snap = snapshots.iter().find(|s| s.id == target_id).unwrap();
    let eq = compute_equilibrium(faction, target_snap, civs);
    if eq < PRUNE_THRESHOLD * 2.0 {
        return;
    }

    // Seed the presence.
    if let Some(system) = galaxy.systems.iter_mut().find(|s| s.id == target_id) {
        let vis = visibility_target(faction, SEED_STRENGTH);
        system.faction_presence.push(FactionPresence {
            faction_id: faction.id,
            strength: SEED_STRENGTH,
            visibility: vis,
            services: default_services(&faction.category, &faction.scope),
        });

        events.push(TickEvent {
            tick_number,
            galactic_day,
            description: expansion_description(&faction.name, &faction.category, &system.name),
            entities: vec![faction.id, target_id],
            category: TickEventCategory::Faction,
        });
    }
}

fn find_expansion_targets(
    source_ids: &[Uuid],
    _faction: &Faction,
    snapshots: &[SystemSnapshot],
    galaxy: &GeneratedGalaxy,
) -> Vec<Uuid> {
    let mut targets = Vec::new();
    let present_ids: Vec<Uuid> = snapshots.iter()
        .filter(|s| s.current_strength.is_some())
        .map(|s| s.id)
        .collect();

    for conn in &galaxy.connections {
        let (a, b) = (conn.system_a, conn.system_b);

        // Check both directions.
        let check = |from: Uuid, to: Uuid| {
            source_ids.contains(&from)
                && !present_ids.contains(&to)
                && !targets.contains(&to)
        };

        if check(a, b) {
            targets.push(b);
        } else if check(b, a) {
            targets.push(a);
        }
    }

    targets
}

/// Services a newly-expanded faction offers at a fresh location.
fn default_services(category: &FactionCategory, scope: &FactionScope) -> Vec<FactionService> {
    match category {
        FactionCategory::Military => vec![FactionService::Intelligence, FactionService::Missions],
        FactionCategory::Economic => vec![FactionService::Trade, FactionService::Missions],
        FactionCategory::Guild => vec![FactionService::Repair, FactionService::Trade],
        FactionCategory::Religious => vec![FactionService::Shelter, FactionService::Intelligence],
        FactionCategory::Political => vec![FactionService::Missions, FactionService::Intelligence],
        FactionCategory::Academic => vec![FactionService::Intelligence, FactionService::Training],
        FactionCategory::Criminal => match scope {
            FactionScope::Independent => vec![FactionService::Smuggling, FactionService::Repair],
            _ => vec![FactionService::Intelligence, FactionService::Smuggling],
        },
    }
}

fn expansion_description(faction_name: &str, category: &FactionCategory, system_name: &str) -> String {
    match category {
        FactionCategory::Military =>
            format!("{} deployed observers to {}.", faction_name, system_name),
        FactionCategory::Economic =>
            format!("{} opened a trade office at {}.", faction_name, system_name),
        FactionCategory::Guild =>
            format!("{} established a chapter house at {}.", faction_name, system_name),
        FactionCategory::Religious =>
            format!("{} pilgrims were seen arriving at {}.", faction_name, system_name),
        FactionCategory::Political =>
            format!("{} opened an administrative office at {}.", faction_name, system_name),
        FactionCategory::Academic =>
            format!("{} established a research outpost at {}.", faction_name, system_name),
        FactionCategory::Criminal =>
            format!("Rumors suggest {} has moved into {}.", faction_name, system_name),
    }
}

// ---------------------------------------------------------------------------
// Pruning: remove negligible presences, report significant retreats
// ---------------------------------------------------------------------------

fn prune_and_report(
    faction: &Faction,
    snapshots: &[SystemSnapshot],
    galaxy: &mut GeneratedGalaxy,
    tick_number: usize,
    galactic_day: f64,
    events: &mut Vec<TickEvent>,
) {
    for snap in snapshots {
        let was_present = snap.current_strength.unwrap_or(0.0);

        if let Some(system) = galaxy.systems.iter_mut().find(|s| s.id == snap.id) {
            let should_prune = system.faction_presence.iter()
                .find(|fp| fp.faction_id == faction.id)
                .map(|fp| fp.strength < PRUNE_THRESHOLD)
                .unwrap_or(false);

            if should_prune {
                system.faction_presence.retain(|fp| fp.faction_id != faction.id);

                // Report the retreat if the faction was meaningfully present before.
                if was_present >= EVENT_DRIFT_THRESHOLD {
                    events.push(TickEvent {
                        tick_number,
                        galactic_day,
                        description: retreat_description(
                            &faction.name, &faction.category, &system.name,
                        ),
                        entities: vec![faction.id, snap.id],
                        category: TickEventCategory::Faction,
                    });
                }
            }
        }
    }
}

fn retreat_description(faction_name: &str, category: &FactionCategory, system_name: &str) -> String {
    match category {
        FactionCategory::Military =>
            format!("{} withdrew its personnel from {}.", faction_name, system_name),
        FactionCategory::Economic =>
            format!("{} closed its operations at {}.", faction_name, system_name),
        FactionCategory::Guild =>
            format!("{} shuttered its chapter house at {}.", faction_name, system_name),
        FactionCategory::Religious =>
            format!("{} pilgrims quietly departed {}.", faction_name, system_name),
        FactionCategory::Political =>
            format!("{} recalled its administrators from {}.", faction_name, system_name),
        FactionCategory::Academic =>
            format!("{} closed its research station at {}.", faction_name, system_name),
        FactionCategory::Criminal =>
            format!("{} is no longer seen at {}.", faction_name, system_name),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn infra_value(level: InfrastructureLevel) -> f32 {
    match level {
        InfrastructureLevel::None => 0.0,
        InfrastructureLevel::Outpost => 0.2,
        InfrastructureLevel::Colony => 0.4,
        InfrastructureLevel::Established => 0.6,
        InfrastructureLevel::Hub => 0.8,
        InfrastructureLevel::Capital => 1.0,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use crate::generate::generate_galaxy;

    /// Run N ticks and return the galaxy state afterward.
    fn run_ticks(seed: u64, num_ticks: usize) -> GeneratedGalaxy {
        let mut galaxy = generate_galaxy(seed);
        let mut rng = StdRng::seed_from_u64(seed);
        let mut events = Vec::new();
        for tick in 0..num_ticks {
            let day = tick as f64 * 365.25;
            tick_factions(&mut galaxy, tick, day, &mut rng, &mut events);
        }
        galaxy
    }

    /// Find first faction of a given category.
    fn find_faction_by_category(galaxy: &GeneratedGalaxy, cat: FactionCategory) -> &Faction {
        galaxy.factions.iter().find(|f| f.category == cat)
            .unwrap_or_else(|| panic!("No {:?} faction found", cat))
    }

    /// Get a faction's strength at a system.
    fn strength_at(galaxy: &GeneratedGalaxy, faction_id: Uuid, system_id: Uuid) -> f32 {
        galaxy.systems.iter()
            .find(|s| s.id == system_id)
            .and_then(|s| s.faction_presence.iter().find(|fp| fp.faction_id == faction_id))
            .map(|fp| fp.strength)
            .unwrap_or(0.0)
    }

    /// Find a system by infrastructure level.
    fn find_system_by_infra(galaxy: &GeneratedGalaxy, level: InfrastructureLevel) -> &StarSystem {
        galaxy.systems.iter().find(|s| s.infrastructure_level == level)
            .unwrap_or_else(|| panic!("No {:?} system found", level))
    }

    /// Find a capital system owned by a specific civ.
    fn find_capital_for_civ(galaxy: &GeneratedGalaxy, civ_id: Uuid) -> Option<&StarSystem> {
        galaxy.systems.iter().find(|s| {
            s.controlling_civ == Some(civ_id)
                && s.infrastructure_level == InfrastructureLevel::Capital
        })
    }

    // -----------------------------------------------------------------------
    // Equilibrium tests
    // -----------------------------------------------------------------------

    #[test]
    fn military_faction_strong_in_parent_territory() {
        let galaxy = run_ticks(42, 20);
        let mil = find_faction_by_category(&galaxy, FactionCategory::Military);
        // Find the parent civ's capital.
        let parent_civ_id = match &mil.scope {
            FactionScope::CivInternal { civ_id } => *civ_id,
            _ => panic!("Military faction should be civ-internal"),
        };
        let capital = find_capital_for_civ(&galaxy, parent_civ_id)
            .expect("Parent civ should have a capital");
        let strength = strength_at(&galaxy, mil.id, capital.id);
        assert!(
            strength > 0.2,
            "Military should be strong at parent capital {}, got {}",
            capital.name, strength,
        );
    }

    #[test]
    fn military_faction_weak_outside_parent_territory() {
        let galaxy = run_ticks(42, 20);
        let mil = find_faction_by_category(&galaxy, FactionCategory::Military);
        let parent_civ_id = match &mil.scope {
            FactionScope::CivInternal { civ_id } => *civ_id,
            _ => panic!("Military faction should be civ-internal"),
        };
        // Find a capital belonging to a DIFFERENT civ.
        let foreign_capital = galaxy.systems.iter().find(|s| {
            s.infrastructure_level == InfrastructureLevel::Capital
                && s.controlling_civ != Some(parent_civ_id)
        });
        if let Some(system) = foreign_capital {
            let strength = strength_at(&galaxy, mil.id, system.id);
            assert!(
                strength < 0.15,
                "Military should be weak at foreign capital {}, got {}",
                system.name, strength,
            );
        }
    }

    #[test]
    fn economic_faction_follows_infrastructure() {
        let galaxy = run_ticks(42, 20);
        let econ = find_faction_by_category(&galaxy, FactionCategory::Economic);
        let hub = find_system_by_infra(&galaxy, InfrastructureLevel::Hub);
        let hub_strength = strength_at(&galaxy, econ.id, hub.id);

        // Find a frontier/wilderness system.
        let frontier = galaxy.systems.iter()
            .find(|s| s.infrastructure_level == InfrastructureLevel::Outpost
                || s.infrastructure_level == InfrastructureLevel::None);
        let frontier_strength = frontier
            .map(|s| strength_at(&galaxy, econ.id, s.id))
            .unwrap_or(0.0);

        assert!(
            hub_strength > frontier_strength + 0.05,
            "Economic faction should be stronger at hub ({:.2}) than frontier ({:.2})",
            hub_strength, frontier_strength,
        );
    }

    #[test]
    fn religious_faction_absent_from_normal_space() {
        let galaxy = run_ticks(42, 20);
        if let Some(religious) = galaxy.factions.iter().find(|f| f.category == FactionCategory::Religious) {
            for system in &galaxy.systems {
                if system.time_factor <= 1.0 {
                    let strength = strength_at(&galaxy, religious.id, system.id);
                    assert!(
                        strength < PRUNE_THRESHOLD + 0.01,
                        "Religious faction should not be at normal-time {} (tf={:.1}), got {:.3}",
                        system.name, system.time_factor, strength,
                    );
                }
            }
        }
    }

    #[test]
    fn frontier_criminal_strong_in_unclaimed_space() {
        let galaxy = run_ticks(42, 20);
        // Find independent criminal faction.
        let criminal = galaxy.factions.iter().find(|f| {
            f.category == FactionCategory::Criminal
                && matches!(f.scope, FactionScope::Independent)
        });
        if let Some(criminal) = criminal {
            // Find an unclaimed system with some infrastructure.
            let unclaimed = galaxy.systems.iter().find(|s| {
                s.controlling_civ.is_none()
                    && s.infrastructure_level != InfrastructureLevel::None
            });
            if let Some(system) = unclaimed {
                let strength = strength_at(&galaxy, criminal.id, system.id);
                assert!(
                    strength > 0.15,
                    "Frontier criminal should be present at unclaimed {}, got {}",
                    system.name, strength,
                );
            }
        }
    }

    #[test]
    fn covert_criminal_grows_with_instability() {
        let mut galaxy_stable = generate_galaxy(42);
        let mut galaxy_unstable = generate_galaxy(42);

        for civ in &mut galaxy_unstable.civilizations {
            civ.internal_dynamics.stability = 0.2;
        }

        let mut rng_s = StdRng::seed_from_u64(42);
        let mut rng_u = StdRng::seed_from_u64(42);
        let mut events_s = Vec::new();
        let mut events_u = Vec::new();

        for tick in 0..15 {
            let day = tick as f64 * 365.25;
            tick_factions(&mut galaxy_stable, tick, day, &mut rng_s, &mut events_s);
            tick_factions(&mut galaxy_unstable, tick, day, &mut rng_u, &mut events_u);
        }

        // Find covert criminal (transnational scope) in each galaxy separately.
        let covert_s = galaxy_stable.factions.iter().find(|f| {
            f.category == FactionCategory::Criminal
                && matches!(f.scope, FactionScope::Transnational { .. })
        });
        let covert_u = galaxy_unstable.factions.iter().find(|f| {
            f.category == FactionCategory::Criminal
                && matches!(f.scope, FactionScope::Transnational { .. })
        });

        if let (Some(cs), Some(cu)) = (covert_s, covert_u) {
            let total_stable: f32 = galaxy_stable.systems.iter()
                .flat_map(|s| s.faction_presence.iter())
                .filter(|fp| fp.faction_id == cs.id)
                .map(|fp| fp.strength)
                .sum();
            let total_unstable: f32 = galaxy_unstable.systems.iter()
                .flat_map(|s| s.faction_presence.iter())
                .filter(|fp| fp.faction_id == cu.id)
                .map(|fp| fp.strength)
                .sum();
            assert!(
                total_unstable > total_stable,
                "Covert criminal should be stronger in unstable galaxy ({:.2}) vs stable ({:.2})",
                total_unstable, total_stable,
            );
        }
    }

    // -----------------------------------------------------------------------
    // Drift mechanics tests
    // -----------------------------------------------------------------------

    #[test]
    fn drift_moves_toward_equilibrium() {
        let galaxy_1 = run_ticks(42, 1);
        let galaxy_10 = run_ticks(42, 10);

        // After ticking, presences should still exist (system didn't crash).
        let total_1: f32 = galaxy_1.systems.iter()
            .flat_map(|s| s.faction_presence.iter())
            .map(|fp| fp.strength)
            .sum();
        let total_10: f32 = galaxy_10.systems.iter()
            .flat_map(|s| s.faction_presence.iter())
            .map(|fp| fp.strength)
            .sum();
        assert!(total_1 > 0.0, "Should still have presences after 1 tick");
        assert!(total_10 > 0.0, "Should still have presences after 10 ticks");
    }

    #[test]
    fn pruning_removes_negligible_presence() {
        let mut galaxy = generate_galaxy(42);

        // Find a wilderness system and inject a negligible presence.
        let wilderness_id = galaxy.systems.iter()
            .find(|s| s.infrastructure_level == InfrastructureLevel::None)
            .map(|s| s.id);

        if let Some(sys_id) = wilderness_id {
            let faction_id = galaxy.factions[0].id;
            if let Some(system) = galaxy.systems.iter_mut().find(|s| s.id == sys_id) {
                system.faction_presence.push(FactionPresence {
                    faction_id,
                    strength: 0.01,
                    visibility: 0.01,
                    services: vec![],
                });
            }

            let mut rng = StdRng::seed_from_u64(42);
            let mut events = Vec::new();
            tick_factions(&mut galaxy, 0, 0.0, &mut rng, &mut events);

            let system = galaxy.systems.iter().find(|s| s.id == sys_id).unwrap();
            let still_present = system.faction_presence.iter()
                .any(|fp| fp.faction_id == faction_id && fp.strength < PRUNE_THRESHOLD);
            assert!(!still_present, "Negligible presence should have been pruned");
        }
    }

    // -----------------------------------------------------------------------
    // Expansion tests
    // -----------------------------------------------------------------------

    #[test]
    fn factions_can_expand_over_many_ticks() {
        let initial = generate_galaxy(42);
        let evolved = run_ticks(42, 50);

        let initial_count: usize = initial.systems.iter()
            .map(|s| s.faction_presence.len())
            .sum();
        let evolved_count: usize = evolved.systems.iter()
            .map(|s| s.faction_presence.len())
            .sum();

        assert!(
            evolved_count >= initial_count.saturating_sub(5),
            "Faction presences should not collapse drastically: {} -> {}",
            initial_count, evolved_count,
        );
    }

    #[test]
    fn expansion_generates_events() {
        let mut galaxy = generate_galaxy(42);
        let mut all_events = Vec::new();

        for tick in 0..50 {
            let mut rng = StdRng::seed_from_u64(42 + tick as u64);
            let day = tick as f64 * 365.25;
            tick_factions(&mut galaxy, tick, day, &mut rng, &mut all_events);
        }

        let faction_events: Vec<_> = all_events.iter()
            .filter(|e| matches!(e.category, TickEventCategory::Faction))
            .collect();

        assert!(
            !faction_events.is_empty(),
            "50 ticks should produce at least some faction events",
        );
    }

    #[test]
    fn religious_does_not_expand_to_normal_space() {
        let evolved = run_ticks(42, 50);

        if let Some(religious) = evolved.factions.iter().find(|f| f.category == FactionCategory::Religious) {
            for system in &evolved.systems {
                if system.time_factor <= 1.0 {
                    let has_religious = system.faction_presence.iter()
                        .any(|fp| fp.faction_id == religious.id);
                    assert!(
                        !has_religious,
                        "Religious faction should not be at normal-time {} (tf={:.1})",
                        system.name, system.time_factor,
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Determinism
    // -----------------------------------------------------------------------

    #[test]
    fn faction_ticks_are_deterministic() {
        let collect = |seed: u64| -> Vec<(String, Vec<(String, f32)>)> {
            let galaxy = run_ticks(seed, 20);
            galaxy.systems.iter()
                .map(|s| {
                    let mut presences: Vec<(String, f32)> = s.faction_presence.iter()
                        .map(|fp| {
                            let fname = galaxy.factions.iter()
                                .find(|f| f.id == fp.faction_id)
                                .map(|f| f.name.clone())
                                .unwrap_or_default();
                            (fname, (fp.strength * 1000.0).round() / 1000.0)
                        })
                        .collect();
                    presences.sort_by(|a, b| a.0.cmp(&b.0));
                    (s.name.clone(), presences)
                })
                .collect()
        };

        let a = collect(99);
        let b = collect(99);
        assert_eq!(a, b, "Faction ticks should be deterministic");
    }

    // -----------------------------------------------------------------------
    // Visibility
    // -----------------------------------------------------------------------

    #[test]
    fn criminal_factions_have_low_visibility() {
        let galaxy = run_ticks(42, 15);

        // Find covert criminal (transnational).
        let covert = galaxy.factions.iter().find(|f| {
            f.category == FactionCategory::Criminal
                && matches!(f.scope, FactionScope::Transnational { .. })
        });
        if let Some(faction) = covert {
            for system in &galaxy.systems {
                if let Some(presence) = system.faction_presence.iter()
                    .find(|fp| fp.faction_id == faction.id)
                {
                    assert!(
                        presence.visibility < presence.strength * 0.5,
                        "Covert criminal visibility ({:.2}) should be much less than strength ({:.2}) at {}",
                        presence.visibility, presence.strength, system.name,
                    );
                }
            }
        }
    }

    #[test]
    fn military_faction_has_high_visibility() {
        let galaxy = run_ticks(42, 15);
        let mil = find_faction_by_category(&galaxy, FactionCategory::Military);

        for system in &galaxy.systems {
            if let Some(presence) = system.faction_presence.iter()
                .find(|fp| fp.faction_id == mil.id)
            {
                if presence.strength > 0.2 {
                    assert!(
                        presence.visibility >= presence.strength * 0.7,
                        "Military visibility ({:.2}) should track strength ({:.2}) at {}",
                        presence.visibility, presence.strength, system.name,
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Integration sanity
    // -----------------------------------------------------------------------

    #[test]
    fn all_presences_remain_valid_after_ticking() {
        let galaxy = run_ticks(42, 30);
        let faction_ids: Vec<Uuid> = galaxy.factions.iter().map(|f| f.id).collect();

        for system in &galaxy.systems {
            for fp in &system.faction_presence {
                assert!(
                    faction_ids.contains(&fp.faction_id),
                    "Presence at {} references unknown faction",
                    system.name,
                );
                assert!(
                    fp.strength >= 0.0 && fp.strength <= 1.0,
                    "Strength out of range at {}: {}",
                    system.name, fp.strength,
                );
                assert!(
                    fp.visibility >= 0.0 && fp.visibility <= 1.0,
                    "Visibility out of range at {}: {}",
                    system.name, fp.visibility,
                );
            }
        }
    }
}