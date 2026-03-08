// file: crates/encounters/src/pipeline.rs
//! The encounter pipeline — universal resolution system for both
//! things that happen TO the player and things the player INITIATES.
//!
//! ```text
//! intent_filter   → narrow to action   (Stage 0, player-initiated only)
//! context_filter  → candidate pool     (matcher + prerequisites)
//! pressure_filter → boost situational  (lean into tensions)
//! echo_filter     → boost thread ties  (weave in the past)
//! novelty_check   → balance new vs old (meter fresh content)
//! tone_filter     → pacing bias        (alternate intensity)
//! reputation      → boost identity fit (player labels)
//! priority        → boost important    (quest events override ambient)
//! convergence     → boost resolution   (mature clusters trigger payoff)
//! ```
//!
//! The pipeline scores candidates rather than eliminating them.
//! Every context-eligible event stays in the pool; filters adjust
//! weights so the final weighted-random selection naturally favors
//! the most narratively appropriate encounter. Sometimes the
//! pipeline returns silence — that's by design (but never for
//! player-initiated actions or priority ≥ 2 events).

use std::fmt;

use rand::rngs::StdRng;
use rand::Rng;

use starbound_core::galaxy::{InfrastructureLevel, StarSystem};
use starbound_core::journey::Journey;
use starbound_core::narrative::{ResolutionState, Tone};

use super::matcher::{match_events, MatchContext};
use super::seed_event::SeedEvent;

// Re-export EventTrigger and EventKind so callers can import from pipeline.
pub use super::seed_event::{EventTrigger, EventKind};

// ---------------------------------------------------------------------------
// Player intent — what action the player is initiating
// ---------------------------------------------------------------------------

/// A player-initiated action. When present, the pipeline selects from
/// events whose trigger matches `action:{tag}` rather than from the
/// full pool.
///
/// The string tag for each intent matches against `SeedEvent.trigger`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerIntent {
    /// Buy/sell goods at a station or settlement.
    Trade,
    /// Examine something unusual — anomaly, signal, ruin.
    Investigate,
    /// Fix damaged ship modules at a facility.
    Repair,
    /// Restock supplies (food, water, air).
    Resupply,
    /// Deep scan the current system for signals, objects, anomalies.
    Scan,
    /// Attempt to hire new crew.
    Recruit,
    /// Rest and let crew decompress.
    Rest,
    /// Attempt to smuggle contraband.
    Smuggle,
    /// Open negotiations with a faction or entity.
    Negotiate,
    /// Listen for rumors and gather information.
    GatherRumors,
}

impl PlayerIntent {
    /// The string tag used to match against SeedEvent.trigger.
    pub fn tag(self) -> &'static str {
        match self {
            PlayerIntent::Trade => "trade",
            PlayerIntent::Investigate => "investigate",
            PlayerIntent::Repair => "repair",
            PlayerIntent::Resupply => "resupply",
            PlayerIntent::Scan => "scan",
            PlayerIntent::Recruit => "recruit",
            PlayerIntent::Rest => "rest",
            PlayerIntent::Smuggle => "smuggle",
            PlayerIntent::Negotiate => "negotiate",
            PlayerIntent::GatherRumors => "gather_rumors",
        }
    }

    /// Human-readable label for the CLI menu.
    pub fn label(self) -> &'static str {
        match self {
            PlayerIntent::Trade => "Trade",
            PlayerIntent::Investigate => "Investigate",
            PlayerIntent::Repair => "Repair ship",
            PlayerIntent::Resupply => "Resupply",
            PlayerIntent::Scan => "Scan system",
            PlayerIntent::Recruit => "Recruit crew",
            PlayerIntent::Rest => "Rest",
            PlayerIntent::Smuggle => "Smuggle",
            PlayerIntent::Negotiate => "Negotiate",
            PlayerIntent::GatherRumors => "Gather rumors",
        }
    }
}

impl fmt::Display for PlayerIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

impl From<PlayerIntent> for EventTrigger {
    fn from(intent: PlayerIntent) -> Self {
        EventTrigger::Action(intent.tag().to_string())
    }
}

// ---------------------------------------------------------------------------
// Pipeline result
// ---------------------------------------------------------------------------

/// What the pipeline produces. Either a selected event or silence.
#[derive(Debug)]
pub enum PipelineResult<'a> {
    /// An encounter was selected.
    Event {
        event: &'a SeedEvent,
        /// Why this event was chosen (for debugging and logging).
        reasoning: String,
    },
    /// Nothing happens. Quiet transit. The ship hums.
    Silence { reason: String },
}

// ---------------------------------------------------------------------------
// Pipeline configuration
// ---------------------------------------------------------------------------

