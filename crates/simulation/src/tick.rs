// file: crates/simulation/src/tick.rs
//! The galactic tick engine.

use rand::rngs::StdRng;
use rand::Rng;
use uuid::Uuid;

use starbound_core::galaxy::*;
use starbound_core::time::Timestamp;

use super::faction_ai::{evaluate_faction, next_infrastructure_level, FactionAction};
use super::generate::GeneratedGalaxy;

const DAYS_PER_TICK: f64 = 365.25;
const MAX_TICKS_PER_CALL: usize = 50;
const DIPLOMACY_DISPOSITION_SHIFT: f32 = 0.05;
const PRESSURE_DISPOSITION_SHIFT: f32 = 0.04;
const MILITARIZE_GROWTH: f32 = 0.03;
const STABILIZE_RECOVERY: f32 = 0.08;
const STABILITY_PASSIVE_DECAY: f32 = 0.01;
const NEW_PRESSURE_CHANCE: f64 = 0.15;

#[derive(Debug, Clone)]
pub struct TickEvent {
    pub tick_number: usize,
    pub galactic_day: f64,
    pub description: String,
    pub entities: Vec<Uuid>,
    pub category: TickEventCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickEventCategory {
    Expansion,
    Infrastructure,
    Diplomacy,
    Military,
    Internal,
}

#[derive(Debug)]
pub struct TickResult {
    pub ticks_run: usize,
    pub days_consumed: f64,
    pub events: Vec<TickEvent>,
}

pub fn tick_galaxy(
    galaxy: &mut GeneratedGalaxy,
    elapsed_galactic_days: f64,
    galactic_day_base: f64,
    rng: &mut StdRng,
) -> TickResult {
    let num_ticks = (elapsed_galactic_days / DAYS_PER_TICK).floor() as usize;
    let num_ticks = num_ticks.min(MAX_TICKS_PER_CALL);
    if num_ticks == 0 {
        return TickResult { ticks_run: 0, days_consumed: 0.0, events: vec![] };
    }
    let mut events = Vec::new();
    for tick in 0..num_ticks {
        let tick_day = galactic_day_base + (tick as f64 * DAYS_PER_TICK);
        let faction_ids: Vec<Uuid> = galaxy.factions.iter().map(|f| f.id).collect();
        for fid in &faction_ids {
            let other_factions: Vec<&Faction> = galaxy.factions.iter().filter(|f| f.id != *fid).collect();
            let faction = galaxy.factions.iter().find(|f| f.id == *fid).unwrap().clone();
            let action = evaluate_faction(&faction, &galaxy.systems, &galaxy.connections, &other_factions, rng);
            let tick_events = resolve_action(&action, galaxy, tick, tick_day);
            events.extend(tick_events);
        }
        apply_passive_effects(galaxy, tick, tick_day, rng, &mut events);
    }
    TickResult { ticks_run: num_ticks, days_consumed: num_ticks as f64 * DAYS_PER_TICK, events }
}

fn resolve_action(action: &FactionAction, galaxy: &mut GeneratedGalaxy, tick_number: usize, galactic_day: f64) -> Vec<TickEvent> {
    match action {
        FactionAction::Expand { faction_id, target_system } => resolve_expand(*faction_id, *target_system, galaxy, tick_number, galactic_day),
        FactionAction::Consolidate { faction_id, target_system } => resolve_consolidate(*faction_id, *target_system, galaxy, tick_number, galactic_day),
        FactionAction::Diplomacy { faction_id, target_faction } => resolve_diplomacy(*faction_id, *target_faction, galaxy, tick_number, galactic_day),
        FactionAction::Pressure { faction_id, target_faction } => resolve_pressure(*faction_id, *target_faction, galaxy, tick_number, galactic_day),
        FactionAction::Militarize { faction_id } => resolve_militarize(*faction_id, galaxy, tick_number, galactic_day),
        FactionAction::Stabilize { faction_id } => resolve_stabilize(*faction_id, galaxy, tick_number, galactic_day),
        FactionAction::Idle { .. } => vec![],
    }
}

fn resolve_expand(faction_id: Uuid, target_system: Uuid, galaxy: &mut GeneratedGalaxy, tick_number: usize, galactic_day: f64) -> Vec<TickEvent> {
    let fname = get_faction_name(faction_id, &galaxy.factions);
    if let Some(system) = galaxy.systems.iter_mut().find(|s| s.id == target_system) {
        if system.controlling_faction.is_some() { return vec![]; }
        system.controlling_faction = Some(faction_id);
        if system.infrastructure_level == InfrastructureLevel::None {
            system.infrastructure_level = InfrastructureLevel::Outpost;
        }
        system.history.push(HistoryEntry {
            timestamp: Timestamp { personal_days: 0.0, galactic_days: galactic_day },
            description: format!("{} established control.", fname),
        });
        let sname = system.name.clone();
        if let Some(f) = galaxy.factions.iter_mut().find(|f| f.id == faction_id) {
            f.capabilities.wealth = (f.capabilities.wealth - 0.02).max(0.0);
            f.capabilities.size = (f.capabilities.size + 0.05).min(1.0);
        }
        vec![TickEvent { tick_number, galactic_day, description: format!("{} claimed {}.", fname, sname), entities: vec![faction_id, target_system], category: TickEventCategory::Expansion }]
    } else { vec![] }
}

fn resolve_consolidate(faction_id: Uuid, target_system: Uuid, galaxy: &mut GeneratedGalaxy, tick_number: usize, galactic_day: f64) -> Vec<TickEvent> {
    let fname = get_faction_name(faction_id, &galaxy.factions);
    if let Some(system) = galaxy.systems.iter_mut().find(|s| s.id == target_system) {
        let old_level = system.infrastructure_level;
        system.infrastructure_level = next_infrastructure_level(old_level);
        if system.infrastructure_level != old_level {
            system.history.push(HistoryEntry {
                timestamp: Timestamp { personal_days: 0.0, galactic_days: galactic_day },
                description: format!("Infrastructure upgraded to {}.", infra_label(system.infrastructure_level)),
            });
            let sname = system.name.clone();
            return vec![TickEvent { tick_number, galactic_day, description: format!("{} upgraded {} to {}.", fname, sname, infra_label(system.infrastructure_level)), entities: vec![faction_id, target_system], category: TickEventCategory::Infrastructure }];
        }
    }
    vec![]
}

fn resolve_diplomacy(faction_id: Uuid, target_faction: Uuid, galaxy: &mut GeneratedGalaxy, tick_number: usize, galactic_day: f64) -> Vec<TickEvent> {
    let a_name = get_faction_name(faction_id, &galaxy.factions);
    let b_name = get_faction_name(target_faction, &galaxy.factions);
    shift_disposition(galaxy, faction_id, target_faction, DIPLOMACY_DISPOSITION_SHIFT, DIPLOMACY_DISPOSITION_SHIFT * 0.5, 0.0);
    shift_disposition(galaxy, target_faction, faction_id, DIPLOMACY_DISPOSITION_SHIFT * 0.5, DIPLOMACY_DISPOSITION_SHIFT * 0.3, 0.0);
    vec![TickEvent { tick_number, galactic_day, description: format!("Relations between {} and {} improved through diplomatic channels.", a_name, b_name), entities: vec![faction_id, target_faction], category: TickEventCategory::Diplomacy }]
}

fn resolve_pressure(faction_id: Uuid, target_faction: Uuid, galaxy: &mut GeneratedGalaxy, tick_number: usize, galactic_day: f64) -> Vec<TickEvent> {
    let a_name = get_faction_name(faction_id, &galaxy.factions);
    let b_name = get_faction_name(target_faction, &galaxy.factions);
    shift_disposition(galaxy, faction_id, target_faction, -PRESSURE_DISPOSITION_SHIFT * 0.5, 0.0, -PRESSURE_DISPOSITION_SHIFT);
    shift_disposition(galaxy, target_faction, faction_id, -PRESSURE_DISPOSITION_SHIFT, 0.0, -PRESSURE_DISPOSITION_SHIFT * 0.5);
    if let Some(target) = galaxy.factions.iter_mut().find(|f| f.id == target_faction) {
        target.internal_dynamics.stability = (target.internal_dynamics.stability - 0.02).max(0.0);
    }
    vec![TickEvent { tick_number, galactic_day, description: format!("Tensions rose between {} and {}.", a_name, b_name), entities: vec![faction_id, target_faction], category: TickEventCategory::Military }]
}

fn resolve_militarize(faction_id: Uuid, galaxy: &mut GeneratedGalaxy, tick_number: usize, galactic_day: f64) -> Vec<TickEvent> {
    let fname = get_faction_name(faction_id, &galaxy.factions);
    if let Some(faction) = galaxy.factions.iter_mut().find(|f| f.id == faction_id) {
        let old = faction.capabilities.military;
        faction.capabilities.military = (old + MILITARIZE_GROWTH).min(1.0);
        faction.capabilities.wealth = (faction.capabilities.wealth - 0.01).max(0.0);
        if faction.capabilities.military > old {
            return vec![TickEvent { tick_number, galactic_day, description: format!("{} expanded its military capabilities.", fname), entities: vec![faction_id], category: TickEventCategory::Military }];
        }
    }
    vec![]
}

fn resolve_stabilize(faction_id: Uuid, galaxy: &mut GeneratedGalaxy, tick_number: usize, galactic_day: f64) -> Vec<TickEvent> {
    let fname = get_faction_name(faction_id, &galaxy.factions);
    if let Some(faction) = galaxy.factions.iter_mut().find(|f| f.id == faction_id) {
        faction.internal_dynamics.stability = (faction.internal_dynamics.stability + STABILIZE_RECOVERY).min(1.0);
        if !faction.internal_dynamics.pressures.is_empty() && faction.internal_dynamics.stability > 0.6 {
            let resolved = faction.internal_dynamics.pressures.remove(0);
            return vec![TickEvent { tick_number, galactic_day, description: format!("{} addressed internal pressures: {}", fname, resolved), entities: vec![faction_id], category: TickEventCategory::Internal }];
        }
        return vec![TickEvent { tick_number, galactic_day, description: format!("{} focused on internal consolidation.", fname), entities: vec![faction_id], category: TickEventCategory::Internal }];
    }
    vec![]
}

fn apply_passive_effects(galaxy: &mut GeneratedGalaxy, tick_number: usize, galactic_day: f64, rng: &mut StdRng, events: &mut Vec<TickEvent>) {
    for faction in galaxy.factions.iter_mut() {
        if !faction.internal_dynamics.pressures.is_empty() {
            let decay = STABILITY_PASSIVE_DECAY * faction.internal_dynamics.pressures.len() as f32;
            faction.internal_dynamics.stability = (faction.internal_dynamics.stability - decay).max(0.0);
        }
        if rng.gen_bool(NEW_PRESSURE_CHANCE) {
            let pressure = generate_pressure(rng);
            let fname = faction.name.clone();
            faction.internal_dynamics.pressures.push(pressure.clone());
            events.push(TickEvent { tick_number, galactic_day, description: format!("Unrest within {}: {}", fname, pressure), entities: vec![faction.id], category: TickEventCategory::Internal });
        }
    }
}

fn generate_pressure(rng: &mut StdRng) -> String {
    let pool = [
        "Trade unions demanding better terms",
        "Outer colony resource disputes",
        "Currency instability in border systems",
        "Reform movement gaining popular support",
        "Opposition faction calling for elections",
        "Provincial governors asserting autonomy",
        "Veterans demanding recognition and resources",
        "Border patrol reporting increased pirate activity",
        "Military leaders questioning civilian oversight",
        "Religious revival clashing with secular governance",
        "Immigrant populations facing discrimination",
        "Historical revisionism sparking public debate",
        "AI rights movement gaining traction",
        "Technology gap between core and frontier worlds",
        "Research ethics scandal involving classified programs",
    ];
    let idx = rng.gen_range(0..pool.len());
    pool[idx].to_string()
}

fn get_faction_name(id: Uuid, factions: &[Faction]) -> String {
    factions.iter().find(|f| f.id == id).map(|f| f.name.clone()).unwrap_or_else(|| "Unknown Faction".into())
}

fn infra_label(level: InfrastructureLevel) -> &'static str {
    match level {
        InfrastructureLevel::None => "uninhabited",
        InfrastructureLevel::Outpost => "outpost",
        InfrastructureLevel::Colony => "colony",
        InfrastructureLevel::Established => "established settlement",
        InfrastructureLevel::Hub => "trade hub",
        InfrastructureLevel::Capital => "capital",
    }
}

