// file: crates/game/src/travel.rs
//! Travel execution — applies a TravelPlan to the journey state.
//!
//! The simulation crate calculates what travel *would* cost.
//! This module actually does it: advances time, burns fuel,
//! moves the player, and logs the event.

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
    let from_system = journey.current_system;
    journey.current_system = plan.destination_id;

    // Log the event.
    let mode_str = match plan.mode {
        TravelMode::Ftl => "FTL",
        TravelMode::Sublight => "sublight",
        TravelMode::Stationary => "stationary",
    };

    let time_desc = if plan.duration.personal_days >= 30.0 {
        format!(
            "{:.1} months personal, {:.1} months galactic",
            plan.duration.personal_months(),
            plan.duration.galactic_days / 30.44,
        )
    } else {
        format!(
            "{:.0} days personal, {:.0} days galactic",
            plan.duration.personal_days,
            plan.duration.galactic_days,
        )
    };

    let event = GameEvent {
        timestamp: journey.time,
        category: EventCategory::Travel,
        description: format!(
            "Arrived at {} via {} transit ({:.1} ly). {}.",
            destination_name,
            mode_str,
            plan.connection.distance_ly,
            time_desc,
        ),
        associated_entities: vec![from_system, plan.destination_id],
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
            civ_standings: HashMap::new(),
        }
    }

    #[test]
    fn ftl_travel_moves_player_and_advances_time() {
        let system_a = Uuid::new_v4();
        let system_b = Uuid::new_v4();
        let conn = Connection {
            system_a,
            system_b,
            distance_ly: 5.0,
            route_type: RouteType::Open,
        };

        let mut journey = test_journey(system_a, 100.0);
        let plan = plan_travel(&conn, &journey.ship, TravelMode::Ftl, system_a);
        let outcome = execute_travel(&mut journey, &plan, "Cygnus Gate").unwrap();

        // Player moved.
        assert_eq!(journey.current_system, system_b);

        // Time advanced — days, not decades.
        assert!(journey.time.personal_days > 10.0 && journey.time.personal_days < 30.0);
        assert!(journey.time.galactic_days > 15.0 && journey.time.galactic_days < 40.0);

        // Roughly in sync.
        let ratio = journey.time.galactic_days / journey.time.personal_days;
        assert!(ratio < 2.0, "FTL should keep times roughly in sync");

        // Fuel burned.
        assert!(journey.ship.fuel < 100.0);
        assert!(outcome.fuel_spent > 0.0);

        // Event logged.
        assert_eq!(journey.event_log.len(), 1);
        assert!(journey.event_log[0].description.contains("FTL"));
    }

    #[test]
    fn sublight_fallback_is_free_but_slow() {
        let system_a = Uuid::new_v4();
        let system_b = Uuid::new_v4();
        let conn = Connection {
            system_a,
            system_b,
            distance_ly: 5.0,
            route_type: RouteType::Open,
        };

        let mut journey = test_journey(system_a, 0.0); // No fuel
        let plan = plan_travel(&conn, &journey.ship, TravelMode::Sublight, system_a);
        let outcome = execute_travel(&mut journey, &plan, "Drift").unwrap();

        assert_eq!(journey.current_system, system_b);
        assert_eq!(journey.ship.fuel, 0.0); // No fuel consumed
        assert!(outcome.personal_days > 100.0, "Sublight should take months");

        // Modest dilation, not decades.
        let ratio = journey.time.galactic_days / journey.time.personal_days;
        assert!(ratio > 2.0 && ratio < 4.0);
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

        let mut journey = test_journey(system_c, 100.0);
        let plan = plan_travel(&conn, &journey.ship, TravelMode::Ftl, system_a);

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
            distance_ly: 50.0, // Way too far
            route_type: RouteType::Open,
        };

        let mut journey = test_journey(system_a, 10.0);
        let plan = plan_travel(&conn, &journey.ship, TravelMode::Ftl, system_a);

        assert!(!plan.feasible);
        let result = execute_travel(&mut journey, &plan, "Anywhere");
        assert!(result.is_err());

        // Nothing changed.
        assert_eq!(journey.current_system, system_a);
        assert_eq!(journey.ship.fuel, 10.0);
    }
}