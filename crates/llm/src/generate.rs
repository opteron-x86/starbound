// file: crates/llm/src/generate.rs
//! Top-level encounter generation — the public API for the game layer.
//!
//! This is what the CLI calls. It assembles context, calls the LLM,
//! validates the response, and returns a SeedEvent the game can
//! process identically to a hand-crafted one.
//!
//! On failure (API error, parse error, validation error), returns None.
//! The caller falls back to the seed library.

use starbound_core::galaxy::StarSystem;
use starbound_core::journey::Journey;
use starbound_core::npc::Npc;

use starbound_encounters::seed_event::{EventTrigger, SeedEvent};

use crate::client::{self, ApiError};
use crate::config::LlmConfig;
use crate::prompt::{self, EncounterContext, DestinationInfo};
use crate::response;

/// Result of an LLM generation attempt.
pub struct GenerationResult {
    /// The generated event, if successful.
    pub event: SeedEvent,
    /// Tokens used (for cost tracking).
    pub tokens_used: Option<u32>,
}

/// Attempt to generate an encounter event via LLM.
///
/// Returns `None` if the LLM is unavailable, the API call fails,
/// or the response fails validation. The caller should fall back
/// to the seed library.
///
/// The `example_event` is included as a few-shot example in the
/// system prompt. Pick one from the seed library that matches the
/// trigger type.
pub fn generate_encounter(
    config: &LlmConfig,
    trigger: &EventTrigger,
    system: &StarSystem,
    journey: &Journey,
    npcs_here: Vec<&Npc>,
    location_name: Option<String>,
    location_type: Option<String>,
    location_description: Option<String>,
    faction_name: Option<String>,
    civ_name: Option<String>,
    recent_scenes: Vec<String>,
    established_facts: Vec<String>,
    destination: Option<DestinationInfo>,
    example_event: Option<&SeedEvent>,
    event_id: &str,
) -> Option<GenerationResult> {
    if !config.is_available() {
        return None;
    }

    // Assemble prompts.
    let system_msg = prompt::build_system_message(example_event);
    let ctx = EncounterContext {
        trigger,
        system,
        journey,
        npcs_here,
        location_name,
        location_type,
        location_description,
        faction_name,
        civ_name,
        recent_scenes,
        established_facts,
        destination,
    };
    let user_msg = prompt::build_user_message(&ctx);

    // Call the API — with one retry on transient failures.
    let mut last_error = String::new();
    let mut api_result = None;

    for attempt in 0..2 {
        match client::chat_completion(config, &system_msg, &user_msg) {
            Ok(result) => {
                api_result = Some(result);
                break;
            }
            Err(e) => {
                last_error = format!("{}", e);
                if attempt == 0 {
                    eprintln!("  [LLM] Attempt 1 failed: {}. Retrying...", e);
                }
            }
        }
    }

    let api_result = match api_result {
        Some(r) => r,
        None => {
            eprintln!("  [LLM] API error after retries: {}", last_error);
            return None;
        }
    };

    // Parse the response.
    let event = match response::parse_llm_response(&api_result.content, trigger, event_id) {
        Ok(event) => event,
        Err(e) => {
            eprintln!("  [LLM] Parse error: {}", e);
            eprintln!("  [LLM] Raw response: {}",
                &api_result.content[..api_result.content.len().min(300)]);
            return None;
        }
    };

    // Basic validation: event should have text and choices.
    if event.text.is_empty() || event.choices.is_empty() {
        eprintln!("  [LLM] Validation failed: empty text or choices");
        return None;
    }

    let tokens = api_result.usage.map(|u| u.total_tokens);

    Some(GenerationResult {
        event,
        tokens_used: tokens,
    })
}