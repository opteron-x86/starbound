// file: crates/simulation/src/travel.rs
//! Travel calculations — the game's central mechanic made concrete.
//!
//! Reference numbers from the design doc:
//! - 8 light-years sublight → ~6 months personal time, ~40 years galactic time
//! - FTL keeps you roughly in sync but costs heavily
//!
//! This module handles pure calculation. It takes distances and ship
//! state, returns durations and costs. No state mutation — that happens
//! in the game crate.

use starbound_core::galaxy::{Connection, RouteType};
use starbound_core::ship::{Ship, TravelMode};
use starbound_core::time::Duration;

// ---------------------------------------------------------------------------
// Constants — tuned to match the design doc's feel
// ---------------------------------------------------------------------------

/// Sublight personal time: ~22.8 days per light-year.
/// (6 months / 8 ly = 182.6 days / 8 ly ≈ 22.8 days/ly)
const SUBLIGHT_PERSONAL_DAYS_PER_LY: f64 = 22.8;

/// Sublight galactic time: ~1826 days (5 years) per light-year.
/// (40 years / 8 ly = 14,610 days / 8 ly ≈ 1826 days/ly)
const SUBLIGHT_GALACTIC_DAYS_PER_LY: f64 = 1826.25;

/// FTL personal time: ~7 days per light-year (a week per ly).
const FTL_PERSONAL_DAYS_PER_LY: f64 = 7.0;

/// FTL galactic time: ~10 days per light-year (slightly more than personal
/// — you're fast but not perfectly in sync).
const FTL_GALACTIC_DAYS_PER_LY: f64 = 10.0;

/// FTL fuel cost per light-year. Sublight is free.
const FTL_FUEL_PER_LY: f32 = 5.0;

/// Hazardous routes multiply travel time (detours, caution).
const HAZARDOUS_TIME_MULTIPLIER: f64 = 1.4;

/// Engine condition below this threshold slows you down.
const ENGINE_DEGRADATION_THRESHOLD: f32 = 0.5;

// ---------------------------------------------------------------------------
// Travel calculation
// ---------------------------------------------------------------------------

/// Everything the caller needs to know about a potential journey.
#[derive(Debug, Clone)]
pub struct TravelPlan {
    /// Where you're going.
    pub destination_id: uuid::Uuid,
    /// The route you'd take.
    pub connection: Connection,
    /// Whether this is sublight or FTL.
    pub mode: TravelMode,
    /// How long it takes on both timescales.
    pub duration: Duration,
    /// Fuel consumed (0 for sublight).
    pub fuel_cost: f32,
    /// Can the ship actually make this trip?
    pub feasible: bool,
    /// Why it's not feasible, if applicable.
    pub infeasible_reason: Option<String>,
}

/// Calculate a travel plan for a given connection and ship state.
pub fn plan_travel(
    connection: &Connection,
    ship: &Ship,
    mode: TravelMode,
    from_system: uuid::Uuid,
) -> TravelPlan {
    let destination_id = if connection.system_a == from_system {
        connection.system_b
    } else {
        connection.system_a
    };

    let distance = connection.distance_ly;

    // Base duration depends on travel mode.
    let (base_personal, base_galactic, base_fuel) = match mode {
        TravelMode::Sublight => (
            distance * SUBLIGHT_PERSONAL_DAYS_PER_LY,
            distance * SUBLIGHT_GALACTIC_DAYS_PER_LY,
            0.0_f32,
        ),
        TravelMode::Ftl => (
            distance * FTL_PERSONAL_DAYS_PER_LY,
            distance * FTL_GALACTIC_DAYS_PER_LY,
            (distance as f32) * FTL_FUEL_PER_LY,
        ),
        TravelMode::Stationary => {
            return TravelPlan {
                destination_id,
                connection: connection.clone(),
                mode,
                duration: Duration { personal_days: 0.0, galactic_days: 0.0 },
                fuel_cost: 0.0,
                feasible: false,
                infeasible_reason: Some("Ship is stationary.".into()),
            };
        }
    };

    // Route hazards increase travel time.
    let route_multiplier = match connection.route_type {
        RouteType::Hazardous => HAZARDOUS_TIME_MULTIPLIER,
        _ => 1.0,
    };

    // FTL requires an FTL lane unless the player is desperate.
    let ftl_on_non_ftl_route = mode == TravelMode::Ftl
        && connection.route_type != RouteType::FtlLane
        && connection.route_type != RouteType::Corridor;

    // Degraded engine slows you down.
    let engine_multiplier = engine_condition_multiplier(ship.modules.engine.condition);

    let personal_days = base_personal * route_multiplier * engine_multiplier;
    let galactic_days = base_galactic * route_multiplier * engine_multiplier;
    let fuel_cost = base_fuel * engine_multiplier as f32; // worse engine burns more

    // Feasibility checks.
    let (feasible, infeasible_reason) = check_feasibility(
        ship,
        fuel_cost,
        mode,
        ftl_on_non_ftl_route,
    );

    TravelPlan {
        destination_id,
        connection: connection.clone(),
        mode,
        duration: Duration {
            personal_days,
            galactic_days,
        },
        fuel_cost,
        feasible,
        infeasible_reason,
    }
}