/// Tuning knobs for the pipeline. Exposed so tests can adjust them.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Base chance of silence (0.0–1.0). Higher = more quiet moments.
    pub silence_chance: f64,
    /// Extra silence chance per consecutive non-silent encounter.
    pub silence_escalation: f64,
    /// Weight multiplier for events matching pressure conditions.
    pub pressure_boost: f64,
    /// Weight multiplier for events matching active threads.
    pub echo_boost: f64,
    /// Weight multiplier for novel encounters (decreases with thread count).
    pub novelty_base_boost: f64,
    /// How recent tones affect selection. Higher = stronger pacing effect.
    pub pacing_contrast_boost: f64,
    /// Weight multiplier per priority tier.
    /// Priority 0 = ×1.0, Priority 1 = ×1.0, Priority 2 = this, Priority 3 = this².
    pub priority_boost: f64,
    /// Weight multiplier for events that could resolve a mature thread cluster.
    /// A "mature cluster" is 3+ open threads sharing a type or tag pattern.
    pub convergence_boost: f64,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            silence_chance: 0.15,
            silence_escalation: 0.05,
            pressure_boost: 2.0,
            echo_boost: 2.5,
            novelty_base_boost: 1.5,
            pacing_contrast_boost: 1.8,
            priority_boost: 2.5,
            convergence_boost: 3.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline state (persisted between calls)
// ---------------------------------------------------------------------------

/// Tracks pacing state across encounters. Kept in memory during a session.
#[derive(Debug, Clone, Default)]
pub struct PipelineState {
    /// Tones of recent encounters (most recent last). Capped at 5.
    pub recent_tones: Vec<Tone>,
    /// IDs of recently fired events (avoid immediate repeats). Capped at 10.
    pub recent_event_ids: Vec<String>,
    /// How many encounters since last silence.
    pub encounters_since_silence: u32,
}

impl PipelineState {
    pub fn record_event(&mut self, event_id: &str, tone: Tone) {
        self.recent_tones.push(tone);
        if self.recent_tones.len() > 5 {
            self.recent_tones.remove(0);
        }
        self.recent_event_ids.push(event_id.to_string());
        if self.recent_event_ids.len() > 10 {
            self.recent_event_ids.remove(0);
        }
        self.encounters_since_silence += 1;
    }

    pub fn record_silence(&mut self) {
        self.encounters_since_silence = 0;
    }
}

// ---------------------------------------------------------------------------
// The pipeline
// ---------------------------------------------------------------------------

