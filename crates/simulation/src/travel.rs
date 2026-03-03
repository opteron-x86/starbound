// file: crates/simulation/src/travel.rs
//! Travel calculations — FTL-default universe.
//!
//! Everyone has an FTL drive. Fuel is the constraint, not the
//! technology. Travel between systems takes days to weeks and
//! keeps personal/galactic time roughly in sync.
//!
//! Time dilation doesn't come from travel — it comes from *place*.
//! Systems near neutron stars, black holes, or anomalous phenomena
//! have a time_factor that determines how fast the outside galaxy
//! ages while you're there. Most settled systems are 1.0 (normal).
//! The strange places at the edge of known space are where time
//! gets expensive.
//!
//! Sublight is the emergency fallback — your drive is damaged,
//! you can't afford fuel. It's slow and has modest dilation,
//! but it's not the decades-per-hop of hard relativity. This is
//! a space opera, not a physics textbook.

use starbound_core::galaxy::{Connection, RouteType, StarSystem};
use starbound_core::ship::{Ship, TravelMode};
use starbound_core::time::Duration;

// ---------------------------------------------------------------------------
// FTL travel constants
// ---------------------------------------------------------------------------

/// FTL personal time: ~3 days per light-year.
/// A 5 ly hop ≈ 2 weeks. Feels like a road trip.
const FTL_PERSONAL_DAYS_PER_LY: f64 = 3.0;

/// FTL galactic time: ~4 days per light-year.
/// Slight drift — you arrive a few days "behind" the galaxy,
/// but nothing you'd notice unless you were counting closely.
const FTL_GALACTIC_DAYS_PER_LY: f64 = 4.0;

/// FTL fuel cost per light-year. The main constraint.
const FTL_FUEL_PER_LY: f32 = 3.0;

// ---------------------------------------------------------------------------
// Sublight (emergency) constants
// ---------------------------------------------------------------------------

/// Sublight personal time: ~30 days per light-year.
/// Slow — a 5 ly hop takes 5 months of your life.
const SUBLIGHT_PERSONAL_DAYS_PER_LY: f64 = 30.0;

/// Sublight galactic time: ~90 days per light-year.
/// Modest dilation — 3:1 ratio. A 5 ly hop costs ~15 months
/// to the galaxy. Noticeable but not civilization-ending.
const SUBLIGHT_GALACTIC_DAYS_PER_LY: f64 = 90.0;

// ---------------------------------------------------------------------------
// Route modifiers
// ---------------------------------------------------------------------------

/// Hazardous routes multiply travel time (detours, caution).
const HAZARDOUS_TIME_MULTIPLIER: f64 = 1.5;

/// Corridor routes are slightly faster (well-charted, optimized).
const CORRIDOR_TIME_MULTIPLIER: f64 = 0.8;

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
    /// FTL (normal) or Sublight (emergency).
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
        TravelMode::Ftl => (
            distance * FTL_PERSONAL_DAYS_PER_LY,
            distance * FTL_GALACTIC_DAYS_PER_LY,
            (distance as f32) * FTL_FUEL_PER_LY,
        ),
        TravelMode::Sublight => (
            distance * SUBLIGHT_PERSONAL_DAYS_PER_LY,
            distance * SUBLIGHT_GALACTIC_DAYS_PER_LY,
            0.0_f32,
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

    // Route modifiers.
    let route_multiplier = match connection.route_type {
        RouteType::Hazardous => HAZARDOUS_TIME_MULTIPLIER,
        RouteType::Corridor => CORRIDOR_TIME_MULTIPLIER,
        _ => 1.0,
    };

    // Degraded engine slows you down.
    let engine_multiplier = engine_condition_multiplier(ship.modules.engine.condition);

    let personal_days = base_personal * route_multiplier * engine_multiplier;
    let galactic_days = base_galactic * route_multiplier * engine_multiplier;
    let fuel_cost = base_fuel * engine_multiplier as f32;

    // Feasibility checks.
    let (feasible, infeasible_reason) = check_feasibility(ship, fuel_cost, mode);

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
///
/// FTL is the default. Sublight is offered as a fallback when the player
/// can't afford the FTL fuel cost for a particular route.
pub fn plan_all_routes(
    connections: &[Connection],
    ship: &Ship,
    current_system: uuid::Uuid,
) -> Vec<TravelPlan> {
    let mut plans = Vec::new();

    for conn in connections {
        let ftl_plan = plan_travel(conn, ship, TravelMode::Ftl, current_system);

        // Always show the FTL option (even if blocked — the player should
        // see what they can't afford).
        plans.push(ftl_plan.clone());

        // Offer sublight as a fallback if FTL is too expensive.
        if !ftl_plan.feasible {
            plans.push(plan_travel(conn, ship, TravelMode::Sublight, current_system));
        }
    }

    plans
}

// ---------------------------------------------------------------------------
// Time at system — where dilation actually lives
// ---------------------------------------------------------------------------

/// Calculate how much galactic time passes while the player spends
/// `personal_days` at a system with the given time factor.
///
/// At a normal system (factor 1.0), a day is a day.
/// At a distorted system (factor 5.0), one personal day = five galactic days.
///
/// Returns a Duration with the personal time unchanged and galactic
/// time scaled by the system's time factor.
pub fn time_at_system(personal_days: f64, system: &StarSystem) -> Duration {
    Duration {
        personal_days,
        galactic_days: personal_days * system.time_factor,
    }
}

/// Describe a system's time distortion for the player.
pub fn describe_time_factor(factor: f64) -> &'static str {
    if factor <= 1.0 {
        "Normal time"
    } else if factor <= 2.0 {
        "Mild temporal drift"
    } else if factor <= 5.0 {
        "Significant time distortion"
    } else if factor <= 15.0 {
        "Severe time distortion"
    } else if factor <= 50.0 {
        "Extreme temporal anomaly"
    } else {
        "Time is broken here"
    }
}

