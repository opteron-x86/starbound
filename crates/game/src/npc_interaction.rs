// file: crates/game/src/npc_interaction.rs
//! NPC interaction system — personality-driven, disposition-gated conversations.
//!
//! This module provides the game logic for NPC interactions. It sits between
//! the data model (`core::npc`) and the presentation layer (CLI). The CLI
//! calls these functions to determine what the NPC says and what options
//! the player has — then displays the results.
//!
//! ## Design principles
//!
//! - **Disposition gates access**: What the player can do depends on how the
//!   NPC feels about them. Hostile NPCs refuse to talk. Neutral NPCs are
//!   transactional. Trusted NPCs share secrets.
//! - **Personality shapes tone**: Two NPCs at the same disposition tier will
//!   behave differently based on their warmth/boldness/idealism axes.
//! - **Templates over generation**: All dialogue comes from JSON templates.
//!   Personality determines *which* templates fire and *what* information
//!   is shared. The actual words are authored.
//! - **LLM-ready**: The personality data + disposition + knowledge items
//!   form a prompt-ready context for future LLM dialogue generation.

use std::collections::HashMap;
use rand::rngs::StdRng;
use rand::Rng;
use serde::Deserialize;
use uuid::Uuid;

use starbound_core::npc::{Npc, NpcConnection, NpcRelationType, DispositionTier};

// ---------------------------------------------------------------------------
// Dialogue templates — loaded from JSON
// ---------------------------------------------------------------------------

const DIALOGUE_JSON: &str = include_str!("../../../data/templates/npc_dialogue.json");

/// Top-level structure for `npc_dialogue.json`.
#[derive(Debug, Deserialize)]
pub struct DialogueTemplates {
    pub greetings: HashMap<String, Vec<String>>,
    pub returning_player: HashMap<String, Vec<String>>,
    pub ask_area_framing: HashMap<String, Vec<String>>,
    pub knowledge_delivery: HashMap<String, Vec<String>>,
    pub ask_about_connection: HashMap<String, Vec<String>>,
    pub contract_refusal: HashMap<String, Vec<String>>,
    pub farewell: HashMap<String, Vec<String>>,
}

/// Load dialogue templates. Panics on bad JSON — this is a compile-time embed.
pub fn load_dialogue_templates() -> DialogueTemplates {
    serde_json::from_str(DIALOGUE_JSON)
        .expect("npc_dialogue.json should be valid — this is a compile-time embed")
}

// ---------------------------------------------------------------------------
// NPC presentation — what the player sees
// ---------------------------------------------------------------------------

/// Everything the CLI needs to display an NPC conversation screen.
pub struct NpcPresentation {
    /// The greeting line when the player approaches.
    pub greeting: String,
    /// Optional returning-player memory line.
    pub memory_line: Option<String>,
    /// Personality-derived description (from personality expressions).
    pub personality_sketch: String,
    /// Available conversation options, in display order.
    pub options: Vec<NpcMenuOption>,
}

/// A single option in the NPC conversation menu.
#[derive(Debug, Clone)]
pub struct NpcMenuOption {
    pub label: String,
    pub action: NpcAction,
}

/// What the player can do with an NPC.
#[derive(Debug, Clone)]
pub enum NpcAction {
    AskAboutWork,
    TurnInContract,
    AskAboutArea,
    AskAboutConnection(Uuid),
    Leave,
}

/// The result of asking about the area — knowledge items + framing text.
pub struct AreaKnowledge {
    /// Opening framing line (personality-shaped).
    pub framing: String,
    /// Knowledge items the NPC shares, each with its delivery template filled in.
    pub items: Vec<KnowledgeItem>,
    /// Optional connection mention.
    pub connection_mention: Option<String>,
}