/// Run the full encounter pipeline.
///
/// Takes the seed library, current game state, and pipeline state.
/// Returns either a selected event or silence.
///
/// The `trigger` parameter determines which events are eligible and
/// how silence behaves:
/// - `Arrival` — classic behavior, most arrivals have something
/// - `Transit` — mostly silent, occasional ambient moments
/// - `Docked` — mostly silent, occasional station atmosphere
/// - `Linger` — slightly more common, player chose to spend time
/// - `Action(tag)` — never silent, player chose to act
pub fn run_pipeline<'a>(
    events: &'a [SeedEvent],
    system: &StarSystem,
    journey: &Journey,
    galactic_years_since_last_visit: Option<f64>,
    state: &PipelineState,
    config: &PipelineConfig,
    rng: &mut StdRng,
    trigger: EventTrigger,
    location_type: Option<&str>,
    location_infrastructure: Option<InfrastructureLevel>,
) -> PipelineResult<'a> {
    let is_player_action = trigger.is_player_action();

    // -----------------------------------------------------------------------
    // Stage 0 — Trigger filter
    //
    // Only consider events whose trigger matches what's happening now.
    // For Arrival triggers (the vast majority of existing events),
    // this also accepts events with no explicit trigger set.
    // -----------------------------------------------------------------------
    let working_events: Vec<&SeedEvent> = events
        .iter()
        .filter(|e| e.matches_trigger(&trigger))
        .collect();

    if working_events.is_empty() {
        return PipelineResult::Silence {
            reason: format!("No events available for trigger: {}", trigger.label()),
        };
    }

    // -----------------------------------------------------------------------
    // Check for high-priority events before silence roll.
    // Priority ≥ 2 events that are eligible should override silence.
    // -----------------------------------------------------------------------
    let has_high_priority_candidates = !is_player_action
        && working_events.iter().any(|e| e.priority >= 2);

    // -----------------------------------------------------------------------
    // Silence check — trigger-aware and location-aware.
    //
    // Base silence rate comes from the trigger type for new triggers
    // (transit, docked, linger) and from config for arrival (backward
    // compatible with existing tests and tuning).
    //
    // Location modifiers and encounter escalation stack on top.
    // Skipped for player-initiated actions and high-priority events.
    // -----------------------------------------------------------------------
    if !is_player_action && !has_high_priority_candidates {
        // Arrival uses config (backward compatible), new triggers use
        // their own base rates.
        let base_silence = match &trigger {
            EventTrigger::Arrival => config.silence_chance,
            other => other.base_silence_rate(),
        };
        let location_silence = location_silence_modifier(location_type, location_infrastructure);
        let silence_threshold = (base_silence + location_silence
            + (state.encounters_since_silence as f64 * config.silence_escalation))
            .min(0.95); // Never quite guarantee silence

        if rng.gen::<f64>() < silence_threshold {
            return PipelineResult::Silence {
                reason: format!(
                    "Silence on {} after {} encounters (threshold {:.0}%, base {:.0}%)",
                    trigger.label(),
                    state.encounters_since_silence,
                    silence_threshold * 100.0,
                    base_silence * 100.0,
                ),
            };
        }
    }

    // -----------------------------------------------------------------------
    // Stage 1 — Context filter (matcher + prerequisites)
    // -----------------------------------------------------------------------
    let ctx = MatchContext {
        system,
        journey,
        galactic_years_since_last_visit,
        location_type: location_type.map(|s| s.to_string()),
        location_infrastructure,
        visited_system_names: Vec::new(), // TODO: pass from game state
    };

    // match_events checks prerequisites as hard gates before context
    // requirements.
    let all_context_matched = match_events(events, &ctx);

    // Intersect: must be both context-appropriate AND trigger-matching.
    let trigger_ids: std::collections::HashSet<&str> =
        working_events.iter().map(|e| e.id.as_str()).collect();
    let candidates: Vec<&SeedEvent> = all_context_matched
        .into_iter()
        .filter(|e| trigger_ids.contains(e.id.as_str()))
        .collect();

    if candidates.is_empty() {
        return PipelineResult::Silence {
            reason: format!(
                "No {} events match current context.",
                trigger.label(),
            ),
        };
    }

    // Filter out recently fired events.
    let candidates: Vec<&SeedEvent> = candidates
        .into_iter()
        .filter(|e| !state.recent_event_ids.contains(&e.id))
        .collect();

    if candidates.is_empty() {
        return PipelineResult::Silence {
            reason: "All matching events fired recently.".into(),
        };
    }

    // -----------------------------------------------------------------------
    // For non-action triggers with high-priority candidates: if only
    // low-priority events survived context filtering, apply the silence
    // check now.
    // -----------------------------------------------------------------------
    if !is_player_action
        && has_high_priority_candidates
        && !candidates.iter().any(|e| e.priority >= 2)
    {
        let base_silence = match &trigger {
            EventTrigger::Arrival => config.silence_chance,
            other => other.base_silence_rate(),
        };
        let location_silence = location_silence_modifier(location_type, location_infrastructure);
        let silence_threshold = (base_silence + location_silence
            + (state.encounters_since_silence as f64 * config.silence_escalation))
            .min(0.95);

        if rng.gen::<f64>() < silence_threshold {
            return PipelineResult::Silence {
                reason: format!(
                    "Silence after {} encounters (threshold {:.0}%)",
                    state.encounters_since_silence,
                    silence_threshold * 100.0,
                ),
            };
        }
    }

    // -----------------------------------------------------------------------
    // Stages 2–8: Score each candidate
    // -----------------------------------------------------------------------
    let scored: Vec<(&SeedEvent, f64, String)> = candidates
        .iter()
        .map(|event| {
            let mut weight = 1.0;
            let mut reasons = Vec::new();

            // Stage 2 — Pressure: boost events that match player tensions
            let pressure = pressure_score(event, journey);
            if pressure > 0.0 {
                weight *= 1.0 + pressure * config.pressure_boost;
                reasons.push(format!("pressure +{:.1}", pressure));
            }

            // Stage 3 — Echo: boost events whose type suggests thread ties
            let echo = echo_score(event, journey);
            if echo > 0.0 {
                weight *= 1.0 + echo * config.echo_boost;
                reasons.push(format!("echo +{:.1}", echo));
            }

            // Stage 4 — Novelty: boost novel encounters, modulated by history
            let novelty = novelty_score(event, journey);
            if novelty > 0.0 {
                weight *= 1.0 + novelty * config.novelty_base_boost;
                reasons.push(format!("novelty +{:.1}", novelty));
            }

            // Stage 5 — Tone/pacing: boost tones that contrast with recent
            let pacing = pacing_score(event, &state.recent_tones);
            if pacing > 0.0 {
                weight *= 1.0 + pacing * config.pacing_contrast_boost;
                reasons.push(format!("pacing +{:.1}", pacing));
            }

            // Stage 6 — Reputation: boost events matching player identity
            let rep_weight = journey
                .profile
                .encounter_weight(&event.context_requirements.tags);
            if (rep_weight - 1.0).abs() > 0.01 {
                weight *= rep_weight;
                reasons.push(format!("reputation ×{:.1}", rep_weight));
            }

            // Stage 7 — Priority: higher priority events get scoring bonus
            let priority_mult = priority_multiplier(event.priority, config.priority_boost);
            if priority_mult > 1.0 {
                weight *= priority_mult;
                reasons.push(format!("priority({}): ×{:.1}", event.priority, priority_mult));
            }

            // Stage 8 — Convergence: boost events that resolve mature clusters
            let convergence = convergence_score(event, journey);
            if convergence > 0.0 {
                weight *= 1.0 + convergence * config.convergence_boost;
                reasons.push(format!("convergence +{:.1}", convergence));
            }

            let reason = if reasons.is_empty() {
                "base weight".into()
            } else {
                reasons.join(", ")
            };

            (*event, weight, reason)
        })
        .collect();

    // -----------------------------------------------------------------------
    // Weighted random selection
    // -----------------------------------------------------------------------
    let total_weight: f64 = scored.iter().map(|(_, w, _)| w).sum();

    if total_weight <= 0.0 {
        return PipelineResult::Silence {
            reason: "All candidates scored zero.".into(),
        };
    }

    let mut roll = rng.gen::<f64>() * total_weight;

    for (event, weight, reason) in &scored {
        roll -= weight;
        if roll <= 0.0 {
            return PipelineResult::Event {
                event,
                reasoning: format!("{} (w={:.1}) [{}]", event.id, weight, reason),
            };
        }
    }

    // Fallback (floating point edge case) — pick the last one.
    let (event, weight, reason) = scored.last().unwrap();
    PipelineResult::Event {
        event,
        reasoning: format!("{} (w={:.1}) [{}] (fallback)", event.id, weight, reason),
    }
}

