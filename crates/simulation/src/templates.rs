// file: crates/simulation/src/templates.rs
//! Template data for procedural galaxy generation.
//!
//! Deserializes `data/templates/civilizations.json` and `data/templates/factions.json`
//! into typed Rust structs. Templates are embedded at compile time via `include_str!`
//! so the binary is self-contained.

use serde::Deserialize;
use std::collections::HashMap;

// ===========================================================================
// Civilization templates
// ===========================================================================

/// Top-level structure for `civilizations.json`.
#[derive(Debug, Deserialize)]
pub struct CivTemplates {
    pub generation_rules: CivGenerationRules,
    pub prefixes: Vec<CivPrefix>,
    pub suffixes: Vec<CivSuffix>,
    pub compatibility: CivCompatibility,
    pub initial_state_ranges: InitialStateRanges,
}

#[derive(Debug, Deserialize)]
pub struct CivGenerationRules {
    pub min_count: usize,
    pub max_count: usize,
    pub default_count: usize,
    pub ethos_noise: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CivPrefix {
    pub name: String,
    pub weight: f64,
    /// Ethos axis name → bias value added on top of the suffix base.
    /// Empty map means no bias.
    #[serde(default)]
    pub ethos_bias: HashMap<String, f32>,
    #[serde(default)]
    pub flavor: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CivSuffix {
    pub name: String,
    pub weight: f64,
    /// Full ethos weight map — the primary personality driver.
    pub ethos_weights: HashMap<String, f32>,
    #[serde(default)]
    pub flavor: Vec<String>,
    pub government_style: String,
}

#[derive(Debug, Deserialize)]
pub struct CivCompatibility {
    /// Prefix name → list of incompatible suffix names.
    pub blocked_pairs: HashMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct InitialStateRanges {
    pub stability: [f32; 2],
    pub growth_rate: [f32; 2],
    pub military_capability: [f32; 2],
    pub economic_output: [f32; 2],
    pub tech_level: [f32; 2],
}

// ===========================================================================
// Faction templates
// ===========================================================================

/// Top-level structure for `factions.json`.
#[derive(Debug, Deserialize)]
pub struct FactionTemplates {
    pub generation_rules: FactionGenerationRules,
    pub categories: HashMap<String, FactionCategoryTemplate>,
}

#[derive(Debug, Deserialize)]
pub struct FactionGenerationRules {
    pub guaranteed: Vec<String>,
    pub optional: Vec<OptionalFactionRule>,
    pub max_factions: usize,
}

#[derive(Debug, Deserialize)]
pub struct OptionalFactionRule {
    pub category: String,
    pub chance: f64,
}

#[derive(Debug, Deserialize)]
pub struct FactionCategoryTemplate {
    pub scope: String,
    pub base_traits: FactionBaseTraits,
    pub default_services: Vec<String>,
    pub visibility_range: [f32; 2],
    pub name_patterns: Vec<String>,
    /// Slot name → list of components that can fill that slot.
    pub components: HashMap<String, Vec<FactionComponent>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FactionBaseTraits {
    pub alignment: f32,
    pub aggression: f32,
    pub openness: f32,
    pub secrecy: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FactionComponent {
    pub name: String,
    pub weight: f64,
    #[serde(default)]
    pub trait_bias: HashMap<String, f32>,
    #[serde(default)]
    pub flavor: Vec<String>,
}

// ===========================================================================
// Loading
// ===========================================================================

const CIV_TEMPLATES_JSON: &str =
    include_str!("../../../data/templates/civilizations.json");

const FACTION_TEMPLATES_JSON: &str =
    include_str!("../../../data/templates/factions.json");

const SYSTEM_TEMPLATES_JSON: &str =
    include_str!("../../../data/templates/star_systems.json");

/// Load and deserialize civilization templates from the embedded JSON.
pub fn load_civ_templates() -> CivTemplates {
    serde_json::from_str(CIV_TEMPLATES_JSON)
        .expect("civilizations.json should be valid — this is a compile-time embed")
}

/// Load and deserialize faction templates from the embedded JSON.
pub fn load_faction_templates() -> FactionTemplates {
    serde_json::from_str(FACTION_TEMPLATES_JSON)
        .expect("factions.json should be valid — this is a compile-time embed")
}

/// Load and deserialize star system templates from the embedded JSON.
pub fn load_system_templates() -> SystemTemplates {
    serde_json::from_str(SYSTEM_TEMPLATES_JSON)
        .expect("star_systems.json should be valid — this is a compile-time embed")
}

// ===========================================================================
// Star system templates
// ===========================================================================

/// Top-level structure for `star_systems.json`.
#[derive(Debug, Deserialize)]
pub struct SystemTemplates {
    pub generation_rules: SystemGenerationRules,
    pub star_types: Vec<StarTypeWeight>,
    pub standalone_names: Vec<NameEntry>,
    pub compound_prefixes: Vec<NameEntry>,
    pub compound_suffixes: Vec<NameEntry>,
    pub explorer_surnames: Vec<WeightedName>,
    pub explorer_suffixes: Vec<WeightedName>,
}

#[derive(Debug, Deserialize)]
pub struct SystemGenerationRules {
    pub system_count: usize,
    pub min_systems: usize,
    pub max_systems: usize,
    pub spatial_spread: f64,
    pub min_distance_between_systems: f64,
    pub connection_threshold_ly: f64,
    pub time_factor_frontier_min: f64,
    pub time_factor_frontier_max: f64,
    pub time_factor_deep_frontier_min: f64,
    pub time_factor_deep_frontier_max: f64,
    pub unclaimed_fraction: f64,
}

#[derive(Debug, Deserialize)]
pub struct StarTypeWeight {
    #[serde(rename = "type")]
    pub star_type: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NameEntry {
    pub name: String,
    pub weight: f64,
    #[serde(default)]
    pub flavor: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WeightedName {
    pub name: String,
    pub weight: f64,
}

// ===========================================================================
// Helpers (used by the generator in the next phase)
// ===========================================================================

impl CivCompatibility {
    /// Returns true if the given prefix+suffix combination is blocked.
    pub fn is_blocked(&self, prefix: &str, suffix: &str) -> bool {
        self.blocked_pairs
            .get(prefix)
            .map_or(false, |blocked| blocked.iter().any(|s| s == suffix))
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Civilization template tests
    // -----------------------------------------------------------------------

    #[test]
    fn civ_templates_deserialize() {
        let t = load_civ_templates();
        // Verify we got the expected counts from the JSON.
        assert_eq!(t.prefixes.len(), 17, "Expected 17 prefixes");
        assert_eq!(t.suffixes.len(), 12, "Expected 12 suffixes");
    }

    #[test]
    fn civ_generation_rules_are_sensible() {
        let t = load_civ_templates();
        assert!(t.generation_rules.min_count >= 1);
        assert!(t.generation_rules.max_count >= t.generation_rules.min_count);
        assert!(t.generation_rules.default_count >= t.generation_rules.min_count);
        assert!(t.generation_rules.default_count <= t.generation_rules.max_count);
        assert!(t.generation_rules.ethos_noise > 0.0);
        assert!(t.generation_rules.ethos_noise <= 0.5);
    }

    #[test]
    fn all_prefixes_have_positive_weight() {
        let t = load_civ_templates();
        for prefix in &t.prefixes {
            assert!(
                prefix.weight > 0.0,
                "Prefix '{}' has non-positive weight {}",
                prefix.name, prefix.weight,
            );
        }
    }

    #[test]
    fn all_suffixes_have_positive_weight() {
        let t = load_civ_templates();
        for suffix in &t.suffixes {
            assert!(
                suffix.weight > 0.0,
                "Suffix '{}' has non-positive weight {}",
                suffix.name, suffix.weight,
            );
        }
    }

    #[test]
    fn suffix_ethos_weights_in_range() {
        let t = load_civ_templates();
        let expected_axes = [
            "expansionist", "isolationist", "militaristic", "diplomatic",
            "theocratic", "mercantile", "technocratic", "communal",
        ];
        for suffix in &t.suffixes {
            // Every suffix should define all 8 ethos axes.
            for axis in &expected_axes {
                let value = suffix.ethos_weights.get(*axis).copied().unwrap_or(-1.0);
                assert!(
                    (0.0..=1.0).contains(&value),
                    "Suffix '{}' axis '{}' = {} — expected 0.0..=1.0",
                    suffix.name, axis, value,
                );
            }
        }
    }

    #[test]
    fn prefix_ethos_bias_values_in_range() {
        let t = load_civ_templates();
        for prefix in &t.prefixes {
            for (axis, &value) in &prefix.ethos_bias {
                assert!(
                    (-1.0..=1.0).contains(&value),
                    "Prefix '{}' bias '{}' = {} — expected -1.0..=1.0",
                    prefix.name, axis, value,
                );
            }
        }
    }

    #[test]
    fn blocked_pairs_reference_valid_names() {
        let t = load_civ_templates();
        let prefix_names: Vec<&str> = t.prefixes.iter().map(|p| p.name.as_str()).collect();
        let suffix_names: Vec<&str> = t.suffixes.iter().map(|s| s.name.as_str()).collect();

        for (prefix, blocked_suffixes) in &t.compatibility.blocked_pairs {
            assert!(
                prefix_names.contains(&prefix.as_str()),
                "Blocked pair references unknown prefix '{}'", prefix,
            );
            for suffix in blocked_suffixes {
                assert!(
                    suffix_names.contains(&suffix.as_str()),
                    "Blocked pair for '{}' references unknown suffix '{}'",
                    prefix, suffix,
                );
            }
        }
    }

    #[test]
    fn is_blocked_works() {
        let t = load_civ_templates();
        // From the JSON: Haven is blocked with Dominion and Hegemony.
        assert!(t.compatibility.is_blocked("Haven", "Dominion"));
        assert!(t.compatibility.is_blocked("Haven", "Hegemony"));
        // Terran has no blocked pairs.
        assert!(!t.compatibility.is_blocked("Terran", "Hegemony"));
        assert!(!t.compatibility.is_blocked("Terran", "Compact"));
    }

    #[test]
    fn initial_state_ranges_are_valid() {
        let t = load_civ_templates();
        let r = &t.initial_state_ranges;
        assert!(r.stability[0] <= r.stability[1]);
        assert!(r.growth_rate[0] <= r.growth_rate[1]);
        assert!(r.military_capability[0] <= r.military_capability[1]);
        assert!(r.economic_output[0] <= r.economic_output[1]);
        assert!(r.tech_level[0] <= r.tech_level[1]);
    }

    #[test]
    fn enough_prefixes_and_suffixes_for_max_civs() {
        let t = load_civ_templates();
        // We need unique suffixes per civ, so suffixes >= max_civs.
        assert!(
            t.suffixes.len() >= t.generation_rules.max_count,
            "Not enough suffixes ({}) for max_civs ({})",
            t.suffixes.len(), t.generation_rules.max_count,
        );
        // Prefixes should also be plentiful enough (though they can repeat
        // across civs, having more is better for variety).
        assert!(
            t.prefixes.len() >= t.generation_rules.max_count,
            "Not enough prefixes ({}) for max_civs ({})",
            t.prefixes.len(), t.generation_rules.max_count,
        );
    }

    // -----------------------------------------------------------------------
    // Faction template tests
    // -----------------------------------------------------------------------

    #[test]
    fn faction_templates_deserialize() {
        let t = load_faction_templates();
        assert_eq!(t.categories.len(), 6, "Expected 6 faction categories");
    }

    #[test]
    fn all_guaranteed_categories_exist() {
        let t = load_faction_templates();
        for category in &t.generation_rules.guaranteed {
            assert!(
                t.categories.contains_key(category),
                "Guaranteed category '{}' not found in categories map",
                category,
            );
        }
    }

    #[test]
    fn all_optional_categories_exist() {
        let t = load_faction_templates();
        for rule in &t.generation_rules.optional {
            assert!(
                t.categories.contains_key(&rule.category),
                "Optional category '{}' not found in categories map",
                rule.category,
            );
        }
    }

    #[test]
    fn optional_chances_in_range() {
        let t = load_faction_templates();
        for rule in &t.generation_rules.optional {
            assert!(
                (0.0..=1.0).contains(&rule.chance),
                "Optional category '{}' has chance {} — expected 0.0..=1.0",
                rule.category, rule.chance,
            );
        }
    }

    #[test]
    fn faction_scopes_are_valid() {
        let t = load_faction_templates();
        let valid_scopes = ["civ_internal", "transnational", "independent"];
        for (name, cat) in &t.categories {
            assert!(
                valid_scopes.contains(&cat.scope.as_str()),
                "Category '{}' has invalid scope '{}'",
                name, cat.scope,
            );
        }
    }

    #[test]
    fn faction_base_traits_in_range() {
        let t = load_faction_templates();
        for (name, cat) in &t.categories {
            let bt = &cat.base_traits;
            assert!(
                (-1.0..=1.0).contains(&bt.alignment),
                "Category '{}' alignment {} out of range", name, bt.alignment,
            );
            assert!(
                (0.0..=1.0).contains(&bt.aggression),
                "Category '{}' aggression {} out of range", name, bt.aggression,
            );
            assert!(
                (0.0..=1.0).contains(&bt.openness),
                "Category '{}' openness {} out of range", name, bt.openness,
            );
            assert!(
                (0.0..=1.0).contains(&bt.secrecy),
                "Category '{}' secrecy {} out of range", name, bt.secrecy,
            );
        }
    }

    #[test]
    fn faction_visibility_ranges_are_valid() {
        let t = load_faction_templates();
        for (name, cat) in &t.categories {
            let [lo, hi] = cat.visibility_range;
            assert!(lo <= hi, "Category '{}' visibility range [{}, {}] is inverted", name, lo, hi);
            assert!((0.0..=1.0).contains(&lo), "Category '{}' visibility lo {} out of range", name, lo);
            assert!((0.0..=1.0).contains(&hi), "Category '{}' visibility hi {} out of range", name, hi);
        }
    }

    #[test]
    fn every_category_has_at_least_one_name_pattern() {
        let t = load_faction_templates();
        for (name, cat) in &t.categories {
            assert!(
                !cat.name_patterns.is_empty(),
                "Category '{}' has no name patterns", name,
            );
        }
    }

    #[test]
    fn name_pattern_slots_have_matching_components() {
        let t = load_faction_templates();
        for (cat_name, cat) in &t.categories {
            for pattern in &cat.name_patterns {
                // Extract {slot_name} references from the pattern.
                let slots: Vec<&str> = pattern
                    .match_indices('{')
                    .filter_map(|(start, _)| {
                        let rest = &pattern[start + 1..];
                        rest.find('}').map(|end| &rest[..end])
                    })
                    .collect();

                for slot in slots {
                    // {civ_prefix} is special — filled from the parent civ, not components.
                    if slot == "civ_prefix" {
                        continue;
                    }
                    assert!(
                        cat.components.contains_key(slot),
                        "Category '{}' pattern '{}' references slot '{{{}}}' \
                         but no matching component pool exists",
                        cat_name, pattern, slot,
                    );
                }
            }
        }
    }

    #[test]
    fn all_component_pools_are_non_empty() {
        let t = load_faction_templates();
        for (cat_name, cat) in &t.categories {
            for (slot_name, components) in &cat.components {
                assert!(
                    !components.is_empty(),
                    "Category '{}' slot '{}' has an empty component pool",
                    cat_name, slot_name,
                );
            }
        }
    }

    #[test]
    fn all_components_have_positive_weight() {
        let t = load_faction_templates();
        for (cat_name, cat) in &t.categories {
            for (slot_name, components) in &cat.components {
                for comp in components {
                    assert!(
                        comp.weight > 0.0,
                        "Category '{}' slot '{}' component '{}' has non-positive weight {}",
                        cat_name, slot_name, comp.name, comp.weight,
                    );
                }
            }
        }
    }

    #[test]
    fn max_factions_accommodates_guaranteed_plus_optional() {
        let t = load_faction_templates();
        // max_factions should be >= guaranteed count (bare minimum).
        assert!(
            t.generation_rules.max_factions >= t.generation_rules.guaranteed.len(),
            "max_factions ({}) is less than guaranteed count ({})",
            t.generation_rules.max_factions, t.generation_rules.guaranteed.len(),
        );
    }

    #[test]
    fn expected_categories_present() {
        let t = load_faction_templates();
        let expected = [
            "military", "economic", "guild", "religious",
            "criminal_frontier", "criminal_covert",
        ];
        for name in &expected {
            assert!(
                t.categories.contains_key(*name),
                "Expected faction category '{}' not found", name,
            );
        }
    }
}