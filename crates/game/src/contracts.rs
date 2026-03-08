// file: crates/game/src/contracts.rs
//! Contract generation and progress tracking.
//!
//! Contracts are deterministically generated based on NPC identity,
//! faction category, available routes, and current galactic time.
//! Progress is checked automatically when the player arrives at
//! locations or docks at systems.

use uuid::Uuid;

use starbound_core::contract::{Contract, ContractState, ContractType};
use starbound_core::galaxy::*;
use starbound_core::journey::Journey;
use starbound_core::npc::Npc;

// ---------------------------------------------------------------------------
// Contract generation
// ---------------------------------------------------------------------------

/// Everything needed to generate a contract for an NPC.
pub struct ContractContext<'a> {
    pub npc: &'a Npc,
    pub systems: &'a [StarSystem],
    pub connections: &'a [Connection],
    pub factions: &'a [Faction],
    pub galactic_days: f64,
}

/// Generate a contract for an NPC based on their faction, location,
/// and the available routes from their home system.
///
/// Returns `None` if the NPC's home system has no connections (no
/// valid destination) or is otherwise ineligible.
///
/// Contract type is deterministically selected from the NPC's ID and
/// current galactic time (rotates on a 30-day window), weighted by
/// faction category.
pub fn generate_contract(ctx: &ContractContext) -> Option<Contract> {
    let npc = ctx.npc;
    let home_id = npc.home_system_id;

    // Find destination via connected systems.
    let connections: Vec<&Connection> = ctx.connections.iter()
        .filter(|c| c.system_a == home_id || c.system_b == home_id)
        .collect();

    if connections.is_empty() {
        return None;
    }

    // Pick a connected system as destination (deterministic from NPC id).
    let conn_idx = (npc.id.as_u128() as usize) % connections.len();
    let conn = connections[conn_idx];
    let dest_sys_id = if conn.system_a == home_id { conn.system_b } else { conn.system_a };

    let dest_sys_name = ctx.systems.iter()
        .find(|s| s.id == dest_sys_id)
        .map(|s| s.name.as_str())
        .unwrap_or("Unknown")
        .to_string();

    // Find the primary dockable location at the destination.
    let dest_system = ctx.systems.iter().find(|s| s.id == dest_sys_id);
    let dest_location = dest_system.and_then(|sys| {
        sys.locations.iter()
            .filter(|l| l.services.contains(&LocationService::Docking))
            .max_by_key(|l| l.infrastructure)
    });
    let dest_loc_id = dest_location.map(|l| l.id);
    let dest_loc_name = dest_location
        .map(|l| l.name.as_str())
        .unwrap_or(&dest_sys_name)
        .to_string();

    // Determine faction category.
    let category = npc.faction_id
        .and_then(|fid| ctx.factions.iter().find(|f| f.id == fid))
        .map(|f| f.category);

    // Deterministic type selection: hash NPC ID + galactic day (30-day window)
    // so the same NPC offers different types over time.
    let day_window = (ctx.galactic_days / 30.0) as u128;
    let type_seed = npc.id.as_u128().wrapping_add(day_window);

    // Each faction category has weighted type options.
    // (contract_type_index: 0=delivery, 1=retrieval, 2=investigation)
    let type_options: &[usize] = match category {
        Some(FactionCategory::Guild)     => &[0, 0, 1, 1, 2],
        Some(FactionCategory::Military)  => &[0, 2, 2, 2],
        Some(FactionCategory::Economic)  => &[0, 0, 0, 1],
        Some(FactionCategory::Criminal)  => &[0, 0, 2],
        Some(FactionCategory::Religious) => &[1, 1, 2],
        Some(FactionCategory::Academic)  => &[2, 2, 1],
        Some(FactionCategory::Political) => &[2, 2, 0],
        None                             => &[0, 0, 1],
    };

    let chosen_type = type_options[(type_seed as usize) % type_options.len()];

    let mut contract = match (chosen_type, category) {
        // ---- DELIVERY contracts ----
        (0, Some(FactionCategory::Guild)) => {
            Contract::delivery(
                npc.id, npc.faction_id,
                format!("Deliver repair components to {}", dest_loc_name),
                format!(
                    "\"We've got a maintenance backlog at {}. \
                     Standard repair components — nothing exotic, but they \
                     need them yesterday. Deliver, get the dock master to sign off, \
                     and come back for your pay.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Repair components", 8, 200.0,
            )
        }
        (0, Some(FactionCategory::Military)) => {
            Contract::delivery(
                npc.id, npc.faction_id,
                format!("Transport sealed cargo to {}", dest_loc_name),
                format!(
                    "\"Military business. Sealed containers, don't ask what's inside. \
                     Take them to {} garrison, hand them over, bring back the receipt. \
                     Standard courier rate.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Sealed military cargo", 5, 250.0,
            )
        }
        (0, Some(FactionCategory::Economic)) => {
            Contract::delivery(
                npc.id, npc.faction_id,
                format!("Supply run to {}", dest_loc_name),
                format!(
                    "\"The market at {} is running short on manufactured goods. \
                     We've got a shipment ready to go. Deliver it, collect payment \
                     on delivery, and bring back our cut.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Manufactured goods", 12, 180.0,
            )
        }
        (0, Some(FactionCategory::Criminal)) => {
            Contract::delivery(
                npc.id, npc.faction_id,
                format!("Discreet delivery to {}", dest_sys_name),
                format!(
                    "\"I've got a package. It needs to get to {} without anyone \
                     asking questions. No manifests, no declarations. \
                     You handle it clean, I make it worth your while.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, "Unmarked cargo", 3, 300.0,
            )
        }
        (0, _) => {
            Contract::delivery(
                npc.id, npc.faction_id,
                format!("Courier run to {}", dest_loc_name),
                format!(
                    "\"Standard job. Take this cargo to {}, hand it off, \
                     come back with confirmation. Simple work, fair pay.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "General cargo", 6, 175.0,
            )
        }

        // ---- RETRIEVAL contracts ----
        (1, Some(FactionCategory::Guild)) => {
            Contract::retrieval(
                npc.id, npc.faction_id,
                format!("Retrieve salvaged parts from {}", dest_loc_name),
                format!(
                    "\"There's a set of reclaimed drive components at {}. \
                     Paid for, just need someone to pick them up and bring \
                     them back here. Should be straightforward.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Reclaimed drive parts", 6, 220.0,
            )
        }
        (1, Some(FactionCategory::Religious)) => {
            Contract::retrieval(
                npc.id, npc.faction_id,
                format!("Recover relics from {}", dest_loc_name),
                format!(
                    "\"An artifact of the Order was left at {} during \
                     the last evacuation. We need it returned. \
                     You'll know it when you see it — it resonates.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Order relics", 2, 200.0,
            )
        }
        (1, Some(FactionCategory::Economic)) => {
            Contract::retrieval(
                npc.id, npc.faction_id,
                format!("Collect payment from {}", dest_loc_name),
                format!(
                    "\"We have an outstanding balance at {}. \
                     They've got our goods sitting in their hold. \
                     Go collect — here's the manifest.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Collected goods", 8, 190.0,
            )
        }
        (1, Some(FactionCategory::Academic)) => {
            Contract::retrieval(
                npc.id, npc.faction_id,
                format!("Retrieve research samples from {}", dest_loc_name),
                format!(
                    "\"Our field team at {} has samples ready for analysis. \
                     Delicate materials — keep them sealed. \
                     The data is more valuable than the containers.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Research samples", 3, 250.0,
            )
        }
        (1, _) => {
            Contract::retrieval(
                npc.id, npc.faction_id,
                format!("Pick up cargo from {}", dest_loc_name),
                format!(
                    "\"There's a shipment waiting for us at {}. \
                     Go get it, bring it back. I'll make it worth your time.\"",
                    dest_loc_name
                ),
                home_id, dest_sys_id, "Retrieved cargo", 5, 185.0,
            )
        }

        // ---- INVESTIGATION contracts ----
        (_, Some(FactionCategory::Military)) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Investigate activity near {}", dest_sys_name),
                format!(
                    "\"We've had reports of unusual activity in the {} system. \
                     Go there, assess the situation, and report back. \
                     Don't engage — just observe and document.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 280.0,
            )
        }
        (_, Some(FactionCategory::Academic)) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Survey anomalous readings at {}", dest_sys_name),
                format!(
                    "\"Our instruments have been picking up unusual readings \
                     from the {} system. We need someone on-site to \
                     confirm and characterize the source. Standard survey protocol.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 260.0,
            )
        }
        (_, Some(FactionCategory::Criminal)) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Scout {} for opportunities", dest_sys_name),
                format!(
                    "\"I need eyes at {}. Security patterns, docking schedules, \
                     who's coming and going. Routine business intelligence. \
                     Just look around and tell me what you see.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 300.0,
            )
        }
        (_, Some(FactionCategory::Political)) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Assess the political situation at {}", dest_sys_name),
                format!(
                    "\"There's been a shift in the local power balance at {}. \
                     I need an outside perspective — someone without ties. \
                     Go there, talk to people, and report back what you find.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 260.0,
            )
        }
        (_, Some(FactionCategory::Religious)) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Investigate temporal readings near {}", dest_sys_name),
                format!(
                    "\"The Order has detected temporal anomalies in the {} region. \
                     We need someone to visit and document what they experience. \
                     Pay attention to how time feels there.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 220.0,
            )
        }
        (_, _) => {
            Contract::investigation(
                npc.id, npc.faction_id,
                format!("Check on situation at {}", dest_sys_name),
                format!(
                    "\"I need someone to swing by {} and see what's going on. \
                     Nothing dangerous — just take a look around and let me know.\"",
                    dest_sys_name
                ),
                home_id, dest_sys_id, 200.0,
            )
        }
    };

    contract.destination_location_id = dest_loc_id;
    Some(contract)
}

// ---------------------------------------------------------------------------
// Contract progress tracking
// ---------------------------------------------------------------------------

/// Check whether any active contracts have been fulfilled at the player's
/// current location. Transitions fulfilled contracts to `ReadyToComplete`
/// and handles cargo delivery/retrieval.
///
/// Returns a list of human-readable progress messages for display.
pub fn check_contract_progress(
    journey: &mut Journey,
    current_system: Uuid,
    current_location: Option<Uuid>,
) -> Vec<String> {
    let mut messages: Vec<String> = Vec::new();

    for contract in &mut journey.active_contracts {
        if contract.state != ContractState::Active {
            continue;
        }

        match contract.contract_type {
            ContractType::Delivery => {
                if contract.destination_system_id != current_system {
                    continue;
                }
                if let Some(dest_loc) = contract.destination_location_id {
                    if current_location != Some(dest_loc) {
                        continue;
                    }
                }
                if let Some((ref cargo_name, qty)) = contract.cargo_required {
                    let held = journey.ship.cargo.get(cargo_name).copied().unwrap_or(0);
                    if held >= qty {
                        let remaining = held - qty;
                        if remaining == 0 {
                            journey.ship.cargo.remove(cargo_name);
                        } else {
                            journey.ship.cargo.insert(cargo_name.clone(), remaining);
                        }
                        contract.state = ContractState::ReadyToComplete;
                        messages.push(format!(
                            "Contract objective complete: {}. Delivered {} x{}. \
                             Return to the contract issuer to collect payment.",
                            contract.title, cargo_name, qty,
                        ));
                    }
                }
            }
            ContractType::Retrieval => {
                if contract.destination_system_id != current_system {
                    continue;
                }
                if let Some(dest_loc) = contract.destination_location_id {
                    if current_location != Some(dest_loc) {
                        continue;
                    }
                }
                if let Some((ref cargo_name, qty)) = contract.cargo_required {
                    let total_cargo: u32 = journey.ship.cargo.values().sum();
                    if total_cargo + qty > journey.ship.cargo_capacity {
                        messages.push(format!(
                            "You've located the {} for contract: {}, \
                             but your cargo hold is too full to take it. \
                             Free up {} units of cargo space.",
                            cargo_name, contract.title, qty,
                        ));
                        continue;
                    }
                    let current = journey.ship.cargo.get(cargo_name).copied().unwrap_or(0);
                    journey.ship.cargo.insert(cargo_name.clone(), current + qty);
                    contract.state = ContractState::ReadyToComplete;
                    messages.push(format!(
                        "Contract objective complete: {}. Retrieved {} x{}. \
                         Return to the contract issuer to collect payment.",
                        contract.title, cargo_name, qty,
                    ));
                } else {
                    contract.state = ContractState::ReadyToComplete;
                    messages.push(format!(
                        "Contract objective complete: {}. \
                         Return to the contract issuer to collect payment.",
                        contract.title,
                    ));
                }
            }
            ContractType::Investigation => {
                if contract.destination_system_id != current_system {
                    continue;
                }
                if let Some(dest_loc) = contract.destination_location_id {
                    if current_location != Some(dest_loc) {
                        continue;
                    }
                }
                contract.state = ContractState::ReadyToComplete;
                messages.push(format!(
                    "Investigation complete: {}. You've seen enough. \
                     Return to the contract issuer to report your findings.",
                    contract.title,
                ));
            }
        }
    }

    messages
}