/// One piece of knowledge the NPC shares.
pub struct KnowledgeItem {
    /// The raw knowledge string (for tracking what's been shared).
    pub raw: String,
    /// The delivered version — template filled with personality + context.
    pub delivered: String,
    /// What tier of knowledge this is.
    pub tier: KnowledgeTier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnowledgeTier {
    General,
    FactionIntel,
    ThreadRumor,
}

/// Result of asking about a connected NPC.
pub struct ConnectionInfo {
    /// The connected NPC's name.
    pub name: String,
    /// Their title.
    pub title: String,
    /// Where they are — system name and location name.
    pub location: String,
    /// What the asking NPC says about them.
    pub description: String,
}

// ---------------------------------------------------------------------------
// Core interaction logic
// ---------------------------------------------------------------------------

/// Build the full NPC presentation for the conversation screen.
///
/// This is the main entry point for the CLI. It gathers everything needed
/// to display the NPC interaction: greeting, memory, personality sketch,
/// and available options.
pub fn build_npc_presentation(
    npc: &Npc,
    has_turnable_contract: bool,
    ship_name: &str,
    system_name: &str,
    faction_name: &str,
    personality_expressions: &HashMap<String, Vec<String>>,
    rng: &mut StdRng,
) -> NpcPresentation {
    let dt = load_dialogue_templates();
    let tier = npc.disposition_tier();
    let tier_key = tier.label().to_string();

    // --- Greeting ---
    let greeting = pick_template(
        dt.greetings.get(&tier_key).map(|v| v.as_slice()).unwrap_or(&[]),
        rng,
    ).map(|t| fill_npc_placeholders(&t, npc, ship_name, system_name, faction_name))
     .unwrap_or_else(|| format!("{} acknowledges your presence.", npc.name));

    // --- Returning player memory ---
    let memory_line = npc.last_interaction().map(|record| {
        let tone_key = if record.disposition_delta > 0.0 {
            "positive"
        } else if record.disposition_delta < 0.0 {
            "negative"
        } else {
            "neutral_memory"
        };
        pick_template(
            dt.returning_player.get(tone_key).map(|v| v.as_slice()).unwrap_or(&[]),
            rng,
        ).map(|t| {
            fill_npc_placeholders(&t, npc, ship_name, system_name, faction_name)
                .replace("{summary}", &record.summary)
        })
        .unwrap_or_else(|| format!("\"I remember you. {}.\"", record.summary))
    });

    // --- Personality sketch ---
    let personality_sketch = build_personality_sketch(npc, personality_expressions, rng);

    // --- Menu options ---
    let mut options = Vec::new();

    if tier == DispositionTier::Hostile {
        // Hostile NPCs only allow leaving.
        options.push(NpcMenuOption {
            label: "Leave".into(),
            action: NpcAction::Leave,
        });
    } else {
        if npc.will_offer_contracts() {
            options.push(NpcMenuOption {
                label: "Ask about work".into(),
                action: NpcAction::AskAboutWork,
            });
        }
        if has_turnable_contract {
            options.push(NpcMenuOption {
                label: "Turn in contract".into(),
                action: NpcAction::TurnInContract,
            });
        }
        if tier >= DispositionTier::Cold {
            options.push(NpcMenuOption {
                label: "Ask about the area".into(),
                action: NpcAction::AskAboutArea,
            });
        }
        // Connection options — only at Warm+.
        if npc.will_share_connections() {
            for conn in &npc.connections {
                options.push(NpcMenuOption {
                    label: format!("Ask about a contact"),
                    action: NpcAction::AskAboutConnection(conn.npc_id),
                });
                break; // Single "ask about contacts" entry; individual selection happens in sub-menu.
            }
        }
        options.push(NpcMenuOption {
            label: "Leave".into(),
            action: NpcAction::Leave,
        });
    }

    NpcPresentation {
        greeting,
        memory_line,
        personality_sketch,
        options,
    }
}

/// Build the "ask about the area" response.
///
/// Uses the NPC's knowledge pool, filtered by disposition and personality.
/// Returns a structured AreaKnowledge that the CLI can display.
pub fn ask_about_area(
    npc: &Npc,
    system_name: &str,
    ship_name: &str,
    faction_name: &str,
    all_npcs: &[Npc],
    already_shared: &[String],
    rng: &mut StdRng,
) -> AreaKnowledge {
    let dt = load_dialogue_templates();
    let tier = npc.disposition_tier();

    // --- Framing line (personality-shaped opening) ---
    let framing_key = if npc.personality.warmth > 0.6 {
        "warmth_high"
    } else if npc.personality.warmth < 0.4 {
        "warmth_low"
    } else if npc.personality.boldness > 0.6 {
        "boldness_high"
    } else {
        "boldness_low"
    };
    let framing = pick_template(
        dt.ask_area_framing.get(framing_key).map(|v| v.as_slice()).unwrap_or(&[]),
        rng,
    ).map(|t| fill_npc_placeholders(&t, npc, ship_name, system_name, faction_name))
     .unwrap_or_default();

    // --- Knowledge items ---
    let shareable = npc.shareable_knowledge(already_shared);
    let mut items = Vec::new();

    for (i, knowledge_str) in shareable.iter().enumerate() {
        let (tier_kind, template_key) = match i {
            0 => (KnowledgeTier::General, "general"),
            1 if tier >= DispositionTier::Warm => (KnowledgeTier::FactionIntel, "faction_intel"),
            _ if tier >= DispositionTier::Friendly => (KnowledgeTier::ThreadRumor, "thread_rumor"),
            _ => (KnowledgeTier::General, "general"),
        };

        let delivered = pick_template(
            dt.knowledge_delivery.get(template_key).map(|v| v.as_slice()).unwrap_or(&[]),
            rng,
        ).map(|t| {
            fill_npc_placeholders(&t, npc, ship_name, system_name, faction_name)
                .replace("{knowledge_item}", knowledge_str)
        })
        .unwrap_or_else(|| format!("\"{}\"", knowledge_str));

        items.push(KnowledgeItem {
            raw: knowledge_str.to_string(),
            delivered,
            tier: tier_kind,
        });
    }

    // --- Connection mention (warmth > 0.6 and has connections) ---
    let connection_mention = if npc.personality.warmth > 0.6 && !npc.connections.is_empty() {
        let conn = &npc.connections[rng.gen_range(0..npc.connections.len())];
        let connected_npc = all_npcs.iter().find(|n| n.id == conn.npc_id);

        connected_npc.map(|cn| {
            // Use the NPC's display name — title-only if the player hasn't met them.
            let cn_display = cn.display_name();
            pick_template(
                dt.knowledge_delivery.get("connection_mention").map(|v| v.as_slice()).unwrap_or(&[]),
                rng,
            ).map(|t| {
                t.replace("{connection_name}", cn_display)
                 .replace("{connection_title}", &cn.title)
                 .replace("{connection_location}", system_name) // Simplified for now.
                 .replace("{connection_object}", &cn.pronouns.object)
                 .replace("{name}", &npc.name)
                 .replace("{pronoun.subject}", &npc.pronouns.subject)
                 .replace("{pronoun.subject_cap}", &npc.pronouns.subject_cap)
                 .replace("{pronoun.possessive}", &npc.pronouns.possessive)
            })
            .unwrap_or_else(|| format!("\"Talk to the {} — {} might know more.\"", cn.title, cn.pronouns.subject))
        })
    } else {
        None
    };

    AreaKnowledge {
        framing,
        items,
        connection_mention,
    }
}

/// Build the "ask about [connection]" response.
pub fn ask_about_connection(
    npc: &Npc,
    connection: &NpcConnection,
    connected_npc: &Npc,
    system_name: &str,
    connected_system_name: &str,
    _ship_name: &str,
    _faction_name: &str,
    rng: &mut StdRng,
) -> ConnectionInfo {
    let dt = load_dialogue_templates();
    let rel_key = match connection.relationship {
        NpcRelationType::Colleague => "colleague",
        NpcRelationType::Acquaintance => "acquaintance",
        NpcRelationType::Rival => "rival",
        NpcRelationType::Dependent => "dependent",
        NpcRelationType::OldFriend => "old_friend",
        NpcRelationType::KnowsOf => "knows_of",
    };

    let description = pick_template(
        dt.ask_about_connection.get(rel_key).map(|v| v.as_slice()).unwrap_or(&[]),
        rng,
    ).map(|t| {
        t.replace("{connection_name}", &connected_npc.name)
         .replace("{connection_object}", &connected_npc.pronouns.object)
         .replace("{context}", &connection.context)
         .replace("{name}", &npc.name)
         .replace("{pronoun.subject}", &npc.pronouns.subject)
         .replace("{pronoun.subject_cap}", &npc.pronouns.subject_cap)
         .replace("{pronoun.possessive}", &npc.pronouns.possessive)
    })
    .unwrap_or_else(|| format!(
        "\"{} — {}. That's all I'll say.\"",
        connected_npc.name, connection.context
    ));

    let location = if connected_npc.home_system_id == npc.home_system_id {
        format!("here at {}", system_name)
    } else {
        format!("{} system", connected_system_name)
    };

    ConnectionInfo {
        name: connected_npc.name.clone(),
        title: connected_npc.title.clone(),
        location,
        description,
    }
}

/// Get the contract refusal text for NPCs who won't offer work.
pub fn contract_refusal_text(
    npc: &Npc,
    ship_name: &str,
    system_name: &str,
    faction_name: &str,
    rng: &mut StdRng,
) -> String {
    let dt = load_dialogue_templates();
    let tier = npc.disposition_tier();
    let tier_key = if tier == DispositionTier::Hostile {
        "hostile"
    } else {
        "cold"
    };

    pick_template(
        dt.contract_refusal.get(tier_key).map(|v| v.as_slice()).unwrap_or(&[]),
        rng,
    ).map(|t| fill_npc_placeholders(&t, npc, ship_name, system_name, faction_name))
     .unwrap_or_else(|| "\"No work for you.\"".into())
}

/// Get a farewell line for when the player leaves.
pub fn farewell_text(
    npc: &Npc,
    ship_name: &str,
    system_name: &str,
    faction_name: &str,
    rng: &mut StdRng,
) -> String {
    let dt = load_dialogue_templates();
    let tier_key = npc.disposition_tier().label().to_string();

    pick_template(
        dt.farewell.get(&tier_key).map(|v| v.as_slice()).unwrap_or(&[]),
        rng,
    ).map(|t| fill_npc_placeholders(&t, npc, ship_name, system_name, faction_name))
     .unwrap_or_else(|| format!("{} nods.", npc.name))
}

// ---------------------------------------------------------------------------
// Personality sketch builder
// ---------------------------------------------------------------------------

/// Build a 2-sentence personality sketch from expressions + personality axes.
///
/// Uses `personality_expressions` from people.json. Picks one expression
/// from the dominant axis and one from the secondary axis.
fn build_personality_sketch(
    npc: &Npc,
    expressions: &HashMap<String, Vec<String>>,
    rng: &mut StdRng,
) -> String {
    let p = &npc.personality;

    let (dom_axis, dom_val, _) = p.dominant_axis();
    let (sec_axis, sec_val, _) = p.secondary_axis();

    let dom_key = format!("{}_{}", dom_axis, if dom_val > 0.5 { "high" } else { "low" });
    let sec_key = format!("{}_{}", sec_axis, if sec_val > 0.5 { "high" } else { "low" });

    let dom_expr = expressions.get(&dom_key)
        .and_then(|pool| pick_template(pool, rng))
        .unwrap_or_default();

    let sec_expr = expressions.get(&sec_key)
        .and_then(|pool| pick_template(pool, rng))
        .unwrap_or_default();

    if dom_expr.is_empty() && sec_expr.is_empty() {
        return format!("{} is {}.", npc.name, p.dominant_description());
    }

    if !sec_expr.is_empty() {
        format!("{} {}. {} also {}.", npc.name, dom_expr, npc.pronouns.subject_cap, sec_expr)
    } else {
        format!("{} {}.", npc.name, dom_expr)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Pick a random template from a slice. Returns None if empty.
fn pick_template(templates: &[String], rng: &mut StdRng) -> Option<String> {
    if templates.is_empty() {
        None
    } else {
        Some(templates[rng.gen_range(0..templates.len())].clone())
    }
}

/// Fill standard NPC placeholders in a template string.
fn fill_npc_placeholders(
    template: &str,
    npc: &Npc,
    ship_name: &str,
    system_name: &str,
    faction_name: &str,
) -> String {
    template
        .replace("{name}", &npc.name)
        .replace("{title}", &npc.title)
        .replace("{pronoun.subject}", &npc.pronouns.subject)
        .replace("{pronoun.object}", &npc.pronouns.object)
        .replace("{pronoun.possessive}", &npc.pronouns.possessive)
        .replace("{pronoun.subject_cap}", &npc.pronouns.subject_cap)
        .replace("{ship_name}", ship_name)
        .replace("{system}", system_name)
        .replace("{faction}", faction_name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use starbound_core::npc::*;
    use rand::SeedableRng;

    fn test_npc(name: &str, disposition: f32) -> Npc {
        let mut npc = Npc::new(
            name,
            "Guild Factor",
            Species::Human { sex: BiologicalSex::Female },
            None,
            Uuid::new_v4(),
            "A seasoned trader.",
        );
        npc.disposition = disposition;
        npc.personality = NpcPersonality {
            warmth: 0.7,
            boldness: 0.4,
            idealism: 0.6,
        };
        npc.knowledge = vec![
            "Trade routes have shifted recently".into(),
            "Military patrols increased last month".into(),
            "A derelict was found near the outer belt".into(),
        ];
        npc
    }

    #[test]
    fn dialogue_templates_load() {
        let dt = load_dialogue_templates();
        assert!(!dt.greetings.is_empty(), "Should have greeting templates");
        assert!(dt.greetings.contains_key("neutral"), "Should have neutral greetings");
        assert!(dt.greetings.contains_key("hostile"), "Should have hostile greetings");
        assert!(dt.greetings.contains_key("trusted"), "Should have trusted greetings");
    }

    #[test]
    fn hostile_npc_only_allows_leave() {
        let mut rng = StdRng::seed_from_u64(42);
        let npc = test_npc("Maren", -0.8);
        let expressions = HashMap::new();

        let pres = build_npc_presentation(
            &npc, false, "Wanderer", "Vela", "Guild",
            &expressions, &mut rng,
        );
        assert_eq!(pres.options.len(), 1);
        assert!(matches!(pres.options[0].action, NpcAction::Leave));
    }

    #[test]
    fn neutral_npc_offers_work_and_area() {
        let mut rng = StdRng::seed_from_u64(42);
        let npc = test_npc("Joss", 0.0);
        let expressions = HashMap::new();

        let pres = build_npc_presentation(
            &npc, false, "Wanderer", "Vela", "Guild",
            &expressions, &mut rng,
        );
        let labels: Vec<&str> = pres.options.iter().map(|o| o.label.as_str()).collect();
        assert!(labels.contains(&"Ask about work"), "Neutral NPC should offer work");
        assert!(labels.contains(&"Ask about the area"), "Neutral NPC should offer area info");
        assert!(labels.contains(&"Leave"));
    }

    #[test]
    fn warm_npc_shows_connection_option() {
        let mut rng = StdRng::seed_from_u64(42);
        let mut npc = test_npc("Suri", 0.3);
        let other_id = Uuid::new_v4();
        npc.connections.push(NpcConnection {
            npc_id: other_id,
            relationship: NpcRelationType::Colleague,
            context: "Works in the same sector".into(),
        });
        let expressions = HashMap::new();

        let pres = build_npc_presentation(
            &npc, false, "Wanderer", "Vela", "Guild",
            &expressions, &mut rng,
        );
        let has_contact = pres.options.iter().any(|o| o.label.contains("contact"));
        assert!(has_contact, "Warm NPC with connections should show contact option");
    }

    #[test]
    fn knowledge_sharing_respects_disposition() {
        let neutral = test_npc("Neutral Npc", 0.0);
        let warm = test_npc("Warm Npc", 0.3);
        let friendly = test_npc("Friendly Npc", 0.6);

        assert!(neutral.knowledge_share_count() < warm.knowledge_share_count());
        assert!(warm.knowledge_share_count() <= friendly.knowledge_share_count());
    }

    #[test]
    fn ask_area_returns_knowledge_items() {
        let mut rng = StdRng::seed_from_u64(42);
        let npc = test_npc("Maren", 0.3);

        let area = ask_about_area(
            &npc, "Vela", "Wanderer", "Guild",
            &[], &[], &mut rng,
        );
        assert!(!area.items.is_empty(), "Warm NPC should share knowledge");
        assert!(!area.framing.is_empty(), "Should have a framing line");
    }

    #[test]
    fn already_shared_knowledge_is_filtered() {
        let npc = test_npc("Joss", 0.3);
        let already = vec!["Trade routes have shifted recently".to_string()];
        let shareable = npc.shareable_knowledge(&already);
        assert!(
            !shareable.contains(&"Trade routes have shifted recently"),
            "Already shared knowledge should be filtered"
        );
    }

    #[test]
    fn returning_player_gets_memory_line() {
        let mut rng = StdRng::seed_from_u64(42);
        let mut npc = test_npc("Thea", 0.2);
        npc.record_interaction("delivered medical supplies", 100.0, 0.1);
        let expressions = HashMap::new();

        let pres = build_npc_presentation(
            &npc, false, "Wanderer", "Vela", "Guild",
            &expressions, &mut rng,
        );
        assert!(pres.memory_line.is_some(), "Returning player should see memory line");
    }

    #[test]
    fn connection_info_includes_relationship_context() {
        let mut rng = StdRng::seed_from_u64(42);
        let npc = test_npc("Maren", 0.4);
        let mut connected = test_npc("Kael", 0.0);
        connected.personality.boldness = 0.8;

        let conn = NpcConnection {
            npc_id: connected.id,
            relationship: NpcRelationType::Rival,
            context: "Competes for the same trade routes".into(),
        };

        let info = ask_about_connection(
            &npc, &conn, &connected,
            "Vela", "Vela",
            "Wanderer", "Guild",
            &mut rng,
        );
        assert!(!info.description.is_empty());
        assert_eq!(info.name, "Kael");
    }

    #[test]
    fn disposition_tiers_are_ordered() {
        assert!(DispositionTier::Hostile < DispositionTier::Cold);
        assert!(DispositionTier::Cold < DispositionTier::Neutral);
        assert!(DispositionTier::Neutral < DispositionTier::Warm);
        assert!(DispositionTier::Warm < DispositionTier::Friendly);
        assert!(DispositionTier::Friendly < DispositionTier::Trusted);
    }
}