/// Short label for a system's time factor, suitable for map display.
pub fn time_factor_label(factor: f64) -> String {
    if (factor - 1.0).abs() < 0.01 {
        String::new() // No label for normal systems.
    } else if factor < 1.0 {
        format!("×{:.1} (slow)", factor)
    } else {
        format!("×{:.0}", factor)
    }
}

// ---------------------------------------------------------------------------
// Format for display
// ---------------------------------------------------------------------------

/// Format a travel plan as a human-readable summary (for the CLI).
pub fn describe_plan(plan: &TravelPlan, dest_name: &str) -> String {
    let mode_str = match plan.mode {
        TravelMode::Ftl => "FTL",
        TravelMode::Sublight => "Sublight (emergency)",
        TravelMode::Stationary => "Stationary",
    };

    let time_str = if plan.duration.galactic_days >= 365.0 {
        format!(
            "{:.1} months personal / {:.1} years galactic",
            plan.duration.personal_months(),
            plan.duration.galactic_years()
        )
    } else if plan.duration.personal_days >= 30.0 {
        format!(
            "{:.1} months personal / {:.1} months galactic",
            plan.duration.personal_months(),
            plan.duration.galactic_days / 30.44,
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
        " | no fuel".into()
    };

    let feasibility = if !plan.feasible {
        format!(" [{}]", plan.infeasible_reason.as_deref().unwrap_or("blocked"))
    } else {
        String::new()
    };

    format!(
        "{} → {} ({:.1} ly) — {}{}{}",
        mode_str,
        dest_name,
        plan.connection.distance_ly,
        time_str,
        fuel_str,
        feasibility,
    )
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn engine_condition_multiplier(condition: f32) -> f64 {
    if condition >= ENGINE_DEGRADATION_THRESHOLD {
        1.0
    } else if condition <= 0.0 {
        10.0
    } else {
        let t = condition / ENGINE_DEGRADATION_THRESHOLD;
        1.0 + (1.0 - t as f64) * 2.0
    }
}

fn check_feasibility(
    ship: &Ship,
    fuel_cost: f32,
    mode: TravelMode,
) -> (bool, Option<String>) {
    if ship.modules.engine.condition <= 0.0 {
        return (false, Some("Engine is non-functional.".into()));
    }

    if mode == TravelMode::Ftl && fuel_cost > ship.fuel {
        return (
            false,
            Some(format!(
                "Insufficient fuel: need {:.1}, have {:.1}",
                fuel_cost, ship.fuel
            )),
        );
    }

    (true, None)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

    fn test_system(time_factor: f64) -> StarSystem {
        StarSystem {
            id: uuid::Uuid::new_v4(),
            name: "Test System".into(),
            position: (0.0, 0.0),
            star_type: starbound_core::galaxy::StarType::YellowDwarf,
            planetary_bodies: vec![],
            controlling_civ: None,
            infrastructure_level: starbound_core::galaxy::InfrastructureLevel::Colony,
            history: vec![],
            active_threads: vec![],
            time_factor,
            faction_presence: vec![],
        }
    }

    // -- FTL travel --

    #[test]
    fn ftl_is_fast_and_roughly_in_sync() {
        let ship = test_ship(100.0, 1.0);
        let conn = test_connection(5.0, RouteType::Open);
        let plan = plan_travel(&conn, &ship, TravelMode::Ftl, conn.system_a);

        assert!(plan.feasible);

        // 5 ly × 3 days/ly = 15 days personal
        assert!((plan.duration.personal_days - 15.0).abs() < 1.0,
            "Expected ~15 personal days, got {:.1}", plan.duration.personal_days);

        // 5 ly × 4 days/ly = 20 days galactic
        assert!((plan.duration.galactic_days - 20.0).abs() < 1.0,
            "Expected ~20 galactic days, got {:.1}", plan.duration.galactic_days);

        // Ratio should be close to 1.0
        let ratio = plan.duration.galactic_days / plan.duration.personal_days;
        assert!(ratio < 2.0, "FTL should keep times roughly in sync, ratio was {:.2}", ratio);
    }

    #[test]
    fn ftl_costs_fuel() {
        let ship = test_ship(100.0, 1.0);
        let conn = test_connection(5.0, RouteType::Open);
        let plan = plan_travel(&conn, &ship, TravelMode::Ftl, conn.system_a);

        // 5 ly × 3 fuel/ly = 15 fuel
        assert!((plan.fuel_cost - 15.0).abs() < 0.1);
    }

    #[test]
    fn insufficient_fuel_blocks_ftl() {
        let ship = test_ship(5.0, 1.0);
        let conn = test_connection(5.0, RouteType::Open);
        let plan = plan_travel(&conn, &ship, TravelMode::Ftl, conn.system_a);

        assert!(!plan.feasible);
        assert!(plan.infeasible_reason.as_ref().unwrap().contains("fuel"));
    }

    // -- Sublight (emergency) --

    #[test]
    fn sublight_is_slow_but_free() {
        let ship = test_ship(0.0, 1.0); // No fuel at all
        let conn = test_connection(5.0, RouteType::Open);
        let plan = plan_travel(&conn, &ship, TravelMode::Sublight, conn.system_a);

        assert!(plan.feasible);
        assert_eq!(plan.fuel_cost, 0.0);

        // 5 ly × 30 days/ly = 150 days (~5 months)
        assert!((plan.duration.personal_days - 150.0).abs() < 5.0,
            "Expected ~150 personal days, got {:.0}", plan.duration.personal_days);

        // 5 ly × 90 days/ly = 450 days (~15 months)
        assert!((plan.duration.galactic_days - 450.0).abs() < 10.0,
            "Expected ~450 galactic days, got {:.0}", plan.duration.galactic_days);

        // Modest dilation — 3:1, not 80:1
        let ratio = plan.duration.galactic_days / plan.duration.personal_days;
        assert!(ratio > 2.5 && ratio < 3.5,
            "Sublight should have ~3:1 dilation, got {:.1}", ratio);
    }

    // -- Route modifiers --

    #[test]
    fn hazardous_routes_take_longer() {
        let ship = test_ship(100.0, 1.0);
        let normal = test_connection(5.0, RouteType::Open);
        let hazardous = test_connection(5.0, RouteType::Hazardous);

        let plan_normal = plan_travel(&normal, &ship, TravelMode::Ftl, normal.system_a);
        let plan_hazard = plan_travel(&hazardous, &ship, TravelMode::Ftl, hazardous.system_a);

        assert!(plan_hazard.duration.personal_days > plan_normal.duration.personal_days);
    }

    #[test]
    fn corridors_are_faster() {
        let ship = test_ship(100.0, 1.0);
        let open = test_connection(5.0, RouteType::Open);
        let corridor = test_connection(5.0, RouteType::Corridor);

        let plan_open = plan_travel(&open, &ship, TravelMode::Ftl, open.system_a);
        let plan_corridor = plan_travel(&corridor, &ship, TravelMode::Ftl, corridor.system_a);

        assert!(plan_corridor.duration.personal_days < plan_open.duration.personal_days);
    }

    #[test]
    fn damaged_engine_slows_travel() {
        let good_ship = test_ship(100.0, 1.0);
        let bad_ship = test_ship(100.0, 0.25);
        let conn = test_connection(5.0, RouteType::Open);

        let plan_good = plan_travel(&conn, &good_ship, TravelMode::Ftl, conn.system_a);
        let plan_bad = plan_travel(&conn, &bad_ship, TravelMode::Ftl, conn.system_a);

        assert!(plan_bad.duration.personal_days > plan_good.duration.personal_days);
    }

    #[test]
    fn dead_engine_blocks_travel() {
        let ship = test_ship(100.0, 0.0);
        let conn = test_connection(5.0, RouteType::Open);
        let plan = plan_travel(&conn, &ship, TravelMode::Ftl, conn.system_a);

        assert!(!plan.feasible);
    }

    // -- Route planning --

    #[test]
    fn plan_all_routes_offers_sublight_fallback() {
        let ship = test_ship(5.0, 1.0); // Low fuel
        let current = uuid::Uuid::new_v4();
        let connections = vec![
            Connection {
                system_a: current,
                system_b: uuid::Uuid::new_v4(),
                distance_ly: 2.0, // Affordable: 6 fuel
                route_type: RouteType::Open,
            },
            Connection {
                system_a: current,
                system_b: uuid::Uuid::new_v4(),
                distance_ly: 8.0, // Too expensive: 24 fuel
                route_type: RouteType::Open,
            },
        ];

        let plans = plan_all_routes(&connections, &ship, current);

        // Short route: FTL only (feasible, 6 > 5 — actually blocked too).
        // Long route: FTL (blocked) + sublight fallback.
        // Both FTL plans are shown; sublight offered where FTL fails.
        let ftl_plans: Vec<_> = plans.iter().filter(|p| p.mode == TravelMode::Ftl).collect();
        let sublight_plans: Vec<_> = plans.iter().filter(|p| p.mode == TravelMode::Sublight).collect();

        assert_eq!(ftl_plans.len(), 2, "Both FTL options shown");
        assert!(sublight_plans.len() >= 1, "Sublight fallback offered for expensive routes");
    }

    // -- Time at system --

    #[test]
    fn normal_system_no_dilation() {
        let system = test_system(1.0);
        let duration = time_at_system(10.0, &system);

        assert_eq!(duration.personal_days, 10.0);
        assert_eq!(duration.galactic_days, 10.0);
    }

    #[test]
    fn neutron_star_mild_dilation() {
        let system = test_system(3.0);
        let duration = time_at_system(10.0, &system);

        assert_eq!(duration.personal_days, 10.0);
        assert_eq!(duration.galactic_days, 30.0);
    }

    #[test]
    fn black_hole_extreme_dilation() {
        let system = test_system(50.0);
        let duration = time_at_system(7.0, &system);

        assert_eq!(duration.personal_days, 7.0);
        assert_eq!(duration.galactic_days, 350.0);
        // A week near a black hole = nearly a year elsewhere.
    }

    #[test]
    fn time_factor_descriptions() {
        assert_eq!(describe_time_factor(1.0), "Normal time");
        assert_eq!(describe_time_factor(1.5), "Mild temporal drift");
        assert_eq!(describe_time_factor(3.0), "Significant time distortion");
        assert_eq!(describe_time_factor(10.0), "Severe time distortion");
        assert_eq!(describe_time_factor(30.0), "Extreme temporal anomaly");
        assert_eq!(describe_time_factor(100.0), "Time is broken here");
    }

    #[test]
    fn time_factor_labels() {
        assert_eq!(time_factor_label(1.0), "");
        assert_eq!(time_factor_label(3.0), "×3");
        assert_eq!(time_factor_label(0.5), "×0.5 (slow)");
    }

    #[test]
    fn describe_plan_readable() {
        let ship = test_ship(100.0, 1.0);
        let conn = test_connection(5.0, RouteType::Open);
        let plan = plan_travel(&conn, &ship, TravelMode::Ftl, conn.system_a);

        let desc = describe_plan(&plan, "Cygnus Gate");
        assert!(desc.contains("FTL"));
        assert!(desc.contains("Cygnus Gate"));
        assert!(desc.contains("days"));
    }
}