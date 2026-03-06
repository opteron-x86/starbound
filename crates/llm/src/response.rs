// file: crates/llm/src/response.rs
//! Parse LLM responses into SeedEvent structures.
//!
//! The LLM returns JSON with text and choices. This module parses it,
//! validates the effects against the known vocabulary, and produces
//! a SeedEvent that the game can process identically to a hand-crafted one.

use starbound_encounters::seed_event::{
    SeedEvent, SeedChoice, EffectDef, EventTrigger, EventKind,
    ContextRequirements,
};

/// Intermediate format — what the LLM produces.
/// Less strict than SeedEvent; we validate and convert.
#[derive(Debug, serde::Deserialize)]
struct RawLlmEvent {
    text: String,
    choices: Vec<RawLlmChoice>,
}

#[derive(Debug, serde::Deserialize)]
struct RawLlmChoice {
    label: String,
    #[serde(default)]
    tone_note: String,
    #[serde(default)]
    effects: Vec<serde_json::Value>,
}

/// Errors that can occur during response parsing.
#[derive(Debug)]
pub enum ParseError {
    /// Response wasn't valid JSON.
    InvalidJson(String),
    /// JSON structure didn't match expected format.
    MissingField(String),
    /// Text was empty or too short.
    TextTooShort,
    /// No choices provided.
    NoChoices,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidJson(e) => write!(f, "Invalid JSON: {}", e),
            ParseError::MissingField(field) => write!(f, "Missing field: {}", field),
            ParseError::TextTooShort => write!(f, "Event text too short"),
            ParseError::NoChoices => write!(f, "No choices in response"),
        }
    }
}

/// Parse an LLM response string into a SeedEvent.
///
/// The event gets a generated ID, default trigger/kind based on what
/// was requested, and validated effects.
pub fn parse_llm_response(
    raw: &str,
    trigger: &EventTrigger,
    event_id: &str,
) -> Result<SeedEvent, ParseError> {
    // Strip markdown code fences if the LLM wrapped its response.
    let trimmed = raw.trim();
    let cleaned = if trimmed.starts_with("```json") {
        trimmed.strip_prefix("```json").unwrap()
            .strip_suffix("```").unwrap_or(trimmed)
            .trim()
    } else if trimmed.starts_with("```") {
        trimmed.strip_prefix("```").unwrap()
            .strip_suffix("```").unwrap_or(trimmed)
            .trim()
    } else {
        trimmed
    };

    let parsed: RawLlmEvent = serde_json::from_str(cleaned)
        .map_err(|e| {
            // Detect likely truncation — JSON cut off mid-stream.
            if !cleaned.ends_with('}') {
                ParseError::InvalidJson(format!(
                    "Response appears truncated (doesn't end with '}}').  \
                     This usually means max_tokens is too low. Error: {}", e
                ))
            } else {
                ParseError::InvalidJson(e.to_string())
            }
        })?;

    // Validate basics.
    if parsed.text.len() < 50 {
        return Err(ParseError::TextTooShort);
    }
    if parsed.choices.is_empty() {
        return Err(ParseError::NoChoices);
    }

    // Convert choices, validating effects.
    let choices: Vec<SeedChoice> = parsed.choices.into_iter().map(|raw_choice| {
        let effects: Vec<EffectDef> = raw_choice.effects.into_iter()
            .filter_map(|v| parse_effect(v))
            .collect();

        // Ensure at least a pass effect if nothing validated.
        let effects = if effects.is_empty() {
            vec![EffectDef::Pass {}]
        } else {
            effects
        };

        SeedChoice {
            label: raw_choice.label,
            effects,
            tone_note: raw_choice.tone_note,
            follows: None,
        }
    }).collect();

    // Determine event kind from trigger.
    let event_kind = if trigger.is_player_action() {
        EventKind::Discovery
    } else {
        EventKind::Ambient
    };

    // Determine tone from trigger (simple heuristic).
    let tone = if trigger.is_player_action() {
        "wonder".into()
    } else {
        "quiet".into()
    };

    Ok(SeedEvent {
        id: event_id.to_string(),
        encounter_type: "llm_generated".into(),
        tone,
        category: "ambient".into(),
        priority: 0,
        context_requirements: ContextRequirements::default(),
        text: parsed.text,
        choices,
        intents: vec![],
        trigger: trigger.clone(),
        event_kind,
    })
}

