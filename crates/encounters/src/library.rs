// file: crates/encounters/src/library.rs
//! The seed library — hand-crafted gold-standard events.
//!
//! These serve three purposes:
//! 1. Playable content for Phase 1 (before the LLM)
//! 2. Few-shot examples for the LLM in Phase 4
//! 3. Fallback content when the API is unavailable
//!
//! Each event is written in the game's voice: quiet, restrained,
//! evocative but incomplete. Trust the player to feel things.

use super::seed_event::SeedEvent;

/// Load all seed events from the embedded library.
pub fn all_seed_events() -> Vec<SeedEvent> {
    let raw = include_str!("seed_events.json");
    serde_json::from_str(raw).expect("Embedded seed events should be valid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_events_load() {
        let events = all_seed_events();
        assert!(events.len() >= 35, "Should have at least 35 seed events, got {}", events.len());
    }

    #[test]
    fn all_events_have_choices() {
        for event in all_seed_events() {
            assert!(!event.choices.is_empty(),
                "Event '{}' has no choices", event.id);
        }
    }

    #[test]
    fn all_choices_have_effects() {
        for event in all_seed_events() {
            for (i, choice) in event.choices.iter().enumerate() {
                assert!(!choice.effects.is_empty(),
                    "Event '{}' choice {} ('{}') has no effects",
                    event.id, i, choice.label);
            }
        }
    }

    #[test]
    fn all_events_have_text() {
        for event in all_seed_events() {
            assert!(event.text.len() > 50,
                "Event '{}' text is too short — this is where tone lives", event.id);
        }
    }

    #[test]
    fn ids_are_unique() {
        let events = all_seed_events();
        let mut ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), events.len(), "Duplicate event IDs found");
    }

    #[test]
    fn tone_coverage() {
        let events = all_seed_events();
        let tones: Vec<&str> = events.iter().map(|e| e.tone.as_str()).collect();
        for expected in &["quiet", "tense", "melancholy", "wonder", "mundane", "urgent"] {
            assert!(tones.contains(expected),
                "No events with tone '{}'", expected);
        }
    }
}