fn shift_disposition(galaxy: &mut GeneratedGalaxy, from: Uuid, toward: Uuid, diplomatic_delta: f32, economic_delta: f32, military_delta: f32) {
    if let Some(faction) = galaxy.factions.iter_mut().find(|f| f.id == from) {
        if let Some(disp) = faction.relationships.get_mut(&toward) {
            disp.diplomatic = (disp.diplomatic + diplomatic_delta).clamp(-1.0, 1.0);
            disp.economic = (disp.economic + economic_delta).clamp(0.0, 1.0);
            disp.military = (disp.military + military_delta).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use crate::generate::generate_galaxy;

    #[test]
    fn zero_days_produces_no_ticks() {
        let mut galaxy = generate_galaxy(42);
        let mut rng = StdRng::seed_from_u64(42);
        let result = tick_galaxy(&mut galaxy, 100.0, 0.0, &mut rng);
        assert_eq!(result.ticks_run, 0);
        assert!(result.events.is_empty());
    }

    #[test]
    fn one_year_produces_one_tick() {
        let mut galaxy = generate_galaxy(42);
        let mut rng = StdRng::seed_from_u64(42);
        let result = tick_galaxy(&mut galaxy, 400.0, 0.0, &mut rng);
        assert_eq!(result.ticks_run, 1);
        assert!(!result.events.is_empty());
    }

    #[test]
    fn multiple_years_produce_multiple_ticks() {
        let mut galaxy = generate_galaxy(42);
        let mut rng = StdRng::seed_from_u64(42);
        let result = tick_galaxy(&mut galaxy, 1830.0, 0.0, &mut rng);
        assert_eq!(result.ticks_run, 5);
    }

    #[test]
    fn max_ticks_caps_computation() {
        let mut galaxy = generate_galaxy(42);
        let mut rng = StdRng::seed_from_u64(42);
        let result = tick_galaxy(&mut galaxy, 36525.0, 0.0, &mut rng);
        assert_eq!(result.ticks_run, MAX_TICKS_PER_CALL);
    }

    #[test]
    fn galaxy_changes_over_time() {
        let mut galaxy = generate_galaxy(42);
        let mut rng = StdRng::seed_from_u64(42);
        let initial_unclaimed: usize = galaxy.systems.iter().filter(|s| s.controlling_faction.is_none()).count();
        let result = tick_galaxy(&mut galaxy, 3652.5, 0.0, &mut rng);
        assert!(!result.events.is_empty(), "10 years should produce events");
        let has_expansion = result.events.iter().any(|e| e.category == TickEventCategory::Expansion);
        if has_expansion {
            let final_unclaimed: usize = galaxy.systems.iter().filter(|s| s.controlling_faction.is_none()).count();
            assert!(final_unclaimed < initial_unclaimed);
        }
    }

    #[test]
    fn ticks_are_deterministic() {
        // Uuid::new_v4() is truly random (not seeded by the game RNG),
        // so we compare structure (faction names per system) not raw IDs.
        let run = |seed: u64| {
            let mut galaxy = generate_galaxy(seed);
            let mut rng = StdRng::seed_from_u64(seed);
            let result = tick_galaxy(&mut galaxy, 1830.0, 0.0, &mut rng);
            let descs: Vec<String> = result.events.iter()
                .map(|e| e.description.clone())
                .collect();
            let ownership: Vec<String> = galaxy.systems.iter()
                .map(|s| match s.controlling_faction {
                    Some(fid) => get_faction_name(fid, &galaxy.factions),
                    None => "None".into(),
                })
                .collect();
            (descs, ownership)
        };
        let (a, sa) = run(99);
        let (b, sb) = run(99);
        assert_eq!(a, b);
        assert_eq!(sa, sb);
    }

    #[test]
    fn history_entries_accumulate() {
        let mut galaxy = generate_galaxy(42);
        let mut rng = StdRng::seed_from_u64(42);
        let initial: usize = galaxy.systems.iter().map(|s| s.history.len()).sum();
        tick_galaxy(&mut galaxy, 3652.5, 0.0, &mut rng);
        let final_count: usize = galaxy.systems.iter().map(|s| s.history.len()).sum();
        assert!(final_count >= initial);
    }
}