// ---------------------------------------------------------------------------
// Location-aware silence modifier
// ---------------------------------------------------------------------------

/// Compute an additive silence modifier based on where the player is.
///
/// Stations are social hubs — encounters feel natural. Uninhabited
/// planets are desolate — the player should mostly experience the
/// emptiness. This modifier is added to the base silence_chance.
///
/// Returns 0.0–0.60. Combined with the base 0.15, this creates:
///   Station (Colony+):  ~15% silence → ~85% encounter rate
///   Station (Outpost):  ~25% silence → ~75% encounter rate
///   Colonized planet:   ~40% silence → ~60% encounter rate
///   Uninhabited planet: ~70% silence → ~30% encounter rate
///   Asteroid belt:      ~45% silence → ~55% encounter rate
///   Deep space:         ~50% silence → ~50% encounter rate
///   Moon (uninhabited): ~65% silence → ~35% encounter rate
fn location_silence_modifier(
    location_type: Option<&str>,
    location_infra: Option<InfrastructureLevel>,
) -> f64 {
    let type_mod = match location_type {
        Some("station") => 0.0,
        Some("planet_surface") => 0.20,
        Some("moon") => 0.30,
        Some("asteroid_belt") => 0.15,
        Some("deep_space") => 0.20,
        Some("megastructure") => 0.05,
        None => 0.0,  // System edge — FTL arrival rate
        _ => 0.15,
    };

    let infra_mod = match location_infra {
        Some(InfrastructureLevel::None) => 0.25,
        Some(InfrastructureLevel::Outpost) => 0.10,
        Some(InfrastructureLevel::Colony) => 0.05,
        Some(InfrastructureLevel::Established) => 0.0,
        Some(InfrastructureLevel::Hub) => 0.0,
        Some(InfrastructureLevel::Capital) => 0.0,
        None => 0.0, // No location — use base rate
    };

    type_mod + infra_mod
}

// ---------------------------------------------------------------------------
// Stage 2 — Pressure scoring
// ---------------------------------------------------------------------------

/// Score how well an event leans into the player's current tensions.
/// Returns 0.0–1.0.
fn pressure_score(event: &SeedEvent, journey: &Journey) -> f64 {
    let mut score: f64 = 0.0;
    let req = &event.context_requirements;

    // Low fuel + event requires low fuel = pressure match.
    if req.fuel_below_fraction.is_some() {
        let fuel_frac = journey.ship.fuel / journey.ship.fuel_capacity;
        if fuel_frac < 0.3 {
            score += 0.5;
        }
    }

    // Damaged hull + event requires damage = pressure match.
    if req.hull_below.is_some() && journey.ship.hull_condition < 0.5 {
        score += 0.5;
    }

    // Faction-controlled space with active grudge threads = pressure.
    if req.faction_controlled == Some(true) && !journey.threads.is_empty() {
        let grudge_threads = journey
            .threads
            .iter()
            .filter(|t| t.thread_type == starbound_core::narrative::ThreadType::Grudge)
            .filter(|t| t.resolution == ResolutionState::Open)
            .count();
        if grudge_threads > 0 {
            score += 0.3;
        }
    }

    score.min(1.0)
}

