// file: crates/llm/src/prompt.rs
//! Prompt assembly — builds structured prompts from game state.
//!
//! The LLM receives two messages:
//! - **System message**: role definition, style bible, output format spec,
//!   effect vocabulary, and a few-shot example.
//! - **User message**: current game context (system, ship, crew, threads,
//!   trigger type, location) assembled from live state.
//!
//! The LLM's job: take the context and produce an encounter event as JSON.
//! The game systems decide what happens. The LLM decides how it reads.

use starbound_core::galaxy::StarSystem;
use starbound_core::journey::Journey;
use starbound_core::npc::Npc;

use starbound_encounters::seed_event::{EventTrigger, SeedEvent};

/// Style bible text — embedded from the TOML, pre-formatted for prompts.
const STYLE_BIBLE: &str = "\
VOICE: Awe tinged with loneliness. The feeling between wonder and melancholy. \
Quiet and restrained. Trust the player to feel things without being told to feel them.

DESCRIPTIONS: Evocative but incomplete. Describe what you perceive, not what things are. \
DO: 'The structure fills your viewport and you still can't see the edges.' \
DON'T: 'The structure is enormous.'

DIALOGUE: Sounds like actual people. Trailing off mid-thought. Mundane observations that \
carry weight. Important things buried in casual conversation.

PACING: A slow-burn film you're directing. Not every moment needs to be dramatic. \
Quiet observation is valid. Silence between characters is valid.

TEST: When a player lies in bed after playing, are they thinking about loot and levels, \
or about their navigator's face when she realized the colony was gone?";

/// The output format specification included in the system prompt.
const OUTPUT_FORMAT: &str = r#"
You must respond with ONLY a JSON object. No markdown, no preamble, no explanation.

The JSON must have this exact structure:
{
  "text": "The encounter text the player reads. Paragraphs separated by \\n\\n.",
  "choices": [
    {
      "label": "Short action label (what the player does)",
      "tone_note": "Brief note on the emotional register of this choice",
      "effects": [
        {"type": "effect_type", ...effect_fields}
      ]
    }
  ]
}

LENGTH RULES:
- Transit/ambient events: 1-2 short paragraphs. A moment, not a scene.
- Docked/arrival events: 1-3 paragraphs. Set the atmosphere, don't overdo it.
- Action/discovery events: 2-4 paragraphs. These earn more space.
- NOT EVERY DETAIL NEEDS TO BE USED. Pick one or two elements from the context.
- Silence and brevity are powerful. A three-sentence encounter can hit harder than six paragraphs.
- Keep text under 400 words. Most events should be under 200.

CHOICE RULES:
- 2-3 choices. Each must feel meaningfully different.
- At least one choice should be low-commitment (observe, move on, say nothing).
- Choice labels should be short — under 10 words.

EFFECT RULES — use ONLY these types:
  {"type": "narrative", "text": "..."}  — log note, no mechanical change
  {"type": "crew_stress", "delta": 0.03}  — positive adds stress, negative reduces
  {"type": "trust_professional", "delta": 0.02}  — crew trust shift
  {"type": "trust_personal", "delta": 0.02}  — crew trust shift
  {"type": "spawn_thread", "thread_type": "mystery|anomaly|relationship|grudge|debt", "description": "..."}
  {"type": "fuel", "delta": -5.0}  — fuel change
  {"type": "supplies", "delta": -3.0}  — supplies change
  {"type": "hull", "delta": -0.05}  — hull condition change
  {"type": "resources", "delta": 50.0}  — credits change
  {"type": "time_cost", "hours": 4.0}  — personal time consumed
  {"type": "pass"}  — no mechanical effect
- Do NOT invent new effect types.
- Keep deltas small. Stress: ±0.01-0.05. Trust: ±0.01-0.04. Resources: ±10-100.
- Most ambient events should use only "narrative" or "pass" effects.
"#;

