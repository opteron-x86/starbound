// file: crates/encounters/src/pipeline.rs
//! The encounter pipeline — universal resolution system for both
//! things that happen TO the player and things the player INITIATES.
//!
//! ```text
//! intent_filter   → narrow to action   (Stage 0, player-initiated only)
//! context_filter  → candidate pool     (Day 4 matcher)
//! pressure_filter → boost situational  (lean into tensions)
//! echo_filter     → boost thread ties  (weave in the past)
//! novelty_check   → balance new vs old (meter fresh content)
//! tone_filter     → pacing bias        (alternate intensity)
//! reputation      → boost identity fit (player labels)
//! ```
//!
//! The pipeline scores candidates rather than eliminating them.
//! Every context-eligible event stays in the pool; filters adjust
//! weights so the final weighted-random selection naturally favors
//! the most narratively appropriate encounter. Sometimes the
//! pipeline returns silence — that's by design (but never for
//! player-initiated actions).

use std::fmt;

use rand::rngs::StdRng;
use rand::Rng;

use starbound_core::galaxy::StarSystem;
use starbound_core::journey::Journey;
use starbound_core::narrative::{Tone, ResolutionState};

use super::matcher::{match_events, MatchContext};
use super::seed_event::SeedEvent;

// ---------------------------------------------------------------------------
// Player intent — what action the player is initiating
// ---------------------------------------------------------------------------

/// A player-initiated action. When present, the pipeline selects from
/// events that can resolve this intent rather than from the full pool.
///
/// The string tag for each intent matches against `SeedEvent.intents`.
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
}

impl PlayerIntent {
    /// The string tag used to match against SeedEvent.intents.
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
        }
    }
}