// ---------------------------------------------------------------------------
// Stage 3 — Echo scoring
// ---------------------------------------------------------------------------

/// Score how well an event connects to dangling narrative threads.
/// Returns 0.0–1.0.
fn echo_score(event: &SeedEvent, journey: &Journey) -> f64 {
    if journey.threads.is_empty() {
        return 0.0;
    }

    let open_threads = journey
        .threads
        .iter()
        .filter(|t| {
            t.resolution == ResolutionState::Open || t.resolution == ResolutionState::Partial
        })
        .count();

    let has_time_req = event
        .context_requirements
        .time_since_last_visit_galactic_years_min
        .is_some();

    let mut score: f64 = 0.0;

    if has_time_req && open_threads > 0 {
        score += 0.6;
    }

    // More open threads = more echo potential (capped).
    score += (open_threads as f64 * 0.1).min(0.4);

    score.min(1.0)
}

// ---------------------------------------------------------------------------
// Stage 4 — Novelty scoring
// ---------------------------------------------------------------------------

/// Score how much this event introduces something new.
/// Novelty is inversely proportional to thread count — early game
/// favors new content, late game favors echoes.
fn novelty_score(event: &SeedEvent, journey: &Journey) -> f64 {
    let is_novel = event.encounter_type == "novel";

    if !is_novel {
        return 0.0;
    }

    // Novelty bonus decreases as threads accumulate.
    // 0 threads → 1.0, 10+ threads → 0.2
    let thread_count = journey.threads.len() as f64;
    (1.0 - thread_count * 0.08).max(0.2)
}

// ---------------------------------------------------------------------------
// Stage 5 — Tone/pacing scoring
// ---------------------------------------------------------------------------

/// Score how well an event's tone contrasts with recent encounters.
/// Returns 0.0–1.0. Higher = better pacing fit.
fn pacing_score(event: &SeedEvent, recent_tones: &[Tone]) -> f64 {
    if recent_tones.is_empty() {
        return 0.0; // No history to contrast with.
    }

    let event_tone = Tone::parse(&event.tone);

    // Count how many recent encounters share this tone.
    let same_count = recent_tones.iter().filter(|t| **t == event_tone).count();

    // Count intensity of recent encounters.
    let recent_intensity: f64 =
        recent_tones.iter().map(|t| tone_intensity(t)).sum::<f64>() / recent_tones.len() as f64;

    let event_intensity = tone_intensity(&event_tone);

    let mut score: f64 = 0.0;

    // Reward tonal variety — if we haven't seen this tone recently.
    if same_count == 0 {
        score += 0.4;
    }

    // After high intensity, reward low intensity (and vice versa).
    let intensity_contrast = (event_intensity - recent_intensity).abs();
    score += intensity_contrast * 0.6;

    score.min(1.0)
}

fn tone_intensity(tone: &Tone) -> f64 {
    match tone {
        Tone::Urgent => 1.0,
        Tone::Tense => 0.8,
        Tone::Wonder => 0.6,
        Tone::Melancholy => 0.4,
        Tone::Quiet => 0.2,
        Tone::Mundane => 0.1,
    }
}

// ---------------------------------------------------------------------------
// Stage 7 — Priority scoring
// ---------------------------------------------------------------------------

/// Convert priority tier to a weight multiplier.
/// Priority 0–1 = ×1.0 (no boost).
/// Priority 2 = ×boost.
/// Priority 3 = ×boost².
fn priority_multiplier(priority: u8, boost: f64) -> f64 {
    match priority {
        0 | 1 => 1.0,
        2 => boost,
        3 => boost * boost,
        _ => 1.0,
    }
}

// ---------------------------------------------------------------------------
// Stage 8 — Convergence scoring
// ---------------------------------------------------------------------------

