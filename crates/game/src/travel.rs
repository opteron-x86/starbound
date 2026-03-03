// file: crates/game/src/travel.rs
//! Travel execution — applies a TravelPlan to the journey state.
//!
//! The simulation crate calculates what travel *would* cost.
//! This module actually does it: advances time, burns fuel,
//! moves the player, logs the event, and saves.

use starbound_core::journey::Journey;
use starbound_core::narrative::{EventCategory, GameEvent};
use starbound_core::ship::TravelMode;
use starbound_simulation::travel::TravelPlan;

/// The result of executing travel — what happened and why.
#[derive(Debug)]
pub struct TravelOutcome {
    /// Name of destination (for display).
    pub destination_name: String,
    /// How long the journey took.
    pub personal_days: f64,
    pub galactic_days: f64,
    /// Fuel burned.
    pub fuel_spent: f32,
    /// How much galactic time has now elapsed total.
    pub total_galactic_years: f64,
    /// How much personal time has now elapsed total.
    pub total_personal_years: f64,
}

/// Execute a travel plan, mutating the journey in place.
/// Returns an error string if the plan isn't feasible.
pub fn execute_travel(
    journey: &mut Journey,
    plan: &TravelPlan,
    destination_name: &str,
) -> Result<TravelOutcome, String> {
    // Guard: plan must be feasible.
    if !plan.feasible {
        return Err(plan
            .infeasible_reason
            .clone()
            .unwrap_or_else(|| "Travel is not feasible.".into()));
    }

    // Guard: player must be at the departure system.
    if journey.current_system != plan.connection.system_a
        && journey.current_system != plan.connection.system_b
    {
        return Err("You are not at either end of this route.".into());
    }

    // Burn fuel.
    journey.ship.fuel -= plan.fuel_cost;

    // Advance time.
    journey.time += plan.duration;

    // Move player.
    let from_name_context = journey.current_system;
    journey.current_system = plan.destination_id;

    // Log the event.
    let mode_str = match plan.mode {
        TravelMode::Sublight => "sublight",
        TravelMode::Ftl => "FTL",
        TravelMode::Stationary => "stationary",
    };

    let event = GameEvent {
        timestamp: journey.time,
        category: EventCategory::Travel,
        description: format!(
            "Arrived at {} via {} transit ({:.1} ly). \
             {:.1} months personal time, {:.1} years galactic time elapsed.",
            destination_name,
            mode_str,
            plan.connection.distance_ly,
            plan.duration.personal_months(),
            plan.duration.galactic_years(),
        ),
        associated_entities: vec![from_name_context, plan.destination_id],
        consequences: vec![],
    };
    journey.event_log.push(event);

    Ok(TravelOutcome {
        destination_name: destination_name.to_string(),
        personal_days: plan.duration.personal_days,
        galactic_days: plan.duration.galactic_days,
        fuel_spent: plan.fuel_cost,
        total_galactic_years: journey.time.galactic_years(),
        total_personal_years: journey.time.personal_years(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use starbound_core::galaxy::{Connection, RouteType};
    use starbound_core::mission::*;
    use starbound_core::ship::*;
    use starbound_core::time::Timestamp;
    use starbound_simulation::travel::plan_travel;
    use uuid::Uuid;

    fn test_journey(current_system: Uuid, fuel: f32) -> Journey {
        Journey {
            ship: Ship {
                name: "Test Ship".into(),
                hull_condition: 1.0,
                fuel,
                fuel_capacity: 100.0,
                cargo: HashMap::new(),
                cargo_capacity: 50,
                modules: ShipModules {
                    engine: Module::standard("Test Engine"),
                    sensors: Module::standard("Sensors"),
                    comms: Module::standard("Comms"),
                    weapons: Module::standard("Weapons"),
                    life_support: Module::standard("Life Support"),
                },
            },
            current_system,
            time: Timestamp::zero(),
            resources: 1000.0,
            mission: MissionState {
                mission_type: MissionType::Search,
                core_truth: "Test".into(),
                knowledge_nodes: vec![],
            },
            crew: vec![],
            threads: vec![],
            event_log: vec![],
        }
    }

    #[test]
    fn sublight_travel_advances_time_and_moves_player() {
        let system_a = Uuid::new_v4();
        let system_b = Uuid::new_v4();
        let conn = Connection {
            system_a,
            system_b,
            distance_ly: 8.0,
            route_type: RouteType::Open,
        };

        let mut journey = test_journey(system_a, 100.0);
        let plan = plan_travel(&conn, &journey.ship, TravelMode::Sublight, system_a);

        let outcome = execute_travel(&mut journey, &plan, "Cygnus Gate").unwrap();

        // Player moved.
        assert_eq!(journey.current_system, system_b);

        // Time advanced.
        assert!(journey.time.personal_days > 0.0);
        assert!(journey.time.galactic_days > 0.0);
        assert!(journey.time.galactic_days > journey.time.personal_days * 50.0,
            "Galactic time should far exceed personal time for sublight");

        // No fuel spent.
        assert_eq!(journey.ship.fuel, 100.0);

        // Event logged.
        assert_eq!(journey.event_log.len(), 1);
        assert!(journey.event_log[0].description.contains("Cygnus Gate"));
        assert!(journey.event_log[0].description.contains("sublight"));

        // Outcome has readable numbers.
        assert!(outcome.galactic_days > 10_000.0);
    }

    #[test]
    fn ftl_travel_burns_fuel_stays_in_sync() {
        let system_a = Uuid::new_v4();
        let system_b = Uuid::new_v4();
        let conn = Connection {
            system_a,
            system_b,
            distance_ly: 8.0,
            route_type: RouteType::FtlLane,
        };

        let mut journey = test_journey(system_a, 100.0);
        let plan = plan_travel(&conn, &journey.ship, TravelMode::Ftl, system_a);

        let outcome = execute_travel(&mut journey, &plan, "Pale Harbor").unwrap();

        // Player moved.
        assert_eq!(journey.current_system, system_b);

        // Fuel burned.
        assert!(journey.ship.fuel < 100.0);
        assert!(outcome.fuel_spent > 0.0);

        // Time roughly in sync.
        let ratio = journey.time.galactic_days / journey.time.personal_days;
        assert!(ratio < 2.0, "FTL should keep times close, ratio was {:.1}", ratio);
    }

    #[test]
    fn cannot_travel_from_wrong_system() {
        let system_a = Uuid::new_v4();
        let system_b = Uuid::new_v4();
        let system_c = Uuid::new_v4();
        let conn = Connection {
            system_a,
            system_b,
            distance_ly: 5.0,
            route_type: RouteType::Open,
        };

        let mut journey = test_journey(system_c, 100.0); // At system_c, not a or b
        let plan = plan_travel(&conn, &journey.ship, TravelMode::Sublight, system_a);

        let result = execute_travel(&mut journey, &plan, "Nowhere");
        assert!(result.is_err());
    }

    #[test]
    fn infeasible_plan_rejected() {
        let system_a = Uuid::new_v4();
        let system_b = Uuid::new_v4();
        let conn = Connection {
            system_a,
            system_b,
            distance_ly: 8.0,
            route_type: RouteType::FtlLane,
        };

        let mut journey = test_journey(system_a, 1.0); // Almost no fuel
        let plan = plan_travel(&conn, &journey.ship, TravelMode::Ftl, system_a);

        assert!(!plan.feasible);
        let result = execute_travel(&mut journey, &plan, "Anywhere");
        assert!(result.is_err());

        // Nothing should have changed.
        assert_eq!(journey.current_system, system_a);
        assert_eq!(journey.ship.fuel, 1.0);
        assert!(journey.event_log.is_empty());
    }

    #[test]
    fn multiple_trips_accumulate_time() {
        let system_a = Uuid::new_v4();
        let system_b = Uuid::new_v4();
        let conn = Connection {
            system_a,
            system_b,
            distance_ly: 4.0,
            route_type: RouteType::Open,
        };

        let mut journey = test_journey(system_a, 100.0);

        // Trip 1: A → B
        let plan1 = plan_travel(&conn, &journey.ship, TravelMode::Sublight, system_a);
        execute_travel(&mut journey, &plan1, "System B").unwrap();
        let time_after_first = journey.time;

        // Trip 2: B → A
        let plan2 = plan_travel(&conn, &journey.ship, TravelMode::Sublight, system_b);
        execute_travel(&mut journey, &plan2, "System A").unwrap();

        // Time should have roughly doubled.
        assert!(journey.time.personal_days > time_after_first.personal_days * 1.9);
        assert!(journey.time.galactic_days > time_after_first.galactic_days * 1.9);
        assert_eq!(journey.event_log.len(), 2);
        assert_eq!(journey.current_system, system_a);
    }
}