/// Build the system message — role, style, format, example.
pub fn build_system_message(example_event: Option<&SeedEvent>) -> String {
    let mut msg = String::with_capacity(4000);

    msg.push_str("You are the narrative engine for Starbound, a space exploration game. ");
    msg.push_str("You generate encounter events based on the game context you receive. ");
    msg.push_str("The game's simulation decides WHAT happens. You decide HOW it reads.\n\n");

    msg.push_str("=== STYLE BIBLE ===\n");
    msg.push_str(STYLE_BIBLE);
    msg.push_str("\n\n");

    msg.push_str("=== OUTPUT FORMAT ===\n");
    msg.push_str(OUTPUT_FORMAT);

    // Include a few-shot example if available.
    if let Some(event) = example_event {
        msg.push_str("\n=== EXAMPLE EVENT ===\n");
        if let Ok(json) = serde_json::to_string_pretty(event) {
            // Trim to just text + choices for the example.
            msg.push_str(&json);
        }
        msg.push_str("\n");
    }

    msg
}

/// Context package assembled from live game state.
/// Serialized into the user message for the LLM.
pub struct EncounterContext<'a> {
    pub trigger: &'a EventTrigger,
    pub system: &'a StarSystem,
    pub journey: &'a Journey,
    pub npcs_here: Vec<&'a Npc>,
    pub location_name: Option<String>,
    pub location_type: Option<String>,
    pub location_description: Option<String>,
    pub faction_name: Option<String>,
    pub civ_name: Option<String>,
    /// Recent scene summaries — what happened in the last 2-3 encounters.
    /// Most recent last. Prevents re-introductions and contradictions.
    pub recent_scenes: Vec<String>,
    /// Established facts about this location that must not be contradicted.
    /// NPCs met, physical details, faction presence, what's NOT here.
    pub established_facts: Vec<String>,
}

