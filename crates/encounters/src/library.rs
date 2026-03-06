// file: crates/encounters/src/library.rs
//! The seed library — hand-crafted gold-standard events.
//!
//! These serve three purposes:
//! 1. Playable content for Phase 1 (before the LLM)
//! 2. Few-shot examples for the LLM in Phase 4
//! 3. Fallback content when the API is unavailable
//!
//! Events are split into purpose-driven files under `data/events/`.
//! This module loads all of them via `include_str!` and concatenates
//! into a single `Vec<SeedEvent>`. The pipeline doesn't know or care
//! which file an event came from — convergence operates on threads,
//! not on file organization.

use super::seed_event::SeedEvent;

// Embed each category file at compile time.
// Paths are relative to this source file: crates/encounters/src/
const AMBIENT_JSON: &str = include_str!("../../../data/events/ambient.json");
const EXPLORATION_JSON: &str = include_str!("../../../data/events/exploration.json");
const FACTION_JSON: &str = include_str!("../../../data/events/faction.json");
const CREW_JSON: &str = include_str!("../../../data/events/crew.json");
const QUEST_MAIN_JSON: &str = include_str!("../../../data/events/quest_main.json");
const QUEST_SIDE_JSON: &str = include_str!("../../../data/events/quest_side.json");
const CONTRACTS_JSON: &str = include_str!("../../../data/events/contracts.json");

/// Load all seed events from the embedded category files.
///
/// Events from all files are merged into a single pool. The pipeline
/// selects from this pool based on context, scoring, and prerequisites.
/// File organization is for human readability only.
pub fn all_seed_events() -> Vec<SeedEvent> {
    let sources: &[(&str, &str)] = &[
        ("ambient", AMBIENT_JSON),
        ("exploration", EXPLORATION_JSON),
        ("faction", FACTION_JSON),
        ("crew", CREW_JSON),
        ("quest_main", QUEST_MAIN_JSON),
        ("quest_side", QUEST_SIDE_JSON),
        ("contracts", CONTRACTS_JSON),
    ];

    let mut all = Vec::new();

    for (name, json) in sources {
        let events: Vec<SeedEvent> = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("Failed to parse {}.json: {}", name, e));
        all.extend(events);
    }

    all
}

/// Load events from a single category file.
/// Useful for testing category-specific behavior.
pub fn events_by_category(category: &str) -> Vec<SeedEvent> {
    let json = match category {
        "ambient" => AMBIENT_JSON,
        "exploration" => EXPLORATION_JSON,
        "faction" => FACTION_JSON,
        "crew" => CREW_JSON,
        "main_quest" => QUEST_MAIN_JSON,
        "side_quest" => QUEST_SIDE_JSON,
        "contract" => CONTRACTS_JSON,
        _ => return Vec::new(),
    };
    serde_json::from_str(json)
        .unwrap_or_else(|e| panic!("Failed to parse {}.json: {}", category, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_events_load() {
        let events = all_seed_events();
        assert!(
            events.len() >= 8,
            "Should have at least 8 seed events, got {}",
            events.len()
        );
    }

    #[test]
    fn all_events_have_choices() {
        for event in all_seed_events() {
            assert!(
                !event.choices.is_empty(),
                "Event '{}' has no choices",
                event.id
            );
        }
    }

    #[test]
    fn all_choices_have_effects() {
        for event in all_seed_events() {
            for (i, choice) in event.choices.iter().enumerate() {
                assert!(
                    !choice.effects.is_empty(),
                    "Event '{}' choice {} ('{}') has no effects",
                    event.id,
                    i,
                    choice.label
                );
            }
        }
    }

    #[test]
    fn all_events_have_text() {
        for event in all_seed_events() {
            assert!(
                event.text.len() > 50,
                "Event '{}' text is too short — this is where tone lives",
                event.id
            );
        }
    }

    #[test]
    fn ids_are_unique() {
        let events = all_seed_events();
        let mut ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
        ids.sort();
        let unique_count = {
            let set: HashSet<&str> = ids.iter().copied().collect();
            set.len()
        };
        assert_eq!(
            unique_count,
            events.len(),
            "Duplicate event IDs found across category files"
        );
    }

    #[test]
    fn tone_coverage() {
        let events = all_seed_events();
        let tones: Vec<&str> = events.iter().map(|e| e.tone.as_str()).collect();
        // Minimal set — at least quiet and tense should always be present.
        for expected in &["quiet", "tense"] {
            assert!(tones.contains(expected), "No events with tone '{}'", expected);
        }
    }

    #[test]
    fn all_events_have_valid_category() {
        let valid = [
            "ambient",
            "exploration",
            "faction",
            "crew",
            "main_quest",
            "side_quest",
            "contract",
        ];
        for event in all_seed_events() {
            assert!(
                valid.contains(&event.category.as_str()),
                "Event '{}' has invalid category '{}'",
                event.id,
                event.category
            );
        }
    }

    #[test]
    fn priority_in_valid_range() {
        for event in all_seed_events() {
            assert!(
                event.priority <= 3,
                "Event '{}' has priority {} (max is 3)",
                event.id,
                event.priority
            );
        }
    }

    #[test]
    fn category_files_contain_matching_events() {
        for category in &["ambient", "exploration", "faction", "crew"] {
            let events = events_by_category(category);
            for event in &events {
                assert_eq!(
                    event.category.as_str(),
                    *category,
                    "Event '{}' in {}.json has category '{}'",
                    event.id,
                    category,
                    event.category
                );
            }
        }
    }
}