/// Calculate travel plans for all available routes from the current system.
pub fn plan_all_routes(
    connections: &[Connection],
    ship: &Ship,
    current_system: uuid::Uuid,
) -> Vec<TravelPlan> {
    let mut plans = Vec::new();

    for conn in connections {
        // Sublight is always an option.
        plans.push(plan_travel(conn, ship, TravelMode::Sublight, current_system));

        // FTL is an option on FTL lanes and corridors (elsewhere it's risky).
        if conn.route_type == RouteType::FtlLane || conn.route_type == RouteType::Corridor {
            plans.push(plan_travel(conn, ship, TravelMode::Ftl, current_system));
        }
    }

    plans
}

fn engine_condition_multiplier(condition: f32) -> f64 {
    if condition >= ENGINE_DEGRADATION_THRESHOLD {
        1.0
    } else if condition <= 0.0 {
        // Engine is dead. Travel takes forever (effectively impossible).
        10.0
    } else {
        // Linear degradation: at 0.5 → 1.0x, at 0.0 → 3.0x
        let t = condition / ENGINE_DEGRADATION_THRESHOLD;
        1.0 + (1.0 - t as f64) * 2.0
    }
}

fn check_feasibility(
    ship: &Ship,
    fuel_cost: f32,
    mode: TravelMode,
    ftl_on_non_ftl_route: bool,
) -> (bool, Option<String>) {
    if ship.modules.engine.condition <= 0.0 {
        return (false, Some("Engine is non-functional.".into()));
    }

    if mode == TravelMode::Ftl && fuel_cost > ship.fuel {
        return (
            false,
            Some(format!(
                "Insufficient fuel: need {:.1}, have {:.1}.",
                fuel_cost, ship.fuel
            )),
        );
    }

    if ftl_on_non_ftl_route {
        return (
            false,
            Some("No FTL lane on this route.".into()),
        );
    }

    (true, None)
}