/// Try to parse a JSON Value into a known EffectDef.
/// Returns None for unrecognized or malformed effects — silently drops them.
fn parse_effect(value: serde_json::Value) -> Option<EffectDef> {
    // Try to deserialize directly — EffectDef has serde support.
    // This works for all known effect types.
    serde_json::from_value::<EffectDef>(value.clone()).ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_response() {
        let json = r#"{
            "text": "The station hums with activity. Through the viewport, cargo shuttles trace slow arcs between the docking rings. Someone has taped a handwritten note to the airlock: 'Welcome back.'",
            "choices": [
                {
                    "label": "Read the note",
                    "tone_note": "Curiosity",
                    "effects": [
                        {"type": "narrative", "text": "Read a welcome note on the airlock."}
                    ]
                },
                {
                    "label": "Head inside",
                    "tone_note": "Practical",
                    "effects": [
                        {"type": "pass"}
                    ]
                }
            ]
        }"#;

        let event = parse_llm_response(json, &EventTrigger::Docked, "llm_test_001").unwrap();
        assert_eq!(event.id, "llm_test_001");
        assert_eq!(event.choices.len(), 2);
        assert_eq!(event.event_kind, EventKind::Ambient);
        assert!(event.text.contains("station hums"));
    }

    #[test]
    fn strips_markdown_fences() {
        let json = "```json\n{\"text\": \"Test encounter with enough text to pass the length check. The station is quiet today, unusually so.\", \"choices\": [{\"label\": \"Look around\", \"effects\": [{\"type\": \"pass\"}]}]}\n```";
        let event = parse_llm_response(json, &EventTrigger::Arrival, "llm_test_002").unwrap();
        assert!(event.text.contains("Test encounter"));
    }

    #[test]
    fn rejects_short_text() {
        let json = r#"{"text": "Short.", "choices": [{"label": "Ok", "effects": [{"type": "pass"}]}]}"#;
        assert!(matches!(
            parse_llm_response(json, &EventTrigger::Arrival, "test"),
            Err(ParseError::TextTooShort)
        ));
    }

    #[test]
    fn rejects_no_choices() {
        let json = r#"{"text": "Long enough text for the minimum check. The station looms ahead, its running lights cycling through their pattern.", "choices": []}"#;
        assert!(matches!(
            parse_llm_response(json, &EventTrigger::Arrival, "test"),
            Err(ParseError::NoChoices)
        ));
    }

    #[test]
    fn unknown_effects_dropped_gracefully() {
        let json = r#"{
            "text": "The drive hums. Your engineer listens carefully, head tilted. Something has changed in the harmonics since the last repair.",
            "choices": [
                {
                    "label": "Ask about it",
                    "effects": [
                        {"type": "narrative", "text": "Asked the engineer."},
                        {"type": "totally_fake_effect", "power": 9000},
                        {"type": "crew_stress", "delta": -0.02}
                    ]
                }
            ]
        }"#;

        let event = parse_llm_response(json, &EventTrigger::Transit, "llm_test_003").unwrap();
        // The fake effect should be dropped, leaving narrative + crew_stress.
        assert_eq!(event.choices[0].effects.len(), 2);
    }

    #[test]
    fn action_trigger_produces_discovery_kind() {
        let json = r#"{
            "text": "Your sensors resolve something in the static. A structure, ancient, turning slowly in the void. Not natural. The readings fold back on themselves.",
            "choices": [
                {
                    "label": "Investigate",
                    "effects": [{"type": "spawn_thread", "thread_type": "anomaly", "description": "Found an ancient structure."}]
                }
            ]
        }"#;

        let event = parse_llm_response(
            json,
            &EventTrigger::Action("investigate".into()),
            "llm_test_004",
        ).unwrap();
        assert_eq!(event.event_kind, EventKind::Discovery);
    }
}