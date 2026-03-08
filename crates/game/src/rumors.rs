// file: crates/game/src/rumors.rs
//! Rumor generation — assembles actionable information from live game state.
//!
//! Rumors are not pre-authored content. They are assembled at runtime from
//! game state when the player selects "Gather Rumors" at a location with
//! the Rumors service.
//!
//! Four scanners produce candidates:
//!   - Trade scanner: price differentials across known economies
//!   - Contract scanner: NPC-offered work based on faction presence
//!   - Faction scanner: recent galactic tick events
//!   - Thread scanner: unresolved narrative threads and potential seeds
//!
//! Candidates are scored, deduplicated by category, and the top 2-4 are
//! selected based on infrastructure level.

use rand::prelude::*;
use uuid::Uuid;

use starbound_core::galaxy::{
    BodyType, FactionCategory, InfrastructureLevel, Location, LocationType, StarSystem,
    StarType, TradeGood,
};
use starbound_core::journey::Journey;
use starbound_core::narrative::{ResolutionState, ThreadType};
use starbound_core::npc::Npc;
use starbound_core::rumor::{
    base_reliability, rumor_count_range, Rumor, RumorCategory, RumorContent,
};

use starbound_simulation::generate::GeneratedGalaxy;
use starbound_simulation::tick::TickEvent;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Context needed to generate rumors at the player's current location.
pub struct RumorContext<'a> {
    pub galaxy: &'a GeneratedGalaxy,
    pub journey: &'a Journey,
    /// Recent galactic tick events (kept in CLI GameState).
    pub recent_tick_events: &'a [TickEvent],
    /// Current location — must have the Rumors service.
    pub location: &'a Location,
    /// The system the player is in.
    pub system: &'a StarSystem,
}