/// Format a travel plan as a human-readable summary (for the CLI).
pub fn describe_plan(plan: &TravelPlan, dest_name: &str) -> String {
    let mode_str = match plan.mode {
        TravelMode::Sublight => "Sublight",
        TravelMode::Ftl => "FTL",
        TravelMode::Stationary => "Stationary",
    };

    let time_str = if plan.duration.galactic_years() >= 1.0 {
        format!(
            "{:.1} months personal / {:.1} years galactic",
            plan.duration.personal_months(),
            plan.duration.galactic_years()
        )
    } else {
        format!(
            "{:.0} days personal / {:.0} days galactic",
            plan.duration.personal_days,
            plan.duration.galactic_days
        )
    };

    let fuel_str = if plan.fuel_cost > 0.0 {
        format!(" | fuel: {:.1}", plan.fuel_cost)
    } else {
        " | no fuel cost".into()
    };

    let feasibility = if !plan.feasible {
        format!(" [BLOCKED: {}]", plan.infeasible_reason.as_deref().unwrap_or("unknown"))
    } else {
        String::new()
    };

    format!(
        "{} → {} ({}) — {}{}{}", 
        mode_str,
        dest_name,
        format!("{:.1} ly", plan.connection.distance_ly),
        time_str,
        fuel_str,
        feasibility,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use starbound_core::ship::{Module, Ship, ShipModules};

    fn test_ship(fuel: f32, engine_condition: f32) -> Ship {
        Ship {
            name: "Test Ship".into(),
            hull_condition: 1.0,
            fuel,
            fuel_capacity: 100.0,
            cargo: HashMap::new(),
            cargo_capacity: 50,
            modules: ShipModules {
                engine: Module {
                    variant: "Test Engine".into(),
                    condition: engine_condition,
                    notes: vec![],
                },
                sensors: Module::standard("Sensors"),
                comms: Module::standard("Comms"),
                weapons: Module::standard("Weapons"),
                life_support: Module::standard("Life Support"),
            },
        }
    }

    fn test_connection(distance_ly: f64, route_type: RouteType) -> Connection {
        Connection {
            system_a: uuid::Uuid::new_v4(),
            system_b: uuid::Uuid::new_v4(),
            distance_ly,
            route_type,
        }
    }

    #[test]
    fn sublight_matches_design_doc() {
        // Design doc: 8 ly → ~6 months personal, ~40 years galactic
        let ship = test_ship(100.0, 1.0);
        let conn = test_connection(8.0, RouteType::Open);
        let plan = plan_travel(&conn, &ship, TravelMode::Sublight, conn.system_a);

        assert!(plan.feasible);
        assert_eq!(plan.fuel_cost, 0.0);

        let personal_months = plan.duration.personal_months();
        let galactic_years = plan.duration.galactic_years();

        // Should be close to 6 months personal.
        assert!(personal_months > 5.5 && personal_months < 6.5,
            "Expected ~6 months personal, got {:.1}", personal_months);

        // Should be close to 40 years galactic.
        assert!(galactic_years > 39.0 && galactic_years < 41.0,
            "Expected ~40 years galactic, got {:.1}", galactic_years);
    }

    #[test]
    fn ftl_is_fast_but_costly() {
        let ship = test_ship(100.0, 1.0);
        let conn = test_connection(8.0, RouteType::FtlLane);

        let sublight = plan_travel(&conn, &ship, TravelMode::Sublight, conn.system_a);
        let ftl = plan_travel(&conn, &ship, TravelMode::Ftl, conn.system_a);

        // FTL should be much faster on both timescales.
        assert!(ftl.duration.personal_days < sublight.duration.personal_days / 2.0);
        assert!(ftl.duration.galactic_days < sublight.duration.galactic_days / 10.0);

        // But costs fuel.
        assert!(ftl.fuel_cost > 0.0);
        assert_eq!(sublight.fuel_cost, 0.0);

        // FTL personal and galactic time should be close to each other.
        let ratio = ftl.duration.galactic_days / ftl.duration.personal_days;
        assert!(ratio < 2.0, "FTL should keep you roughly in sync, ratio was {:.1}", ratio);
    }

    #[test]
    fn insufficient_fuel_blocks_ftl() {
        let ship = test_ship(5.0, 1.0); // Only 5 fuel
        let conn = test_connection(8.0, RouteType::FtlLane);
        let plan = plan_travel(&conn, &ship, TravelMode::Ftl, conn.system_a);

        assert!(!plan.feasible);
        assert!(plan.infeasible_reason.as_ref().unwrap().contains("fuel"));
    }

    #[test]
    fn no_ftl_on_open_routes() {
        let ship = test_ship(100.0, 1.0);
        let conn = test_connection(4.0, RouteType::Open);
        let plan = plan_travel(&conn, &ship, TravelMode::Ftl, conn.system_a);

        assert!(!plan.feasible);
        assert!(plan.infeasible_reason.as_ref().unwrap().contains("FTL lane"));
    }

    #[test]
    fn hazardous_routes_take_longer() {
        let ship = test_ship(100.0, 1.0);
        let normal = test_connection(5.0, RouteType::Open);
        let hazardous = test_connection(5.0, RouteType::Hazardous);

        let plan_normal = plan_travel(&normal, &ship, TravelMode::Sublight, normal.system_a);
        let plan_hazard = plan_travel(&hazardous, &ship, TravelMode::Sublight, hazardous.system_a);

        assert!(plan_hazard.duration.personal_days > plan_normal.duration.personal_days);
        assert!(plan_hazard.duration.galactic_days > plan_normal.duration.galactic_days);
    }

    #[test]
    fn damaged_engine_slows_travel() {
        let good_ship = test_ship(100.0, 1.0);
        let bad_ship = test_ship(100.0, 0.25); // Half of threshold
        let conn = test_connection(5.0, RouteType::Open);

        let plan_good = plan_travel(&conn, &good_ship, TravelMode::Sublight, conn.system_a);
        let plan_bad = plan_travel(&conn, &bad_ship, TravelMode::Sublight, conn.system_a);

        assert!(plan_bad.duration.personal_days > plan_good.duration.personal_days,
            "Damaged engine should slow travel");
    }

    #[test]
    fn dead_engine_blocks_travel() {
        let ship = test_ship(100.0, 0.0);
        let conn = test_connection(5.0, RouteType::Open);
        let plan = plan_travel(&conn, &ship, TravelMode::Sublight, conn.system_a);

        assert!(!plan.feasible);
        assert!(plan.infeasible_reason.as_ref().unwrap().contains("non-functional"));
    }

    #[test]
    fn plan_all_routes_includes_ftl_on_lanes() {
        let ship = test_ship(100.0, 1.0);
        let current = uuid::Uuid::new_v4();
        let connections = vec![
            Connection {
                system_a: current,
                system_b: uuid::Uuid::new_v4(),
                distance_ly: 5.0,
                route_type: RouteType::Open,
            },
            Connection {
                system_a: current,
                system_b: uuid::Uuid::new_v4(),
                distance_ly: 8.0,
                route_type: RouteType::FtlLane,
            },
        ];

        let plans = plan_all_routes(&connections, &ship, current);

        // Open route: sublight only. FTL lane: sublight + FTL.
        assert_eq!(plans.len(), 3);
        let ftl_plans: Vec<_> = plans.iter().filter(|p| p.mode == TravelMode::Ftl).collect();
        assert_eq!(ftl_plans.len(), 1);
    }

    #[test]
    fn describe_plan_is_readable() {
        let ship = test_ship(100.0, 1.0);
        let conn = test_connection(8.0, RouteType::Open);
        let plan = plan_travel(&conn, &ship, TravelMode::Sublight, conn.system_a);

        let desc = describe_plan(&plan, "Cygnus Gate");
        assert!(desc.contains("Sublight"));
        assert!(desc.contains("Cygnus Gate"));
        assert!(desc.contains("months personal"));
        assert!(desc.contains("years galactic"));
    }
}