/// Build the user message — game context for this specific encounter.
pub fn build_user_message(ctx: &EncounterContext) -> String {
    let mut msg = String::with_capacity(4000);

    msg.push_str("Generate an encounter event for the following context.\n\n");

    // --- Established facts (non-negotiable constraints) ---
    if !ctx.established_facts.is_empty() {
        msg.push_str("=== ESTABLISHED FACTS (do not contradict) ===\n");
        for fact in &ctx.established_facts {
            msg.push_str(&format!("- {}\n", fact));
        }
        msg.push_str("\n");
    }

    // --- Recent scene memory ---
    if !ctx.recent_scenes.is_empty() {
        msg.push_str("=== RECENT EVENTS (what just happened — do not repeat or re-introduce) ===\n");
        for scene in &ctx.recent_scenes {
            msg.push_str(&format!("- {}\n", scene));
        }
        msg.push_str("IMPORTANT: Do NOT re-introduce characters the player has already met. ");
        msg.push_str("Do NOT repeat scenes or information from recent events. ");
        msg.push_str("Build on what happened, don't restart it.\n\n");
    }

    // Trigger type
    msg.push_str(&format!("TRIGGER: {}\n", ctx.trigger.label()));
    match ctx.trigger {
        EventTrigger::Transit => {
            msg.push_str("This is an ambient moment during sublight travel between locations. ");
            msg.push_str("Keep it small — a crew observation, environmental detail, or quiet moment. ");
            msg.push_str("Low stakes. No major plot developments.\n");
        }
        EventTrigger::Docked => {
            msg.push_str("The player just docked at a station or landed at a colony. ");
            msg.push_str("An atmospheric moment — station life, colony culture, sensory transition. ");
            msg.push_str("Low stakes. Environmental texture.\n");
        }
        EventTrigger::Arrival => {
            msg.push_str("The player has arrived at a location. ");
            msg.push_str("This could be atmospheric or consequential. Match the tone to the context.\n");
        }
        EventTrigger::Linger => {
            msg.push_str("The player is spending time at a location. ");
            msg.push_str("Something they notice while lingering. Moderate stakes.\n");
        }
        EventTrigger::Action(tag) => {
            msg.push_str(&format!(
                "The player chose to {}. This is a discovery event — they acted, something should happen. ",
                tag
            ));
            msg.push_str("Meaningful stakes. Consequences. Give them something to work with.\n");
        }
    }

    // System
    msg.push_str(&format!("\nSYSTEM: {}\n", ctx.system.name));
    msg.push_str(&format!("  Star: {}\n", ctx.system.star_type));
    msg.push_str(&format!("  Infrastructure: {}\n", ctx.system.infrastructure_level));
    msg.push_str(&format!("  Time factor: {:.1}x\n", ctx.system.time_factor));

    if let Some(ref civ) = ctx.civ_name {
        msg.push_str(&format!("  Controlled by: {}\n", civ));
    } else {
        msg.push_str("  Unclaimed space\n");
    }

    if let Some(ref faction) = ctx.faction_name {
        msg.push_str(&format!("  Dominant faction: {}\n", faction));
    }

    // Location — now with richer description
    if let Some(ref loc_name) = ctx.location_name {
        msg.push_str(&format!("\nLOCATION: {}", loc_name));
        if let Some(ref loc_type) = ctx.location_type {
            msg.push_str(&format!(" ({})", loc_type));
        }
        msg.push_str("\n");
        if let Some(ref desc) = ctx.location_description {
            msg.push_str(&format!("  Description: {}\n", desc));
        }
    }

    // Ship state
    let ship = &ctx.journey.ship;
    msg.push_str(&format!(
        "\nSHIP: {}\n  Hull: {:.0}%  Fuel: {:.0}/{:.0}  Supplies: {:.0}/{:.0}  Credits: {:.0}\n",
        ship.name,
        ship.hull_condition * 100.0,
        ship.fuel, ship.fuel_capacity,
        ship.supplies, ship.supply_capacity,
        ctx.journey.resources,
    ));

    // Crew
    if !ctx.journey.crew.is_empty() {
        msg.push_str(&format!("\nCREW ({}):\n", ctx.journey.crew.len()));
        for member in &ctx.journey.crew {
            msg.push_str(&format!(
                "  {} — {} (stress: {:.0}%, mood: {})\n",
                member.name, member.role,
                member.state.stress * 100.0,
                member.state.mood,
            ));
        }
    }

    // NPCs at this location — with interaction history
    if !ctx.npcs_here.is_empty() {
        msg.push_str(&format!("\nNPCs HERE ({}):\n", ctx.npcs_here.len()));
        for npc in &ctx.npcs_here {
            msg.push_str(&format!(
                "  {} — {} (disposition: {}, personality: {})\n",
                npc.name, npc.title,
                npc.disposition_tier(),
                npc.personality.dominant_description(),
            ));
            // Include last interaction if any.
            if let Some(last) = npc.last_interaction() {
                msg.push_str(&format!(
                    "    Last interaction: {}\n", last.summary
                ));
            }
        }
    }

    // Active threads
    if !ctx.journey.threads.is_empty() {
        let open_threads: Vec<_> = ctx.journey.threads.iter()
            .filter(|t| {
                t.resolution == starbound_core::narrative::ResolutionState::Open
                    || t.resolution == starbound_core::narrative::ResolutionState::Partial
            })
            .collect();
        if !open_threads.is_empty() {
            msg.push_str(&format!("\nACTIVE THREADS ({}):\n", open_threads.len()));
            for thread in open_threads.iter().take(5) {
                msg.push_str(&format!(
                    "  [{}] {}\n",
                    thread.thread_type, thread.description,
                ));
            }
            if open_threads.len() > 5 {
                msg.push_str(&format!("  ... and {} more\n", open_threads.len() - 5));
            }
        }
    }

    // Time context
    let personal_months = ctx.journey.time.personal_days / 30.44;
    let galactic_years = ctx.journey.time.galactic_days / 365.25;
    msg.push_str(&format!(
        "\nTIME: {:.1} months personal / {:.1} years galactic\n",
        personal_months, galactic_years,
    ));

    msg.push_str("\nRespond with ONLY the JSON object. No other text.\n");

    msg
}