/// Generate rumors at the player's current location.
///
/// Returns 1-4 rumors selected from all scanner candidates,
/// scored by relevance and variety-balanced across categories.
pub fn generate_rumors(ctx: &RumorContext, rng: &mut StdRng) -> Vec<Rumor> {
    let (min_count, max_count) = rumor_count_range(ctx.location.infrastructure);
    let target_count = rng.gen_range(min_count..=max_count);
    let reliability = base_reliability(ctx.location.infrastructure);

    // Gather candidates from all scanners.
    let mut candidates: Vec<ScoredCandidate> = Vec::new();

    candidates.extend(scan_trade(ctx, reliability));
    candidates.extend(scan_contracts(ctx, reliability));
    candidates.extend(scan_factions(ctx, reliability));
    candidates.extend(scan_threads(ctx, reliability));
    candidates.extend(scan_local_color(ctx, reliability));

    if candidates.is_empty() {
        return vec![];
    }

    // Sort by score (descending).
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    // Select with variety: at most one per category.
    let mut selected: Vec<ScoredCandidate> = Vec::new();
    let mut used_categories: Vec<RumorCategory> = Vec::new();
    let mut selected_ids: Vec<Uuid> = Vec::new();

    // First pass: one per category (highest-scoring).
    for candidate in &candidates {
        if selected.len() >= target_count {
            break;
        }
        if used_categories.contains(&candidate.category) {
            continue;
        }
        used_categories.push(candidate.category);
        selected_ids.push(candidate.id);
        selected.push(candidate.clone());
    }

    // Second pass: fill remaining slots with duplicates if needed.
    if selected.len() < target_count {
        for candidate in &candidates {
            if selected.len() >= target_count {
                break;
            }
            if selected_ids.contains(&candidate.id) {
                continue;
            }
            selected_ids.push(candidate.id);
            selected.push(candidate.clone());
        }
    }

    // Convert to Rumor structs.
    selected
        .into_iter()
        .map(|c| c.into_rumor(ctx))
        .collect()
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

/// A candidate rumor with a relevance score for selection.
#[derive(Clone)]
struct ScoredCandidate {
    id: Uuid,
    category: RumorCategory,
    content: RumorContent,
    display_text: String,
    summary: String,
    score: f64,
    reliability: f64,
    expires_in: f64,
}

impl ScoredCandidate {
    fn into_rumor(self, ctx: &RumorContext) -> Rumor {
        Rumor {
            id: self.id,
            category: self.category,
            content: self.content,
            source_system: ctx.system.id,
            source_location: ctx.location.id,
            generated_at: ctx.journey.time.galactic_days,
            expires_in: self.expires_in,
            reliability: self.reliability,
            acted_on: false,
            outcome: None,
            display_text: self.display_text,
            summary: self.summary,
        }
    }
}

// ---------------------------------------------------------------------------
// Trade scanner
// ---------------------------------------------------------------------------

/// Scan all known economies for profitable trade routes relative to the
/// player's current location.
fn scan_trade(ctx: &RumorContext, reliability: f64) -> Vec<ScoredCandidate> {
    let local_economy = match &ctx.location.economy {
        Some(e) => e,
        None => return vec![],
    };

    let mut candidates = Vec::new();

    for good in TradeGood::all() {
        let local_buy = local_economy.buy_price(*good);

        // Check every location in every system for sell opportunities.
        for system in &ctx.galaxy.systems {
            if system.id == ctx.system.id {
                continue; // Skip current system.
            }
            for loc in &system.locations {
                let other_economy = match &loc.economy {
                    Some(e) => e,
                    None => continue,
                };
                let sell_there = other_economy.sell_price(*good);
                let spread = sell_there - local_buy;

                // Only surface profitable spreads above a minimum threshold.
                if spread < 3.0 {
                    continue;
                }

                let display = format!(
                    "\"{}\" is selling for {:.0} credits at {} — you can buy it here for {:.0}. \
                     That's roughly {:.0} per unit profit.\"",
                    good.display_name(),
                    sell_there,
                    system.name,
                    local_buy,
                    spread,
                );

                let summary = format!(
                    "{}: buy here ~{:.0}, sell at {} ~{:.0} (+{:.0}/unit)",
                    good.display_name(),
                    local_buy,
                    system.name,
                    sell_there,
                    spread,
                );

                // Score: higher spread = more relevant. Normalize by base price.
                let score = spread / good.base_price();

                candidates.push(ScoredCandidate {
                    id: Uuid::new_v4(),
                    category: RumorCategory::TradeTip,
                    content: RumorContent::TradeTip {
                        good: good.display_name().to_string(),
                        buy_system: ctx.system.id,
                        buy_location: Some(ctx.location.id),
                        sell_system: system.id,
                        sell_location: Some(loc.id),
                        estimated_spread: spread,
                        estimated_sell_price: sell_there,
                    },
                    display_text: display,
                    summary,
                    score,
                    reliability,
                    expires_in: RumorCategory::TradeTip.default_expiry(),
                });
            }
        }
    }

    // Keep only the top 3 trade tips (don't flood with trade data).
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(3);
    candidates
}

// ---------------------------------------------------------------------------
// Contract scanner
// ---------------------------------------------------------------------------

/// Scan NPCs at the current location for potential contract offerings.
/// Generates ContractLead rumors that reference specific NPCs by name,
/// with contract types varied by faction category.
fn scan_contracts(ctx: &RumorContext, reliability: f64) -> Vec<ScoredCandidate> {
    let mut candidates = Vec::new();

    let loc_id = match ctx.journey.current_location {
        Some(id) => id,
        None => return candidates, // Not docked — no NPCs to hear about.
    };

    // Find NPCs at this location who could offer work.
    let npcs_here: Vec<&Npc> = ctx.galaxy.npcs.iter()
        .filter(|n| {
            n.home_system_id == ctx.system.id
                && n.alive
                && n.home_location_id == Some(loc_id)
                && n.will_offer_contracts()
        })
        .collect();

    // Check if the player already has contracts from these NPCs (skip them).
    let active_issuers: Vec<Uuid> = ctx.journey.active_contracts.iter()
        .filter(|c| c.state == starbound_core::contract::ContractState::Active
            || c.state == starbound_core::contract::ContractState::ReadyToComplete)
        .map(|c| c.issuer_npc_id)
        .collect();

    for npc in npcs_here {
        if active_issuers.contains(&npc.id) {
            continue; // Already working for this NPC.
        }

        let faction_category = npc.faction_id
            .and_then(|fid| {
                ctx.galaxy.factions.iter()
                    .find(|f| f.id == fid)
                    .map(|f| f.category)
            });

        let faction_name = npc.faction_id
            .and_then(|fid| {
                ctx.galaxy.factions.iter()
                    .find(|f| f.id == fid)
                    .map(|f| f.name.as_str())
            })
            .unwrap_or("an independent operator");

        // Pick a contract type and flavor text based on faction category.
        let (contract_type, verb, reward_estimate) = match faction_category {
            Some(FactionCategory::Military) => {
                ("investigation", "investigate a situation", 280.0)
            }
            Some(FactionCategory::Economic) => {
                ("delivery", "run a shipment", 200.0)
            }
            Some(FactionCategory::Guild) => {
                ("retrieval", "retrieve some equipment", 220.0)
            }
            Some(FactionCategory::Criminal) => {
                ("delivery", "move some cargo discreetly", 320.0)
            }
            Some(FactionCategory::Religious) => {
                ("retrieval", "recover an artifact", 180.0)
            }
            Some(FactionCategory::Academic) => {
                ("investigation", "look into something", 240.0)
            }
            Some(FactionCategory::Political) => {
                ("investigation", "assess a situation", 260.0)
            }
            None => {
                ("delivery", "handle a courier job", 175.0)
            }
        };

        let npc_display = npc.display_name();

        let display = if npc.met_player {
            format!(
                "\"{} — {} — is looking for someone to {}. \
                 Talk to {} if you're interested.\"",
                npc.name, npc.title, verb,
                npc.pronouns.object,
            )
        } else {
            format!(
                "\"The {} is looking for someone to {}. \
                 Talk to {} if you're interested.\"",
                npc.title, verb,
                npc.pronouns.object,
            )
        };

        let summary = format!(
            "{} ({}) wants a {} job done (~{:.0} cr)",
            npc_display, faction_name, contract_type, reward_estimate,
        );

        // Score: contract leads are generally valuable. Boost slightly
        // for NPCs with better disposition (better terms likely).
        let disposition_bonus = (npc.disposition.max(0.0) as f64) * 0.2;
        let score = 0.7 + disposition_bonus;

        candidates.push(ScoredCandidate {
            id: Uuid::new_v4(),
            category: RumorCategory::ContractLead,
            content: RumorContent::ContractLead {
                faction_id: npc.faction_id.unwrap_or(Uuid::nil()),
                contract_type: contract_type.into(),
                destination_system: None, // Specific destination determined at offer time.
                estimated_reward: reward_estimate,
                npc_id: Some(npc.id),
                npc_name: if npc.met_player { Some(npc.name.clone()) } else { None },
            },
            display_text: display,
            summary,
            score,
            reliability, // Contract leads are as reliable as the location.
            expires_in: RumorCategory::ContractLead.default_expiry(),
        });
    }

    // Keep top 2 (don't overwhelm with contract leads).
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(2);
    candidates
}

// ---------------------------------------------------------------------------
// Faction scanner
// ---------------------------------------------------------------------------

/// Surface political, military, and diplomatic intelligence from both recent
/// tick events and live galaxy state — civ stability, inter-civ tensions,
/// internal pressures, militarization, and faction power shifts at neighbors.
fn scan_factions(ctx: &RumorContext, reliability: f64) -> Vec<ScoredCandidate> {
    let mut candidates = Vec::new();

    // Factions present at this system.
    let local_faction_ids: Vec<Uuid> = ctx.system.faction_presence.iter()
        .map(|fp| fp.faction_id)
        .collect();

    // ----- Tick event rumors (recent galactic history) -----

    for event in ctx.recent_tick_events {
        let involves_local = event.entities.iter()
            .any(|eid| local_faction_ids.contains(eid));

        if !involves_local && ctx.recent_tick_events.len() > 5 {
            continue;
        }

        let faction_names: Vec<String> = resolve_entity_names(
            &event.entities, &ctx.galaxy.factions, &ctx.galaxy.civilizations,
        );

        let display = format!("\"{}\"", event.description);
        let summary = event.description.clone();
        let score = if involves_local { 0.8 } else { 0.4 };

        candidates.push(ScoredCandidate {
            id: Uuid::new_v4(),
            category: RumorCategory::FactionIntel,
            content: RumorContent::FactionIntel {
                summary: event.description.clone(),
                factions_involved: event.entities.clone(),
                implication: if involves_local {
                    format!(
                        "This directly affects {} presence at {}.",
                        faction_names.join(" and "),
                        ctx.system.name,
                    )
                } else {
                    "Distant events, but the ripples may reach here.".into()
                },
            },
            display_text: display,
            summary,
            score,
            reliability,
            expires_in: RumorCategory::FactionIntel.default_expiry(),
        });
    }

    // ----- Live galaxy state: controlling civilization -----

    if let Some(civ_id) = ctx.system.controlling_civ {
        if let Some(civ) = ctx.galaxy.civilizations.iter().find(|c| c.id == civ_id) {
            // Low stability — the population is nervous.
            if civ.internal_dynamics.stability < 0.4 {
                let severity = if civ.internal_dynamics.stability < 0.2 {
                    "on the verge of something ugly"
                } else {
                    "not stable — people are nervous"
                };
                let display = format!(
                    "\"Things under {} rule are {}. \
                     You can feel it in the docking bay.\"",
                    civ.name, severity,
                );
                let summary = format!(
                    "{}: stability low ({:.0}%)",
                    civ.name, civ.internal_dynamics.stability * 100.0,
                );
                candidates.push(faction_intel_candidate(
                    format!("{} internal stability deteriorating", civ.name),
                    vec![civ_id],
                    "Instability means unpredictability — and opportunity.".into(),
                    display, summary, 0.75, reliability,
                ));
            }

            // Active internal pressures — specific unrest.
            if let Some(pressure) = civ.internal_dynamics.pressures.first() {
                let display = format!(
                    "\"Word is {} is dealing with problems. {}\"",
                    civ.name, pressure.description,
                );
                let summary = format!("{}: {}", civ.name, pressure.description);
                candidates.push(faction_intel_candidate(
                    format!("{}: {}", civ.name, pressure.description),
                    vec![civ_id],
                    "Internal problems spill outward sooner or later.".into(),
                    display, summary, 0.65, reliability,
                ));
            }

            // Inter-civ relationships: military tension or diplomatic warming.
            for (other_id, disp) in &civ.relationships {
                let other = match ctx.galaxy.civilizations.iter().find(|c| c.id == *other_id) {
                    Some(c) => c,
                    None => continue,
                };

                if disp.military < -0.3 {
                    let (severity, implication) = if disp.military < -0.7 {
                        (
                            "on the brink of open conflict",
                            "Border systems will become dangerous. Plan routes carefully.",
                        )
                    } else {
                        (
                            "at each other's throats politically",
                            "Military traffic is up. Inspections may get thorough.",
                        )
                    };
                    let display = format!(
                        "\"{} and {} are {}. Military traffic through here has picked up.\"",
                        civ.name, other.name, severity,
                    );
                    let summary = format!(
                        "Military tension: {} vs {} ({:.0}%)",
                        civ.name, other.name, -disp.military * 100.0,
                    );
                    candidates.push(faction_intel_candidate(
                        format!("Military tension between {} and {}", civ.name, other.name),
                        vec![civ_id, *other_id],
                        implication.into(),
                        display, summary, 0.85, reliability,
                    ));
                } else if disp.diplomatic > 0.5 && disp.economic > 0.4 {
                    let display = format!(
                        "\"{} and {} are getting friendly. Trade agreements, joint patrols — \
                         good for business if you work both sides.\"",
                        civ.name, other.name,
                    );
                    let summary = format!(
                        "Warming relations: {} + {}",
                        civ.name, other.name,
                    );
                    candidates.push(faction_intel_candidate(
                        format!("{} and {} strengthening ties", civ.name, other.name),
                        vec![civ_id, *other_id],
                        "Alliance means smoother trade routes through both territories.".into(),
                        display, summary, 0.5, reliability,
                    ));
                }
            }

            // Militarization — civ is arming up.
            if civ.capabilities.military > 0.7 {
                let display = format!(
                    "\"{} has been building up military strength. Shipyards running \
                     double shifts, new patrol routes going in.\"",
                    civ.name,
                );
                let summary = format!(
                    "{}: military buildup ({:.0}%)",
                    civ.name, civ.capabilities.military * 100.0,
                );
                candidates.push(faction_intel_candidate(
                    format!("{} expanding military capabilities", civ.name),
                    vec![civ_id],
                    "Militarization usually means someone expects a fight.".into(),
                    display, summary, 0.6, reliability,
                ));
            }
        }
    }

    // ----- Live galaxy state: neighboring systems -----

    let neighbor_ids: Vec<Uuid> = ctx.galaxy.connections.iter()
        .filter_map(|c| {
            if c.system_a == ctx.system.id { Some(c.system_b) }
            else if c.system_b == ctx.system.id { Some(c.system_a) }
            else { None }
        })
        .collect();

    for neighbor_id in &neighbor_ids {
        let neighbor = match ctx.galaxy.systems.iter().find(|s| s.id == *neighbor_id) {
            Some(s) => s,
            None => continue,
        };

        // Dominant faction consolidating power at a neighbor.
        if let Some(dominant) = neighbor.faction_presence.iter()
            .max_by(|a, b| a.strength.partial_cmp(&b.strength).unwrap_or(std::cmp::Ordering::Equal))
        {
            if dominant.strength > 0.7 {
                if let Some(faction) = ctx.galaxy.factions.iter().find(|f| f.id == dominant.faction_id) {
                    let display = format!(
                        "\"The {} practically run {} now. \
                         Anyone doing business there works on their terms.\"",
                        faction.name, neighbor.name,
                    );
                    let summary = format!(
                        "{} dominant at {} ({:.0}%)",
                        faction.name, neighbor.name, dominant.strength * 100.0,
                    );
                    candidates.push(faction_intel_candidate(
                        format!("{} consolidating power at {}", faction.name, neighbor.name),
                        vec![faction.id],
                        format!("Expect {} rules to shape conditions at {}.", faction.name, neighbor.name),
                        display, summary, 0.55, reliability * 0.9,
                    ));
                }
            }
        }

        // Three or more factions contesting a neighbor — powder keg.
        let strong_here: Vec<Uuid> = neighbor.faction_presence.iter()
            .filter(|fp| fp.strength >= 0.3)
            .map(|fp| fp.faction_id)
            .collect();

        if strong_here.len() >= 3 {
            let names: Vec<String> = strong_here.iter()
                .filter_map(|fid| {
                    ctx.galaxy.factions.iter()
                        .find(|f| f.id == *fid)
                        .map(|f| f.name.clone())
                })
                .collect();

            if !names.is_empty() {
                let display = format!(
                    "\"{} is a powder keg — {} all jockeying for position there.\"",
                    neighbor.name, names.join(", "),
                );
                let summary = format!(
                    "{}: {} factions competing",
                    neighbor.name, strong_here.len(),
                );
                candidates.push(faction_intel_candidate(
                    format!("Power struggle at {}", neighbor.name),
                    strong_here,
                    "Contested systems are volatile — and profitable for the right captain.".into(),
                    display, summary, 0.6, reliability * 0.85,
                ));
            }
        }
    }

    // Keep top 4 — more material now, allow slightly deeper pool.
    candidates.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(4);
    candidates
}

/// Helper: build a FactionIntel candidate without repeating boilerplate.
fn faction_intel_candidate(
    content_summary: String,
    factions_involved: Vec<Uuid>,
    implication: String,
    display_text: String,
    summary: String,
    score: f64,
    reliability: f64,
) -> ScoredCandidate {
    ScoredCandidate {
        id: Uuid::new_v4(),
        category: RumorCategory::FactionIntel,
        content: RumorContent::FactionIntel {
            summary: content_summary,
            factions_involved,
            implication,
        },
        display_text,
        summary,
        score,
        reliability,
        expires_in: RumorCategory::FactionIntel.default_expiry(),
    }
}

/// Resolve entity UUIDs to display names, checking factions then civilizations.
fn resolve_entity_names(
    ids: &[Uuid],
    factions: &[starbound_core::galaxy::Faction],
    civs: &[starbound_core::galaxy::Civilization],
) -> Vec<String> {
    ids.iter()
        .filter_map(|eid| {
            factions.iter()
                .find(|f| f.id == *eid)
                .map(|f| f.name.clone())
                .or_else(|| civs.iter().find(|c| c.id == *eid).map(|c| c.name.clone()))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Thread scanner
// ---------------------------------------------------------------------------

/// Read the player's thread ledger and surface hints for unresolved threads,
/// plus potential new thread seeds from galactic state.
fn scan_threads(ctx: &RumorContext, reliability: f64) -> Vec<ScoredCandidate> {
    let mut candidates = Vec::new();

    // Existing open threads with high tension.
    for thread in &ctx.journey.threads {
        if thread.resolution != ResolutionState::Open {
            continue;
        }
        if thread.tension < 0.3 {
            continue;
        }

        // Check if the thread connects to this system.
        let connects_here = thread.associated_entities.contains(&ctx.system.id);

        let display = format!(
            "\"People are still talking about {}. {}\"",
            thread.description,
            if connects_here {
                "It happened right here."
            } else {
                "Word travels."
            },
        );

        let summary = format!("Thread: {} (tension {:.0}%)", thread.description, thread.tension * 100.0);

        let score = thread.tension as f64 * if connects_here { 1.5 } else { 0.8 };

        candidates.push(ScoredCandidate {
            id: Uuid::new_v4(),
            category: RumorCategory::ThreadSeed,
            content: RumorContent::ThreadSeed {
                description: thread.description.clone(),
                related_system: if connects_here { Some(ctx.system.id) } else { None },
                thread_type: format!("{}", thread.thread_type),
            },
            display_text: display,
            summary,
            score,
            reliability: 1.0, // Threads are facts about the player's own story.
            expires_in: RumorCategory::ThreadSeed.default_expiry(),
        });
    }

    // Generate a potential new thread seed from galactic state.
    // Look for systems with high faction tension nearby.
    for system in &ctx.galaxy.systems {
        if system.id == ctx.system.id {
            continue;
        }
        if system.faction_presence.len() < 2 {
            continue;
        }

        // Check for contested systems (multiple factions with significant strength).
        let strong_factions: Vec<&Uuid> = system.faction_presence.iter()
            .filter(|fp| fp.strength >= 0.3)
            .map(|fp| &fp.faction_id)
            .collect();

        if strong_factions.len() >= 2 {
            let faction_names: Vec<String> = strong_factions.iter()
                .filter_map(|fid| {
                    ctx.galaxy.factions.iter()
                        .find(|f| f.id == **fid)
                        .map(|f| f.name.clone())
                })
                .collect();

            // Only emit if the player doesn't already have a thread about this.
            let already_tracked = ctx.journey.threads.iter().any(|t| {
                t.associated_entities.contains(&system.id)
                    && t.resolution == ResolutionState::Open
            });
            if already_tracked {
                continue;
            }

            let faction_list = if faction_names.len() == 2 {
                format!("{} and {}", faction_names[0], faction_names[1])
            } else {
                // "A, B, and C"
                let last = faction_names.last().unwrap().clone();
                let rest = &faction_names[..faction_names.len() - 1];
                format!("{}, and {}", rest.join(", "), last)
            };

            let display = format!(
                "\"Things are tense at {}. {} are all vying for influence there.\"",
                system.name,
                faction_list,
            );

            let summary = format!(
                "Contested: {} ({} competing factions)",
                system.name,
                strong_factions.len(),
            );

            candidates.push(ScoredCandidate {
                id: Uuid::new_v4(),
                category: RumorCategory::ThreadSeed,
                content: RumorContent::ThreadSeed {
                    description: format!(
                        "Power struggle at {} between {}",
                        system.name,
                        faction_list,
                    ),
                    related_system: Some(system.id),
                    thread_type: format!("{}", ThreadType::Mystery),
                },
                display_text: display,
                summary,
                score: 0.6,
                reliability,
                expires_in: RumorCategory::ThreadSeed.default_expiry(),
            });
        }
    }

    candidates.truncate(3);
    candidates
}

// ---------------------------------------------------------------------------
// Local color scanner
// ---------------------------------------------------------------------------

/// Generate atmospheric details from the system, location, ship, and faction
/// state. No mechanical payload — pure world texture that makes a spaceport
/// feel like a place, not a menu.
fn scan_local_color(ctx: &RumorContext, reliability: f64) -> Vec<ScoredCandidate> {
    let mut candidates = Vec::new();

    // ----- Time distortion (preserved from original) -----

    if ctx.system.time_factor >= 1.5 {
        let display = if ctx.system.time_factor >= 8.0 {
            format!(
                "\"Don't linger here. Clocks run ×{:.0} — a week at {} \
                 costs you months outside.\"",
                ctx.system.time_factor, ctx.system.name,
            )
        } else {
            format!(
                "\"Time runs a bit thick at {}. ×{:.1} — \
                 nothing dramatic, but it adds up.\"",
                ctx.system.name, ctx.system.time_factor,
            )
        };

        let summary = format!(
            "{}: time factor ×{:.1}",
            ctx.system.name, ctx.system.time_factor,
        );

        candidates.push(local_color_candidate(
            display, summary, 0.3,
        ));
    }

    // ----- Low infrastructure (preserved from original) -----

    if ctx.location.infrastructure <= InfrastructureLevel::Outpost {
        let display = format!(
            "\"Not much out here. {} is barely an outpost — \
             don't expect reliable information.\"",
            ctx.location.name,
        );
        let summary = format!("{}: low infrastructure", ctx.location.name);
        candidates.push(local_color_candidate(display, summary, 0.2));
    }

    // ----- Star type flavor (exotic stars get a comment) -----

    let star_flavor: Option<(&str, f64)> = match ctx.system.star_type {
        StarType::BlackHole => Some((
            "You can't see it directly, but the lensing is unmistakable — a dark \
             circle eating the starfield. The station creaks when it accretes.",
            0.40,
        )),
        StarType::Neutron => Some((
            "The neutron star pulses through the viewport like a lighthouse. \
             Instruments flicker every time it sweeps past.",
            0.35,
        )),
        StarType::Pulsar => Some((
            "The pulsar ticks like a clock you can feel in your teeth. \
             Navigation is precise but the radiation is brutal.",
            0.35,
        )),
        StarType::WolfRayet => Some((
            "The Wolf-Rayet is shedding its skin — luminous gas shells expanding \
             outward. Beautiful to look at. Don't go outside.",
            0.30,
        )),
        StarType::BlueSuperGiant | StarType::BlueGiant => Some((
            "The star fills half the viewport. Blue-white and furious. Radiation \
             warnings ping every few minutes — you get used to it.",
            0.28,
        )),
        StarType::RedGiant => Some((
            "The red giant hangs out there like a dying ember the size of a solar \
             system. The light makes everything look like permanent sunset.",
            0.25,
        )),
        StarType::BrownDwarf => Some((
            "It's dark here. The brown dwarf barely qualifies as a star — more \
             like a warm spot in the void. Station lights do the real work.",
            0.25,
        )),
        StarType::Binary => Some((
            "Two suns. The shadows never sit still. Takes a few hours before \
             your eyes stop trying to make sense of the light.",
            0.22,
        )),
        StarType::WhiteDwarf => Some((
            "The white dwarf is small and fierce — a tiny bright point throwing \
             hard light across the system. Everything looks overexposed.",
            0.20,
        )),
        _ => None, // Common star types don't warrant comment.
    };

    if let Some((text, score)) = star_flavor {
        let display = format!("\"{}\"", text);
        let summary = format!("{}: notable star", ctx.system.name);
        candidates.push(local_color_candidate(display, summary, score));
    }

    // ----- Faction atmosphere at this system -----

    let faction_count = ctx.system.faction_presence.len();
    let visible_count = ctx.system.faction_presence.iter()
        .filter(|fp| fp.visibility > 0.4)
        .count();

    if visible_count >= 3 {
        let display = "\"Three or more insignias on the concourse. Everyone's polite, \
             but nobody's relaxed. You can feel the factions sizing each other up.\"";
        let summary = format!("{}: multi-faction tension", ctx.location.name);
        candidates.push(local_color_candidate(display.to_string(), summary, 0.28));
    } else if let Some(dominant) = ctx.system.faction_presence.iter()
        .filter(|fp| fp.visibility > 0.5)
        .max_by(|a, b| a.strength.partial_cmp(&b.strength).unwrap_or(std::cmp::Ordering::Equal))
    {
        if dominant.strength > 0.6 {
            if let Some(faction) = ctx.galaxy.factions.iter().find(|f| f.id == dominant.faction_id) {
                let flavor = match faction.category {
                    FactionCategory::Military => {
                        "Uniforms everywhere. Security checkpoints at every junction. \
                         The kind of place where you keep your ident ready."
                    }
                    FactionCategory::Economic => {
                        "Everything here has a price tag. The concourse is all \
                         storefronts and trading terminals. Commerce never sleeps."
                    }
                    FactionCategory::Criminal => {
                        "The kind of place where you don't ask questions and nobody \
                         asks them of you. Useful, if you know how to behave."
                    }
                    FactionCategory::Guild => {
                        "Guild banners on the docking pylons. Engineers and pilots \
                         everywhere. The sort of station that runs well."
                    }
                    FactionCategory::Religious => {
                        "Chanting drifts from somewhere down-corridor. Pilgrims mix \
                         with the dock crews. There's incense in the air recyclers."
                    }
                    FactionCategory::Academic => {
                        "Research equipment stacked in every corridor. Half the people \
                         here look like they haven't slept in days. Lab coats and coffee."
                    }
                    FactionCategory::Political => {
                        "Officials and aides everywhere, moving with purpose. \
                         Screens broadcasting policy debates. Bureaucracy in motion."
                    }
                };
                let display = format!("\"{}\"", flavor);
                let summary = format!(
                    "{}: {} atmosphere ({})",
                    ctx.location.name, faction.category, faction.name,
                );
                candidates.push(local_color_candidate(display, summary, 0.25));
            }
        }
    }

    if faction_count == 0 && ctx.location.infrastructure >= InfrastructureLevel::Colony {
        let display = "\"No faction insignias on the concourse. Nobody's claimed this \
             place — or everyone who tried gave up. Frontier rules.\"";
        let summary = format!("{}: unclaimed territory", ctx.location.name);
        candidates.push(local_color_candidate(display.to_string(), summary, 0.22));
    }

    // ----- Location type flavor -----

    let loc_flavor: Option<(&str, f64)> = match &ctx.location.location_type {
        LocationType::AsteroidBelt => Some((
            "Rocks drift past the viewport. The whole station hums with mining \
             equipment — drills, ore processors, the clang of cargo pods coupling.",
            0.22,
        )),
        LocationType::PlanetSurface { body_type } => match body_type {
            BodyType::Gaia => Some((
                "Real air outside. Wind, sky, the faint smell of vegetation \
                 through the port vents. Crew members linger at the airlocks.",
                0.25,
            )),
            BodyType::IceWorld => Some((
                "The cold seeps through the docking seals. Everything outside \
                 is white and grey and still. Beautiful, in a desolate way.",
                0.20,
            )),
            BodyType::Oceanic => Some((
                "Water in every direction through the lower viewports. The \
                 station sways slightly with the current. Takes getting used to.",
                0.22,
            )),
            BodyType::Barren => Some((
                "Dust and rock as far as the sensors reach. The dome keeps \
                 the nothing out. People here chose isolation deliberately.",
                0.18,
            )),
            _ => None,
        },
        LocationType::DeepSpace => Some((
            "Nothing out the viewport but stars. The station hangs in the void \
             like a dropped coin. No planet, no belt — just the structure and \
             whatever brought people here.",
            0.22,
        )),
        LocationType::Megastructure { .. } => Some((
            "The scale is wrong. Corridors that go on too long, ceilings too high, \
             proportions built for something other than human comfort. You feel small.",
            0.30,
        )),
        _ => None,
    };

    if let Some((text, score)) = loc_flavor {
        let display = format!("\"{}\"", text);
        let summary = format!("{}: location atmosphere", ctx.location.name);
        candidates.push(local_color_candidate(display, summary, score));
    }

    // ----- Ship condition (you notice this when you dock) -----

    let ship = &ctx.journey.ship;
    if ship.hull_condition < 0.5 {
        let display = if ship.hull_condition < 0.25 {
            "\"Dock workers give your hull a long look as you taxi in. One of \
             them shakes her head. You pretend not to notice.\""
        } else {
            "\"Your ship is showing its scars. A few glances from the dock \
             crew — nothing hostile, just the quiet assessment of professionals.\""
        };
        let summary = format!(
            "Ship hull at {:.0}% — drawing attention",
            ship.hull_condition * 100.0,
        );
        candidates.push(local_color_candidate(display.to_string(), summary, 0.28));
    }

    // ----- Supply situation (prices tell a story) -----

    if let Some(economy) = &ctx.location.economy {
        if economy.fuel_price > 5.0 {
            let display = format!(
                "\"Fuel's expensive here — {:.0} credits a unit. Someone's either \
                 gouging or there's a shortage. Either way, plan accordingly.\"",
                economy.fuel_price,
            );
            let summary = format!(
                "{}: fuel at {:.0} cr (expensive)",
                ctx.location.name, economy.fuel_price,
            );
            candidates.push(local_color_candidate(display, summary, 0.22));
        } else if economy.fuel_price < 2.0 {
            let display = format!(
                "\"Fuel's cheap here — {:.0} credits a unit. Might be worth \
                 topping off the tank while you can.\"",
                economy.fuel_price,
            );
            let summary = format!(
                "{}: fuel at {:.0} cr (cheap)",
                ctx.location.name, economy.fuel_price,
            );
            candidates.push(local_color_candidate(display, summary, 0.18));
        }
    }

    // ----- Busy hub vs. quiet port -----

    if ctx.location.infrastructure >= InfrastructureLevel::Hub && faction_count >= 3 {
        let display = "\"Busy port. Ships queuing for berths, cargo loaders running \
             double shifts, every docking bay full. The concourse sounds like a \
             market day.\"";
        let summary = format!("{}: busy port traffic", ctx.location.name);
        candidates.push(local_color_candidate(display.to_string(), summary, 0.15));
    } else if ctx.location.infrastructure == InfrastructureLevel::Colony && faction_count <= 1 {
        let display = "\"Quiet here. A handful of ships, long stretches of empty \
             corridor. The bartender has time to talk.\"";
        let summary = format!("{}: quiet port", ctx.location.name);
        candidates.push(local_color_candidate(display.to_string(), summary, 0.15));
    }

    candidates
}

/// Helper: build a LocalColor candidate without repeating boilerplate.
fn local_color_candidate(
    display_text: String,
    summary: String,
    score: f64,
) -> ScoredCandidate {
    ScoredCandidate {
        id: Uuid::new_v4(),
        category: RumorCategory::LocalColor,
        content: RumorContent::LocalColor {
            description: display_text.clone(),
        },
        display_text,
        summary,
        score,
        reliability: 1.0, // Direct observations don't lie.
        expires_in: RumorCategory::LocalColor.default_expiry(),
    }
}

// ---------------------------------------------------------------------------
// Rumor validation
// ---------------------------------------------------------------------------

/// A result from validating a trade rumor against actual prices.
pub struct RumorValidation {
    pub rumor_idx: usize,
    pub outcome: starbound_core::rumor::RumorOutcome,
    pub message: String,
}

/// Validate trade tip rumors when the player docks at a location.
///
/// Checks rumors that reference this system as a sell destination.
/// Compares the estimated spread against actual prices. Returns
/// validation results for any rumors that can be checked here.
pub fn validate_rumors_at_location(
    journey: &Journey,
    system: &StarSystem,
    location: &Location,
    galactic_day: f64,
) -> Vec<RumorValidation> {
    let mut results = Vec::new();
    let economy = match &location.economy {
        Some(e) => e,
        None => return results,
    };

    for (idx, rumor) in journey.discovered_rumors.iter().enumerate() {
        // Only validate trade tips that haven't been checked yet.
        if rumor.outcome.is_some() || rumor.acted_on {
            continue;
        }

        if let RumorContent::TradeTip {
            ref good, sell_system, estimated_sell_price, ..
        } = rumor.content {
            // Only validate if this is the sell destination.
            if sell_system != system.id {
                continue;
            }

            // Find the actual sell price for this good.
            let actual_sell = TradeGood::all().iter()
                .find(|g| g.display_name() == good)
                .map(|g| economy.sell_price(*g));

            let actual_sell = match actual_sell {
                Some(p) => p,
                None => continue,
            };

            // Check if the rumor has expired.
            let age = galactic_day - rumor.generated_at;
            let expired = age > rumor.expires_in;

            // Compare actual sell price vs what was estimated when the rumor was heard.
            // Within 20% = accurate, beyond that = stale, expired = stale.
            let price_diff = (actual_sell - estimated_sell_price).abs();
            let tolerance = estimated_sell_price.max(1.0) * 0.2;

            let outcome = if expired {
                starbound_core::rumor::RumorOutcome::Stale
            } else if price_diff <= tolerance {
                starbound_core::rumor::RumorOutcome::Accurate
            } else {
                starbound_core::rumor::RumorOutcome::Stale
            };

            let message = match outcome {
                starbound_core::rumor::RumorOutcome::Accurate => {
                    format!(
                        "Trade tip confirmed: {} is selling for ~{:.0} here, as expected.",
                        good, actual_sell,
                    )
                }
                starbound_core::rumor::RumorOutcome::Stale => {
                    format!(
                        "Trade tip outdated: {} is now selling for ~{:.0} here — \
                         prices have shifted since you heard.",
                        good, actual_sell,
                    )
                }
                starbound_core::rumor::RumorOutcome::Inaccurate => {
                    format!(
                        "Trade tip was wrong: {} prices at {} don't match what you heard.",
                        good, system.name,
                    )
                }
            };

            results.push(RumorValidation {
                rumor_idx: idx,
                outcome,
                message,
            });
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use starbound_core::galaxy::*;
    use starbound_core::ship::{Module, Ship, ShipModules};
    use starbound_core::mission::{MissionState, MissionType};
    use starbound_core::reputation::PlayerProfile;
    use starbound_core::time::Timestamp;
    use std::collections::HashMap;

    fn test_economy(fuel_price: f32, food_prod: f32, food_cons: f32) -> SystemEconomy {
        let mut production = HashMap::new();
        let mut consumption = HashMap::new();
        production.insert(TradeGood::Food, food_prod);
        consumption.insert(TradeGood::Food, food_cons);
        production.insert(TradeGood::MedicalSupplies, 0.2);
        consumption.insert(TradeGood::MedicalSupplies, 0.8);
        SystemEconomy {
            production,
            consumption,
            price_volatility: 0.5,
            fuel_price,
            supply_price: 2.0,
        }
    }

    fn test_location(name: &str, infra: InfrastructureLevel, economy: Option<SystemEconomy>) -> Location {
        Location {
            id: Uuid::new_v4(),
            name: name.into(),
            location_type: LocationType::Station,
            orbital_distance: 1.0,
            infrastructure: infra,
            controlling_faction: None,
            economy,
            description: "A test location.".into(),
            services: vec![
                LocationService::Docking,
                LocationService::Trade,
                LocationService::Rumors,
            ],
            discovered: true,
        }
    }

    fn test_system(name: &str, locations: Vec<Location>) -> StarSystem {
        StarSystem {
            id: Uuid::new_v4(),
            name: name.into(),
            position: (0.0, 0.0),
            star_type: StarType::YellowDwarf,
            controlling_civ: None,
            infrastructure_level: InfrastructureLevel::Hub,
            history: vec![],
            active_threads: vec![],
            time_factor: 1.0,
            faction_presence: vec![],
            locations,
        }
    }

    fn test_journey(current_system: Uuid) -> Journey {
        Journey {
            ship: Ship {
                name: "Test Ship".into(),
                hull_condition: 1.0,
                fuel: 80.0,
                fuel_capacity: 100.0,
                supplies: 80.0,
                supply_capacity: 100.0,
                cargo: HashMap::new(),
                cargo_capacity: 50,
                modules: ShipModules {
                    engine: Module::standard("Test Engine"),
                    sensors: Module::standard("Test Sensors"),
                    comms: Module::standard("Test Comms"),
                    weapons: Module::standard("Test Weapons"),
                    life_support: Module::standard("Test Life Support"),
                },
            },
            current_system,
            current_location: None,
            time: Timestamp::zero(),
            resources: 500.0,
            mission: MissionState {
                mission_type: MissionType::Search,
                core_truth: "Test".into(),
                knowledge_nodes: vec![],
            },
            crew: vec![],
            threads: vec![],
            event_log: vec![],
            civ_standings: HashMap::new(),
            profile: PlayerProfile::new(),
            active_contracts: vec![],
            discovered_rumors: vec![],
        }
    }

    fn minimal_galaxy(systems: Vec<StarSystem>) -> GeneratedGalaxy {
        GeneratedGalaxy {
            sector: Sector {
                id: Uuid::new_v4(),
                name: "Test Sector".into(),
                description: "A test sector.".into(),
                system_ids: systems.iter().map(|s| s.id).collect(),
            },
            start_system_id: systems[0].id,
            civilizations: vec![],
            factions: vec![],
            connections: vec![],
            npcs: vec![],
            systems,
        }
    }

    #[test]
    fn test_trade_scanner_finds_spread() {
        // System A: food is cheap to buy (high production, low consumption).
        let loc_a = test_location("Station A", InfrastructureLevel::Hub, Some(test_economy(3.0, 0.9, 0.1)));
        let sys_a = test_system("Alpha", vec![loc_a.clone()]);

        // System B: food is expensive (low production, high consumption).
        let loc_b = test_location("Station B", InfrastructureLevel::Colony, Some(test_economy(3.0, 0.1, 0.9)));
        let sys_b = test_system("Beta", vec![loc_b]);

        let galaxy = minimal_galaxy(vec![sys_a.clone(), sys_b]);
        let journey = test_journey(sys_a.id);

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc_a,
            system: &sys_a,
        };

        let candidates = scan_trade(&ctx, 0.8);
        assert!(!candidates.is_empty(), "Should find at least one trade tip");
        assert!(candidates[0].score > 0.0, "Top trade tip should have positive score");
        assert_eq!(candidates[0].category, RumorCategory::TradeTip);
    }

    #[test]
    fn test_faction_scanner_reads_events() {
        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let faction_id = Uuid::new_v4();

        let mut sys = test_system("Alpha", vec![loc.clone()]);
        sys.faction_presence.push(FactionPresence {
            faction_id,
            strength: 0.6,
            visibility: 0.8,
            services: vec![],
        });

        let galaxy = minimal_galaxy(vec![sys.clone()]);
        let journey = test_journey(sys.id);

        let events = vec![
            TickEvent {
                tick_number: 1,
                galactic_day: 365.0,
                description: "Tensions rose between factions.".into(),
                entities: vec![faction_id],
                category: starbound_simulation::tick::TickEventCategory::Military,
            },
        ];

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &events,
            location: &loc,
            system: &sys,
        };

        let candidates = scan_factions(&ctx, 0.8);
        assert!(!candidates.is_empty(), "Should find faction intel from tick events");
        assert_eq!(candidates[0].category, RumorCategory::FactionIntel);
    }

    #[test]
    fn test_local_color_time_distortion() {
        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let mut sys = test_system("Drift", vec![loc.clone()]);
        sys.time_factor = 2.0;

        let galaxy = minimal_galaxy(vec![sys.clone()]);
        let journey = test_journey(sys.id);

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };

        let candidates = scan_local_color(&ctx, 0.8);
        assert!(!candidates.is_empty(), "Should comment on time distortion");
        assert_eq!(candidates[0].category, RumorCategory::LocalColor);
    }

    #[test]
    fn test_generate_rumors_variety() {
        // Set up a galaxy where all scanners have candidates.
        let loc_a = test_location("Station A", InfrastructureLevel::Hub, Some(test_economy(3.0, 0.9, 0.1)));
        let mut sys_a = test_system("Alpha", vec![loc_a.clone()]);
        sys_a.time_factor = 2.0;

        let loc_b = test_location("Station B", InfrastructureLevel::Colony, Some(test_economy(3.0, 0.1, 0.9)));
        let sys_b = test_system("Beta", vec![loc_b]);

        let galaxy = minimal_galaxy(vec![sys_a.clone(), sys_b]);
        let journey = test_journey(sys_a.id);

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc_a,
            system: &sys_a,
        };

        let mut rng = StdRng::seed_from_u64(42);
        let rumors = generate_rumors(&ctx, &mut rng);

        assert!(!rumors.is_empty(), "Should generate at least one rumor");

        // Check that we have variety (not all the same category).
        let categories: Vec<RumorCategory> = rumors.iter().map(|r| r.category).collect();
        let unique: std::collections::HashSet<_> = categories.iter().collect();
        if rumors.len() > 1 {
            assert!(unique.len() > 1, "Multiple rumors should span multiple categories");
        }
    }

    #[test]
    fn test_contract_scanner_finds_npcs() {
        use starbound_core::npc::{Npc, Species, BiologicalSex, NpcPersonality};

        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let sys = test_system("Alpha", vec![loc.clone()]);

        let faction_id = Uuid::new_v4();
        let mut npc = Npc::new(
            "Maren Vasquez",
            "Guild Factor",
            Species::Human { sex: BiologicalSex::Female },
            Some(faction_id),
            sys.id,
            "A seasoned trader.",
        );
        npc.home_location_id = Some(loc.id);
        npc.disposition = 0.1; // Neutral — will offer contracts.
        npc.personality = NpcPersonality {
            warmth: 0.7,
            boldness: 0.4,
            idealism: 0.6,
        };

        let mut galaxy = minimal_galaxy(vec![sys.clone()]);
        galaxy.factions.push(starbound_core::galaxy::Faction {
            id: faction_id,
            name: "Corridor Guild".into(),
            category: FactionCategory::Guild,
            scope: starbound_core::galaxy::FactionScope::Independent,
            ethos: starbound_core::galaxy::FactionEthos {
                alignment: 0.0,
                openness: 0.7,
                aggression: 0.2,
            },
            influence: HashMap::new(),
            player_standing: starbound_core::galaxy::FactionStanding::unknown(),
            description: "A guild.".into(),
            notable_assets: vec![],
        });
        galaxy.npcs.push(npc);

        let mut journey = test_journey(sys.id);
        journey.current_location = Some(loc.id);

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };

        let candidates = scan_contracts(&ctx, 0.8);
        assert!(!candidates.is_empty(), "Should find contract leads from NPCs");
        assert_eq!(candidates[0].category, RumorCategory::ContractLead);

        // Unmet NPC — display text should use title, not name.
        assert!(
            candidates[0].display_text.contains("Guild Factor"),
            "Contract lead should reference unmet NPC by title"
        );
        assert!(
            !candidates[0].display_text.contains("Maren Vasquez"),
            "Contract lead should NOT reveal unmet NPC's name"
        );

        // Now mark the NPC as met and regenerate.
        galaxy.npcs[0].met_player = true;
        let ctx2 = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };
        let candidates2 = scan_contracts(&ctx2, 0.8);
        assert!(
            candidates2[0].display_text.contains("Maren Vasquez"),
            "Contract lead should show met NPC's name"
        );
    }

    #[test]
    fn test_contract_scanner_skips_hostile_npcs() {
        use starbound_core::npc::{Npc, Species, BiologicalSex, NpcPersonality};

        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let sys = test_system("Alpha", vec![loc.clone()]);

        let mut npc = Npc::new(
            "Cold Officer",
            "Watch Officer",
            Species::Human { sex: BiologicalSex::Male },
            None,
            sys.id,
            "A hostile officer.",
        );
        npc.home_location_id = Some(loc.id);
        npc.disposition = -0.6; // Hostile — won't offer contracts.
        npc.personality = NpcPersonality {
            warmth: 0.2,
            boldness: 0.8,
            idealism: 0.3,
        };

        let mut galaxy = minimal_galaxy(vec![sys.clone()]);
        galaxy.npcs.push(npc);

        let mut journey = test_journey(sys.id);
        journey.current_location = Some(loc.id);

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };

        let candidates = scan_contracts(&ctx, 0.8);
        assert!(candidates.is_empty(), "Should not generate leads for hostile NPCs");
    }

    // -----------------------------------------------------------------------
    // Expanded faction scanner tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_faction_scanner_civ_stability() {
        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let mut sys = test_system("Alpha", vec![loc.clone()]);

        // Create a civilization with low stability.
        let civ_id = Uuid::new_v4();
        sys.controlling_civ = Some(civ_id);

        let mut galaxy = minimal_galaxy(vec![sys.clone()]);
        galaxy.civilizations.push(Civilization {
            id: civ_id,
            name: "Shaky Republic".into(),
            ethos: CivEthos {
                expansionist: 0.3, isolationist: 0.2, militaristic: 0.4,
                diplomatic: 0.5, theocratic: 0.1, mercantile: 0.6,
                technocratic: 0.3, communal: 0.4,
            },
            capabilities: CivCapabilities {
                size: 0.5, wealth: 0.4, technology: 0.5, military: 0.3,
            },
            relationships: HashMap::new(),
            internal_dynamics: InternalDynamics {
                stability: 0.25,
                pressures: vec![CivPressure {
                    description: "Trade unions demanding better terms".into(),
                    source_faction: None,
                }],
            },
            faction_ids: vec![],
        });

        let journey = test_journey(sys.id);
        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };

        let candidates = scan_factions(&ctx, 0.8);
        assert!(
            candidates.len() >= 2,
            "Should find stability + pressure intel (got {})",
            candidates.len(),
        );
        assert!(
            candidates.iter().any(|c| c.display_text.contains("Shaky Republic")),
            "Should mention the civilization by name",
        );
    }

    #[test]
    fn test_faction_scanner_military_tension() {
        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let mut sys = test_system("Alpha", vec![loc.clone()]);

        let civ_a = Uuid::new_v4();
        let civ_b = Uuid::new_v4();
        sys.controlling_civ = Some(civ_a);

        let mut rels_a = HashMap::new();
        rels_a.insert(civ_b, CivDisposition {
            diplomatic: -0.4, economic: 0.1, military: -0.8,
        });

        let mut galaxy = minimal_galaxy(vec![sys.clone()]);
        galaxy.civilizations.push(Civilization {
            id: civ_a,
            name: "Iron Dominion".into(),
            ethos: CivEthos {
                expansionist: 0.5, isolationist: 0.1, militaristic: 0.8,
                diplomatic: 0.2, theocratic: 0.0, mercantile: 0.3,
                technocratic: 0.4, communal: 0.2,
            },
            capabilities: CivCapabilities {
                size: 0.6, wealth: 0.5, technology: 0.5, military: 0.8,
            },
            relationships: rels_a,
            internal_dynamics: InternalDynamics { stability: 0.7, pressures: vec![] },
            faction_ids: vec![],
        });
        galaxy.civilizations.push(Civilization {
            id: civ_b,
            name: "Free Colonies".into(),
            ethos: CivEthos {
                expansionist: 0.3, isolationist: 0.4, militaristic: 0.2,
                diplomatic: 0.7, theocratic: 0.0, mercantile: 0.5,
                technocratic: 0.3, communal: 0.6,
            },
            capabilities: CivCapabilities {
                size: 0.4, wealth: 0.5, technology: 0.4, military: 0.3,
            },
            relationships: HashMap::new(),
            internal_dynamics: InternalDynamics { stability: 0.8, pressures: vec![] },
            faction_ids: vec![],
        });

        let journey = test_journey(sys.id);
        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };

        let candidates = scan_factions(&ctx, 0.8);
        let has_tension = candidates.iter().any(|c| {
            c.display_text.contains("Iron Dominion")
                && c.display_text.contains("Free Colonies")
        });
        assert!(has_tension, "Should surface military tension between civs");
    }

    // -----------------------------------------------------------------------
    // Expanded local color tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_local_color_star_type() {
        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let mut sys = test_system("Vortex", vec![loc.clone()]);
        sys.star_type = StarType::BlackHole;

        let galaxy = minimal_galaxy(vec![sys.clone()]);
        let journey = test_journey(sys.id);

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };

        let candidates = scan_local_color(&ctx, 0.8);
        let has_star = candidates.iter().any(|c| c.display_text.contains("lensing"));
        assert!(has_star, "Should comment on black hole star type");
    }

    #[test]
    fn test_local_color_ship_damage() {
        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let sys = test_system("Alpha", vec![loc.clone()]);

        let galaxy = minimal_galaxy(vec![sys.clone()]);
        let mut journey = test_journey(sys.id);
        journey.ship.hull_condition = 0.2;

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };

        let candidates = scan_local_color(&ctx, 0.8);
        let has_damage = candidates.iter().any(|c| c.display_text.contains("hull"));
        assert!(has_damage, "Should comment on damaged ship hull");
    }

    #[test]
    fn test_local_color_faction_atmosphere() {
        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let faction_id = Uuid::new_v4();
        let mut sys = test_system("Alpha", vec![loc.clone()]);
        sys.faction_presence.push(FactionPresence {
            faction_id,
            strength: 0.8,
            visibility: 0.9,
            services: vec![],
        });

        let mut galaxy = minimal_galaxy(vec![sys.clone()]);
        galaxy.factions.push(Faction {
            id: faction_id,
            name: "Naval Command".into(),
            category: FactionCategory::Military,
            scope: FactionScope::Independent,
            ethos: FactionEthos { alignment: 0.5, openness: 0.3, aggression: 0.7 },
            influence: HashMap::new(),
            player_standing: FactionStanding::unknown(),
            description: "A military faction.".into(),
            notable_assets: vec![],
        });

        let journey = test_journey(sys.id);
        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };

        let candidates = scan_local_color(&ctx, 0.8);
        let has_military = candidates.iter().any(|c|
            c.display_text.contains("Uniforms") || c.display_text.contains("uniforms")
        );
        assert!(has_military, "Should describe military faction atmosphere");
    }

    #[test]
    fn test_local_color_gaia_planet() {
        let mut loc = test_location("Colony", InfrastructureLevel::Colony, None);
        loc.location_type = LocationType::PlanetSurface { body_type: BodyType::Gaia };
        let sys = test_system("Eden", vec![loc.clone()]);

        let galaxy = minimal_galaxy(vec![sys.clone()]);
        let journey = test_journey(sys.id);

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };

        let candidates = scan_local_color(&ctx, 0.8);
        let has_gaia = candidates.iter().any(|c|
            c.display_text.contains("air") || c.display_text.contains("sky")
        );
        assert!(has_gaia, "Should comment on gaia planet atmosphere");
    }

    #[test]
    fn test_local_color_common_star_no_comment() {
        let loc = test_location("Station", InfrastructureLevel::Hub, None);
        let sys = test_system("Sol-like", vec![loc.clone()]);
        // Default star_type is YellowDwarf — should not generate star flavor.

        let galaxy = minimal_galaxy(vec![sys.clone()]);
        let journey = test_journey(sys.id);

        let ctx = RumorContext {
            galaxy: &galaxy,
            journey: &journey,
            recent_tick_events: &[],
            location: &loc,
            system: &sys,
        };

        let candidates = scan_local_color(&ctx, 0.8);
        // Should not have any star-type comment for a boring yellow dwarf.
        let has_star = candidates.iter().any(|c| c.summary.contains("notable star"));
        assert!(!has_star, "Yellow dwarf should not generate a star-type comment");
    }
}