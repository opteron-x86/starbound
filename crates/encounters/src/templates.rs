// file: crates/encounters/src/templates.rs
//! Template resolution — fills placeholders in event text with live game state.
//!
//! Event text can contain `{placeholder}` tokens that get replaced with
//! values from the current game context before the player sees them.
//!
//! Supported placeholders:
//! - `{system.name}` — current star system name
//! - `{system.description}` — current system description
//! - `{faction.name}` — dominant faction at this system (if any)
//! - `{faction.category}` — faction category (military, economic, etc.)
//! - `{civ.name}` — civilization that controls this system (if any)
//! - `{crew.random.name}` — a random crew member's name
//! - `{ship.name}` — the player's ship name
//! - `{personal.months}` — ship-time elapsed in months (formatted)
//! - `{galactic.years}` — galactic time elapsed in years (formatted)
//!
//! Unrecognized placeholders are left as-is (useful for debugging
//! and for future expansion).

use std::collections::HashMap;

/// Context for template resolution — built from game state at encounter time.
#[derive(Debug, Clone)]
pub struct TemplateContext {
    pub system_name: String,
    pub system_description: String,
    pub faction_name: Option<String>,
    pub faction_category: Option<String>,
    pub civ_name: Option<String>,
    pub crew_random_name: Option<String>,
    pub ship_name: String,
    pub personal_months: f64,
    pub galactic_years: f64,
    /// Custom key-value pairs for event-chain state passing.
    pub custom: HashMap<String, String>,
}

impl TemplateContext {
    /// Look up a placeholder key and return its value.
    fn resolve_key(&self, key: &str) -> Option<String> {
        match key {
            "system.name" => Some(self.system_name.clone()),
            "system.description" => Some(self.system_description.clone()),
            "faction.name" => self.faction_name.clone().or_else(|| Some("an unknown faction".into())),
            "faction.category" => self.faction_category.clone(),
            "civ.name" => self.civ_name.clone().or_else(|| Some("an unknown civilization".into())),
            "crew.random.name" => self.crew_random_name.clone().or_else(|| Some("a crew member".into())),
            "ship.name" => Some(self.ship_name.clone()),
            "personal.months" => Some(format!("{:.1}", self.personal_months)),
            "galactic.years" => Some(format!("{:.1}", self.galactic_years)),
            _ => self.custom.get(key).cloned(),
        }
    }
}

/// Resolve all `{placeholder}` tokens in the given text.
/// Unrecognized placeholders are left unchanged.
pub fn resolve_template(text: &str, ctx: &TemplateContext) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            // Collect the key until we hit '}' or run out of input.
            let mut key = String::new();
            let mut found_close = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    found_close = true;
                    break;
                }
                key.push(inner);
            }

            if found_close {
                if let Some(value) = ctx.resolve_key(&key) {
                    result.push_str(&value);
                } else {
                    // Unrecognized — leave as-is for debugging.
                    result.push('{');
                    result.push_str(&key);
                    result.push('}');
                }
            } else {
                // Unclosed brace — emit literally.
                result.push('{');
                result.push_str(&key);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context() -> TemplateContext {
        TemplateContext {
            system_name: "Acheron".into(),
            system_description: "A frozen world at the edge of mapped space.".into(),
            faction_name: Some("Corridor Guild".into()),
            faction_category: Some("economic".into()),
            civ_name: Some("Terran Hegemony".into()),
            crew_random_name: Some("Kael Vasquez".into()),
            ship_name: "Persistence".into(),
            personal_months: 14.3,
            galactic_years: 87.2,
            custom: HashMap::new(),
        }
    }

    #[test]
    fn basic_substitution() {
        let ctx = test_context();
        let text = "Fossils found on the surface of {system.name}.";
        let result = resolve_template(text, &ctx);
        assert_eq!(result, "Fossils found on the surface of Acheron.");
    }

    #[test]
    fn multiple_placeholders() {
        let ctx = test_context();
        let text = "The {ship.name} docks at {system.name}. {crew.random.name} steps out.";
        let result = resolve_template(text, &ctx);
        assert_eq!(result, "The Persistence docks at Acheron. Kael Vasquez steps out.");
    }

    #[test]
    fn missing_optional_with_fallback() {
        let mut ctx = test_context();
        ctx.faction_name = None;
        let text = "Controlled by {faction.name}.";
        let result = resolve_template(text, &ctx);
        assert_eq!(result, "Controlled by an unknown faction.");
    }

    #[test]
    fn unrecognized_placeholder_left_intact() {
        let ctx = test_context();
        let text = "The {unknown.placeholder} remains.";
        let result = resolve_template(text, &ctx);
        assert_eq!(result, "The {unknown.placeholder} remains.");
    }

    #[test]
    fn no_placeholders_pass_through() {
        let ctx = test_context();
        let text = "Just plain text with no templates.";
        let result = resolve_template(text, &ctx);
        assert_eq!(result, text);
    }

    #[test]
    fn unclosed_brace_emitted_literally() {
        let ctx = test_context();
        let text = "Broken {system.name template.";
        let result = resolve_template(text, &ctx);
        assert_eq!(result, "Broken {system.name template.");
    }

    #[test]
    fn custom_entries() {
        let mut ctx = test_context();
        ctx.custom.insert("artifact.name".into(), "The Quiet Lens".into());
        let text = "You found {artifact.name} on {system.name}.";
        let result = resolve_template(text, &ctx);
        assert_eq!(result, "You found The Quiet Lens on Acheron.");
    }

    #[test]
    fn numeric_formatting() {
        let ctx = test_context();
        let text = "Elapsed: {personal.months} months / {galactic.years} galactic years.";
        let result = resolve_template(text, &ctx);
        assert_eq!(result, "Elapsed: 14.3 months / 87.2 galactic years.");
    }
}