impl fmt::Display for PlayerIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
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
    Silence {
        reason: String,
    },
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
/// When `intent` is `Some`, the pipeline runs in action mode:
/// - Stage 0 filters to events matching the intent
/// - Silence check is skipped (the player chose to act)
/// - If no matching events exist, returns silence with explanation
///
/// When `intent` is `None`, the pipeline runs in arrival mode
/// (original behavior — backwards compatible).
pub fn run_pipeline<'a>(
    events: &'a [SeedEvent],
    system: &StarSystem,
    journey: &Journey,
    galactic_years_since_last_visit: Option<f64>,
    state: &PipelineState,
    config: &PipelineConfig,
    rng: &mut StdRng,
    intent: Option<PlayerIntent>,
    location_type: Option<&str>,
) -> PipelineResult<'a> {
    // -----------------------------------------------------------------------
    // Stage 0 — Intent filter (player-initiated actions only)
    // -----------------------------------------------------------------------
    let intent_tag = intent.map(|i| i.tag());
    let is_player_action = intent.is_some();

    // Separate events into intent-matching and arrival-only pools.
    let working_events: Vec<&SeedEvent> = if let Some(tag) = intent_tag {
        // Player-initiated: only events that declare this intent.
        events
            .iter()
            .filter(|e| e.intents.iter().any(|i| i == tag))
            .collect()
    } else {
        // Arrival mode: only events with no intents (or all events if
        // we want arrival events to also fire intent events — but for
        // now, keep them separate for clean separation).
        events
            .iter()
            .filter(|e| e.intents.is_empty())
            .collect()
    };

    if working_events.is_empty() {
        return PipelineResult::Silence {
            reason: if let Some(i) = intent {
                format!("No events available for action: {}", i.label())
            } else {
                "No arrival events in library.".into()
            },
        };
    }

    // -----------------------------------------------------------------------
    // Silence check — skipped for player-initiated actions.
    // "A game that's always exciting is never exciting."
    // But when the player asks to do something, something should happen.
    // -----------------------------------------------------------------------
    if !is_player_action {
        let silence_threshold = config.silence_chance
            + (state.encounters_since_silence as f64 * config.silence_escalation);

        if rng.gen::<f64>() < silence_threshold {
            return PipelineResult::Silence {
                reason: format!(
                    "Silence after {} consecutive encounters (threshold {:.0}%)",
                    state.encounters_since_silence,
                    silence_threshold * 100.0,
                ),
            };
        }
    }

    // -----------------------------------------------------------------------
    // Stage 1 — Context filter (Day 4 matcher)
    // -----------------------------------------------------------------------
    let ctx = MatchContext {
        system,
        journey,
        galactic_years_since_last_visit,
        location_type: location_type.map(|s| s.to_string()),
    };

    // For intent mode, we need to intersect context-matched events with
    // the intent-filtered pool. match_events works on the full library,
    // so we filter its output against our working set.
    let all_context_matched = match_events(events, &ctx);

    let candidates: Vec<&SeedEvent> = if is_player_action {
        // Intersect: must be both context-appropriate AND intent-matching.
        let intent_ids: std::collections::HashSet<&str> =
            working_events.iter().map(|e| e.id.as_str()).collect();
        all_context_matched
            .into_iter()
            .filter(|e| intent_ids.contains(e.id.as_str()))
            .collect()
    } else {
        // Arrival mode: context filter only, excluding intent-only events.
        all_context_matched
            .into_iter()
            .filter(|e| e.intents.is_empty())
            .collect()
    };

    if candidates.is_empty() {
        // For player actions, provide a more helpful message.
        if let Some(i) = intent {
            return PipelineResult::Silence {
                reason: format!(
                    "No {} opportunities available at this system.",
                    i.label().to_lowercase(),
                ),
            };
        }
        return PipelineResult::Silence {
            reason: "No events match current context.".into(),
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
    // Stages 2–5: Score each candidate
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
            let rep_weight = journey.profile.encounter_weight(&event.context_requirements.tags);
            if (rep_weight - 1.0).abs() > 0.01 {
                weight *= rep_weight;
                reasons.push(format!("reputation ×{:.1}", rep_weight));
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
    if req.hull_below.is_some() {
        if journey.ship.hull_condition < 0.5 {
            score += 0.5;
        }
    }

    // Faction-controlled space with active grudge threads = pressure.
    if req.faction_controlled == Some(true) && !journey.threads.is_empty() {
        let grudge_threads = journey.threads.iter()
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

    let open_threads = journey.threads.iter()
        .filter(|t| t.resolution == ResolutionState::Open || t.resolution == ResolutionState::Partial)
        .count();

    // Events tagged with time-since-last-visit requirements are natural
    // echo candidates — they respond to the passage of time.
    let has_time_req = event.context_requirements
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
    let novelty: f64 = (1.0 - thread_count * 0.08).max(0.2);

    novelty
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

    let event_tone = parse_tone(&event.tone);

    // Count how many recent encounters share this tone.
    let same_count = recent_tones.iter()
        .filter(|t| **t == event_tone)
        .count();

    // Count intensity of recent encounters.
    let recent_intensity: f64 = recent_tones.iter()
        .map(|t| tone_intensity(t))
        .sum::<f64>() / recent_tones.len() as f64;

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

fn parse_tone(s: &str) -> Tone {
    match s {
        "tense" => Tone::Tense,
        "quiet" => Tone::Quiet,
        "wonder" => Tone::Wonder,
        "urgent" => Tone::Urgent,
        "melancholy" => Tone::Melancholy,
        "mundane" => Tone::Mundane,
        _ => Tone::Mundane,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::all_seed_events;
    use rand::SeedableRng;
    use std::collections::HashMap;
    use uuid::Uuid;
    use starbound_core::crew::*;
    use starbound_core::galaxy::*;
    use starbound_core::mission::*;
    use starbound_core::narrative::*;
    use starbound_core::ship::*;
    use starbound_core::time::Timestamp;
    use starbound_core::reputation::PlayerProfile;

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
                    security: 0.5, freedom: 0.5, purpose: 0.5,
                    connection: 0.5, knowledge: 0.5, justice: 0.5,
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
        // Use config with zero silence chance so we always get an event.
        let config = PipelineConfig {
            silence_chance: 0.0,
            silence_escalation: 0.0,
            ..Default::default()
        };
        let mut rng = StdRng::seed_from_u64(42);

        let result = run_pipeline(
            &events, &system, &journey, None, &state, &config, &mut rng,
            None,
            None,
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
    fn silence_can_occur() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let state = PipelineState::default();
        let config = PipelineConfig {
            silence_chance: 1.0, // Always silent
            ..Default::default()
        };
        let mut rng = StdRng::seed_from_u64(42);

        let result = run_pipeline(
            &events, &system, &journey, None, &state, &config, &mut rng,
            None,
            None,
        );

        assert!(matches!(result, PipelineResult::Silence { .. }));
    }

    #[test]
    fn recent_events_not_repeated() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let config = PipelineConfig {
            silence_chance: 0.0,
            silence_escalation: 0.0,
            ..Default::default()
        };

        // Fill recent_event_ids with all event IDs → should get silence.
        let mut state = PipelineState::default();
        state.recent_event_ids = events.iter().map(|e| e.id.clone()).collect();
        let mut rng = StdRng::seed_from_u64(42);

        let result = run_pipeline(
            &events, &system, &journey, None, &state, &config, &mut rng,
            None,
            None,
        );

        assert!(matches!(result, PipelineResult::Silence { .. }));
    }

    #[test]
    fn pressure_boosts_situational_events() {
        let events = all_seed_events();
        // Low fuel at a colony — fuel_merchant_desperate should be boosted.
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(15.0, 0.9, 3);
        let config = PipelineConfig {
            silence_chance: 0.0,
            silence_escalation: 0.0,
            pressure_boost: 10.0, // Cranked up so pressure dominates
            ..Default::default()
        };
        // Run many times and count how often fuel event fires.
        let mut fuel_count = 0;
        let trials = 100;
        for seed in 0..trials {
            let mut rng = StdRng::seed_from_u64(seed);
            let result = run_pipeline(
                &events, &system, &journey, None,
                &PipelineState::default(), &config, &mut rng,
                None,
                None,
            );
            if let PipelineResult::Event { event, .. } = result {
                if event.id == "fuel_merchant_desperate" {
                    fuel_count += 1;
                }
            }
        }

        // With heavy pressure boost, fuel event should fire often (but not always,
        // because other events also match this context).
        assert!(fuel_count > 20,
            "fuel_merchant_desperate should fire frequently with pressure boost, got {}/{}",
            fuel_count, trials);
    }

    #[test]
    fn pacing_alternates_intensity() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let config = PipelineConfig {
            silence_chance: 0.0,
            silence_escalation: 0.0,
            pacing_contrast_boost: 10.0, // Cranked so pacing dominates
            ..Default::default()
        };

        // After several tense encounters, quiet/mundane should be boosted.
        let mut state = PipelineState::default();
        state.recent_tones = vec![Tone::Tense, Tone::Tense, Tone::Urgent, Tone::Tense];

        let mut quiet_count = 0;
        let trials = 100;
        for seed in 0..trials {
            let mut rng = StdRng::seed_from_u64(seed);
            let result = run_pipeline(
                &events, &system, &journey, None, &state, &config, &mut rng,
                None,
                None,
            );
            if let PipelineResult::Event { event, .. } = result {
                let tone = parse_tone(&event.tone);
                if tone == Tone::Quiet || tone == Tone::Mundane {
                    quiet_count += 1;
                }
            }
        }

        assert!(quiet_count > 30,
            "After intense encounters, quiet/mundane should be boosted, got {}/{}",
            quiet_count, trials);
    }

    #[test]
    fn novelty_decreases_with_threads() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Outpost, None);
        let config = PipelineConfig {
            silence_chance: 0.0,
            silence_escalation: 0.0,
            novelty_base_boost: 10.0,
            ..Default::default()
        };

        // Few threads → high novelty boost.
        let journey_early = test_journey(80.0, 0.9, 3);

        let mut journey_late = test_journey(80.0, 0.9, 3);
        add_threads(&mut journey_late, 10);

        let mut novel_early = 0;
        let mut novel_late = 0;
        let trials = 100;

        for seed in 0..trials {
            let mut rng = StdRng::seed_from_u64(seed);
            if let PipelineResult::Event { event, .. } = run_pipeline(
                &events, &system, &journey_early, None,
                &PipelineState::default(), &config, &mut rng,
                None,
                None,
            ) {
                if event.encounter_type == "novel" { novel_early += 1; }
            }

            let mut rng = StdRng::seed_from_u64(seed);
            if let PipelineResult::Event { event, .. } = run_pipeline(
                &events, &system, &journey_late, None,
                &PipelineState::default(), &config, &mut rng,
                None,
                None,
            ) {
                if event.encounter_type == "novel" { novel_late += 1; }
            }
        }

        assert!(novel_early > novel_late,
            "Novel events should fire more often early ({}) than late ({})",
            novel_early, novel_late);
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
        // Tones and IDs preserved across silence.
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

        let r1 = run_pipeline(&events, &system, &journey, None, &state, &config, &mut rng1, None, None);
        let r2 = run_pipeline(&events, &system, &journey, None, &state, &config, &mut rng2, None, None);

        match (r1, r2) {
            (PipelineResult::Event { event: e1, .. }, PipelineResult::Event { event: e2, .. }) => {
                assert_eq!(e1.id, e2.id, "Same seed should produce same event");
            }
            (PipelineResult::Silence { .. }, PipelineResult::Silence { .. }) => {
                // Both silent — deterministic.
            }
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

        // After 0 encounters: 10% silence.
        // After 5 encounters: 60% silence.
        // After 9 encounters: 100% silence.
        let mut state_stale = PipelineState::default();
        state_stale.encounters_since_silence = 9;

        let mut rng = StdRng::seed_from_u64(42);
        let result = run_pipeline(
            &events, &system, &journey, None, &state_stale, &config, &mut rng,
            None,
            None,
        );

        // With 100% silence chance, this must be silent.
        assert!(matches!(result, PipelineResult::Silence { .. }),
            "Should be silent after 9 consecutive encounters with escalation");
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

        // Run with Trade intent — should only select trade events.
        let mut found_trade = false;
        for seed in 0..50 {
            let mut rng = StdRng::seed_from_u64(seed);
            if let PipelineResult::Event { event, .. } = run_pipeline(
                &events, &system, &journey, None,
                &PipelineState::default(), &config, &mut rng,
                Some(PlayerIntent::Trade),
                None,
            ) {
                assert!(
                    event.intents.contains(&"trade".to_string()),
                    "Intent mode should only select trade events, got: {}",
                    event.id,
                );
                found_trade = true;
            }
        }
        assert!(found_trade, "Should have found at least one trade event");
    }

    #[test]
    fn intent_mode_skips_silence() {
        let events = all_seed_events();
        let system = test_system(InfrastructureLevel::Colony, Some(Uuid::new_v4()));
        let journey = test_journey(80.0, 0.9, 3);
        let config = PipelineConfig {
            silence_chance: 1.0, // 100% silence — would always be silent in arrival mode
            silence_escalation: 0.0,
            ..Default::default()
        };

        let mut rng = StdRng::seed_from_u64(42);
        let result = run_pipeline(
            &events, &system, &journey, None,
            &PipelineState::default(), &config, &mut rng,
            Some(PlayerIntent::Trade),
            None,
        );

        // Intent mode should NOT be silenced even with 100% silence chance.
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

        // Run in arrival mode many times — should never get an intent event.
        for seed in 0..50 {
            let mut rng = StdRng::seed_from_u64(seed);
            if let PipelineResult::Event { event, .. } = run_pipeline(
                &events, &system, &journey, None,
                &PipelineState::default(), &config, &mut rng,
                None,
                None,
            ) {
                assert!(
                    event.intents.is_empty(),
                    "Arrival mode should not select intent events, got: {} (intents: {:?})",
                    event.id, event.intents,
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

        // Recruit intent — no events for this yet.
        let mut rng = StdRng::seed_from_u64(42);
        let result = run_pipeline(
            &events, &system, &journey, None,
            &PipelineState::default(), &config, &mut rng,
            Some(PlayerIntent::Recruit),
            None,
        );

        assert!(
            matches!(result, PipelineResult::Silence { .. }),
            "Intent with no matching events should return silence",
        );
    }
}