/// Score how well this event resolves mature thread clusters.
///
/// A "mature cluster" is 3+ open threads sharing a thread type.
/// Events from the `main_quest` or `side_quest` categories get a
/// convergence boost proportional to cluster maturity.
///
/// This is the echo filter turned up to 11 — when enough puzzle
/// pieces have accumulated, the game leans hard into resolution.
fn convergence_score(event: &SeedEvent, journey: &Journey) -> f64 {
    // Only quest and important events benefit from convergence.
    if event.priority < 2 {
        return 0.0;
    }

    // Count open threads by type.
    let mut type_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for thread in &journey.threads {
        if thread.resolution == ResolutionState::Open
            || thread.resolution == ResolutionState::Partial
        {
            let key = format!("{}", thread.thread_type).to_lowercase();
            *type_counts.entry(key).or_insert(0) += 1;
        }
    }

    // Find the largest cluster.
    let max_cluster = type_counts.values().copied().max().unwrap_or(0);

    if max_cluster < 3 {
        return 0.0;
    }

    // Scale: 3 threads = 0.3, 5 threads = 0.5, 7+ = 0.7 (capped)
    let cluster_maturity = ((max_cluster as f64 - 2.0) * 0.1).min(0.7);

    // Extra boost if the event has prerequisites (it's designed for this moment).
    let has_prereqs = event.context_requirements.prerequisites.is_some();
    if has_prereqs {
        (cluster_maturity * 1.5).min(1.0)
    } else {
        cluster_maturity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::all_seed_events;
    use rand::SeedableRng;
    use starbound_core::crew::*;
    use starbound_core::galaxy::*;
    use starbound_core::mission::*;
    use starbound_core::narrative::*;
    use starbound_core::reputation::PlayerProfile;
    use starbound_core::ship::*;
    use starbound_core::time::Timestamp;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn test_system(infra: InfrastructureLevel, faction: Option<Uuid>) -> StarSystem {
        StarSystem {
            id: Uuid::new_v4(),
            name: "Test".into(),
            position: (0.0, 0.0),
            star_type: StarType::YellowDwarf,
            locations: vec![],
            controlling_civ: faction,
            infrastructure_level: infra,
            history: vec![],
            active_threads: vec![],
            time_factor: 1.0,
            faction_presence: vec![],
        }
    }

    fn test_journey(fuel: f32, hull: f32, crew_count: usize) -> Journey {
        let crew: Vec<CrewMember> = (0..crew_count)
            .map(|i| CrewMember {
                id: Uuid::new_v4(),
                name: format!("Crew {}", i),
                role: CrewRole::Navigator,
                drives: PersonalityDrives {
                    security: 0.5,
                    freedom: 0.5,
                    purpose: 0.5,
                    connection: 0.5,
                    knowledge: 0.5,
                    justice: 0.5,
                },
                trust: Trust::starting_crew(),
                relationships: HashMap::new(),
                background: String::new(),
                state: CrewState {
                    mood: Mood::Content,
                    stress: 0.2,
                    active_concerns: vec![],
                },
                origin: CrewOrigin::Starting,
            })
            .collect();

        Journey {
            ship: Ship {
                name: "Test Ship".into(),
                hull_condition: hull,
                fuel,
                fuel_capacity: 100.0,
                supplies: 80.0,
                supply_capacity: 100.0,
                cargo: HashMap::new(),
                cargo_capacity: 50,
                modules: ShipModules {
                    engine: Module::standard("Engine"),
                    sensors: Module::standard("Sensors"),
                    comms: Module::standard("Comms"),
                    weapons: Module::standard("Weapons"),
                    life_support: Module::standard("Life Support"),
                },
            },
            current_system: Uuid::new_v4(),
            time: Timestamp::zero(),
            resources: 1000.0,
            mission: MissionState {
                mission_type: MissionType::Search,
                core_truth: "Test".into(),
                knowledge_nodes: vec![],
            },
            crew,
            threads: vec![],
            event_log: vec![],
            civ_standings: HashMap::new(),
            profile: PlayerProfile::new(),
            active_contracts: vec![],
            discovered_rumors: vec![],
            current_location: None,
        }
    }

    fn add_threads(journey: &mut Journey, count: usize) {
        for i in 0..count {
            journey.threads.push(Thread {
                id: Uuid::new_v4(),
                thread_type: ThreadType::Mystery,
                associated_entities: vec![],
                tension: 0.7,
                created_at: Timestamp::zero(),
                last_touched: Timestamp::zero(),
                resolution: ResolutionState::Open,
                description: format!("Thread {}", i),
            });
        }
    }

    #[test]
    fn pipeline_produces_events() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let state = PipelineState::default();
        let config = PipelineConfig {
            silence_chance: 0.0,
            silence_escalation: 0.0,
            ..Default::default()
        };
        let mut rng = StdRng::seed_from_u64(42);

        let result = run_pipeline(
            &events, &system, &journey, None, &state, &config, &mut rng, EventTrigger::Arrival, Some("station"), None,
        );

        match result {
            PipelineResult::Event { event, reasoning } => {
                assert!(!event.id.is_empty());
                assert!(!reasoning.is_empty());
            }
            PipelineResult::Silence { reason } => {
                panic!("Expected event, got silence: {}", reason);
            }
        }
    }

    #[test]
    fn novelty_bias_toward_early_game() {
        // With minimal event set, verify the pipeline produces events
        // both early and late game. The scoring stages still affect
        // which events surface — this test validates the pipeline runs
        // without errors at different thread counts.
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let config = PipelineConfig {
            silence_chance: 0.0,
            silence_escalation: 0.0,
            ..Default::default()
        };

        let journey_early = test_journey(80.0, 0.9, 3);
        let mut journey_late = test_journey(80.0, 0.9, 3);
        add_threads(&mut journey_late, 10);

        let mut events_early = 0;
        let mut events_late = 0;
        let trials = 50;

        for seed in 0..trials {
            let mut rng = StdRng::seed_from_u64(seed);
            if let PipelineResult::Event { .. } = run_pipeline(
                &events,
                &system,
                &journey_early,
                None,
                &PipelineState::default(),
                &config,
                &mut rng,
                EventTrigger::Arrival,
                Some("station"),
                None,
            ) {
                events_early += 1;
            }

            let mut rng = StdRng::seed_from_u64(seed);
            if let PipelineResult::Event { .. } = run_pipeline(
                &events,
                &system,
                &journey_late,
                None,
                &PipelineState::default(),
                &config,
                &mut rng,
                EventTrigger::Arrival,
                Some("station"),
                None,
            ) {
                events_late += 1;
            }
        }

        // Both should produce events with silence disabled.
        assert!(
            events_early > 0 && events_late > 0,
            "Pipeline should produce events both early ({}) and late ({})",
            events_early,
            events_late
        );
    }

    #[test]
    fn state_tracks_history() {
        let mut state = PipelineState::default();

        state.record_event("test_1", Tone::Tense);
        state.record_event("test_2", Tone::Quiet);

        assert_eq!(state.recent_tones.len(), 2);
        assert_eq!(state.recent_event_ids.len(), 2);
        assert_eq!(state.encounters_since_silence, 2);

        state.record_silence();
        assert_eq!(state.encounters_since_silence, 0);
        assert_eq!(state.recent_tones.len(), 2);
    }

    #[test]
    fn deterministic_with_same_seed() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let state = PipelineState::default();
        let config = PipelineConfig {
            silence_chance: 0.0,
            silence_escalation: 0.0,
            ..Default::default()
        };

        let mut rng1 = StdRng::seed_from_u64(999);
        let mut rng2 = StdRng::seed_from_u64(999);

        let r1 = run_pipeline(
            &events, &system, &journey, None, &state, &config, &mut rng1, EventTrigger::Arrival, Some("station"), None,
        );
        let r2 = run_pipeline(
            &events, &system, &journey, None, &state, &config, &mut rng2, EventTrigger::Arrival, Some("station"), None,
        );

        match (r1, r2) {
            (PipelineResult::Event { event: e1, .. }, PipelineResult::Event { event: e2, .. }) => {
                assert_eq!(e1.id, e2.id, "Same seed should produce same event");
            }
            (PipelineResult::Silence { .. }, PipelineResult::Silence { .. }) => {}
            _ => panic!("Same seed produced different result types"),
        }
    }

    #[test]
    fn silence_escalates() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let config = PipelineConfig {
            silence_chance: 0.1,
            silence_escalation: 0.1,
            ..Default::default()
        };

        let mut state_stale = PipelineState::default();
        state_stale.encounters_since_silence = 9;

        let mut rng = StdRng::seed_from_u64(42);
        let result = run_pipeline(
            &events,
            &system,
            &journey,
            None,
            &state_stale,
            &config,
            &mut rng,
            EventTrigger::Arrival,
            None,
            None,
        );

        assert!(
            matches!(result, PipelineResult::Silence { .. }),
            "Should be silent after 9 consecutive encounters with escalation"
        );
    }

    // --- Intent mode tests ---

    #[test]
    fn intent_filters_to_matching_events() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let config = PipelineConfig {
            silence_chance: 0.0,
            silence_escalation: 0.0,
            ..Default::default()
        };

        let mut found_scan = false;
        for seed in 0..50 {
            let mut rng = StdRng::seed_from_u64(seed);
            if let PipelineResult::Event { event, .. } = run_pipeline(
                &events,
                &system,
                &journey,
                None,
                &PipelineState::default(),
                &config,
                &mut rng,
                PlayerIntent::Scan.into(),
                None,
                None,
            ) {
                assert!(
                    event.effective_trigger() == EventTrigger::Action("scan".to_string()),
                    "Intent mode should only select scan events, got: {}",
                    event.id,
                );
                found_scan = true;
            }
        }
        assert!(found_scan, "Should have found at least one scan event");
    }

    #[test]
    fn intent_mode_skips_silence() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let config = PipelineConfig {
            silence_chance: 1.0,
            silence_escalation: 0.0,
            ..Default::default()
        };

        let mut rng = StdRng::seed_from_u64(42);
        let result = run_pipeline(
            &events,
            &system,
            &journey,
            None,
            &PipelineState::default(),
            &config,
            &mut rng,
            PlayerIntent::Scan.into(),
            None,
            None,
        );

        assert!(
            matches!(result, PipelineResult::Event { .. }),
            "Intent mode should skip silence check",
        );
    }

    #[test]
    fn arrival_mode_excludes_intent_events() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let config = PipelineConfig {
            silence_chance: 0.0,
            silence_escalation: 0.0,
            ..Default::default()
        };

        for seed in 0..50 {
            let mut rng = StdRng::seed_from_u64(seed);
            if let PipelineResult::Event { event, .. } = run_pipeline(
                &events,
                &system,
                &journey,
                None,
                &PipelineState::default(),
                &config,
                &mut rng,
                EventTrigger::Arrival,
                Some("station"),
                None,
            ) {
                assert!(
                    !event.effective_trigger().is_player_action(),
                    "Arrival mode should not select action events, got: {} (trigger: {:?})",
                    event.id,
                    event.trigger,
                );
            }
        }
    }

    #[test]
    fn nonexistent_intent_returns_silence() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::None, None);
        let journey = test_journey(80.0, 0.9, 3);
        let config = PipelineConfig {
            silence_chance: 0.0,
            silence_escalation: 0.0,
            ..Default::default()
        };

        let mut rng = StdRng::seed_from_u64(42);
        let result = run_pipeline(
            &events,
            &system,
            &journey,
            None,
            &PipelineState::default(),
            &config,
            &mut rng,
            PlayerIntent::Recruit.into(),
            None,
            None,
        );

        assert!(
            matches!(result, PipelineResult::Silence { .. }),
            "Intent with no matching events should return silence",
        );
    }

    // --- Priority tests (new) ---

    #[test]
    fn priority_multiplier_values() {
        assert!((priority_multiplier(0, 2.5) - 1.0).abs() < 0.001);
        assert!((priority_multiplier(1, 2.5) - 1.0).abs() < 0.001);
        assert!((priority_multiplier(2, 2.5) - 2.5).abs() < 0.001);
        assert!((priority_multiplier(3, 2.5) - 6.25).abs() < 0.001);
    }

    #[test]
    fn convergence_zero_without_threads() {
        use super::super::seed_event::*;
        let event = SeedEvent {
            id: "test".into(),
            encounter_type: "contextual".into(),
            tone: "wonder".into(),
            category: "main_quest".into(),
            priority: 3,
            context_requirements: ContextRequirements::default(),
            text: "Test.".repeat(20),
            choices: vec![],
            intents: vec![],
            trigger: EventTrigger::default(),
            event_kind: EventKind::default(),
        };
        let journey = test_journey(80.0, 0.9, 3);
        assert!(
            convergence_score(&event, &journey) < 0.001,
            "No threads should produce zero convergence"
        );
    }

    #[test]
    fn convergence_increases_with_cluster_size() {
        use super::super::seed_event::*;
        let event = SeedEvent {
            id: "test".into(),
            encounter_type: "contextual".into(),
            tone: "wonder".into(),
            category: "main_quest".into(),
            priority: 3,
            context_requirements: ContextRequirements::default(),
            text: "Test.".repeat(20),
            choices: vec![],
            intents: vec![],
            trigger: EventTrigger::default(),
            event_kind: EventKind::default(),
        };

        let mut journey_3 = test_journey(80.0, 0.9, 3);
        for i in 0..3 {
            journey_3.threads.push(Thread {
                id: Uuid::new_v4(),
                thread_type: ThreadType::Anomaly,
                associated_entities: vec![],
                tension: 0.5,
                created_at: Timestamp::zero(),
                last_touched: Timestamp::zero(),
                resolution: ResolutionState::Open,
                description: format!("Anomaly {}", i),
            });
        }

        let mut journey_5 = test_journey(80.0, 0.9, 3);
        for i in 0..5 {
            journey_5.threads.push(Thread {
                id: Uuid::new_v4(),
                thread_type: ThreadType::Anomaly,
                associated_entities: vec![],
                tension: 0.5,
                created_at: Timestamp::zero(),
                last_touched: Timestamp::zero(),
                resolution: ResolutionState::Open,
                description: format!("Anomaly {}", i),
            });
        }

        let score_3 = convergence_score(&event, &journey_3);
        let score_5 = convergence_score(&event, &journey_5);

        assert!(
            score_5 > score_3,
            "Larger clusters should produce higher convergence: 3={:.2}, 5={:.2}",
            score_3,
            score_5
        );
    }
}