// file: crates/llm/src/npc_dialogue.rs
//! LLM flavor for NPC interactions — optional atmospheric dialogue.
//!
//! Takes structured NPC data (personality, disposition, knowledge) and
//! generates in-world dialogue. Falls back silently to template text
//! on failure.
//!
//! Two entry points:
//! - `flavor_npc_greeting` — the opening line when the player approaches
//! - `flavor_npc_knowledge` — delivering knowledge items as conversation
//!
//! Each call is cheap: short prompt, short response, single attempt.

use crate::client;
use crate::config::LlmConfig;

// ---------------------------------------------------------------------------
// NPC context for prompts
// ---------------------------------------------------------------------------

/// Structured NPC context for LLM prompts.
/// Extracted from the game state by the CLI before calling flavor functions.
pub struct NpcContext {
    pub name: String,
    pub title: String,
    pub pronouns_subject: String,
    pub pronouns_object: String,
    pub pronouns_possessive: String,
    /// Descriptive personality — "warm and cautious" rather than numbers.
    pub personality_desc: String,
    /// Disposition tier label — "neutral", "warm", "trusted", etc.
    pub disposition_label: String,
    /// Short bio — 1-2 sentences of background.
    pub bio: String,
    /// The station/location name.
    pub location_name: String,
    /// The star system name.
    pub system_name: String,
    /// Faction name, if any.
    pub faction_name: String,
    /// The player's ship name.
    pub ship_name: String,
}

impl NpcContext {
    /// Build a compact character brief for the LLM.
    fn character_brief(&self) -> String {
        format!(
            "CHARACTER: {} ({}), {}\n\
             PERSONALITY: {}\n\
             DISPOSITION TOWARD PLAYER: {}\n\
             PRONOUNS: {}/{}/{}\n\
             BACKGROUND: {}\n\
             LOCATION: {}, {} system\n\
             FACTION: {}\n\
             PLAYER'S SHIP: {}",
            self.name, self.title, self.faction_name,
            self.personality_desc,
            self.disposition_label,
            self.pronouns_subject, self.pronouns_object, self.pronouns_possessive,
            self.bio,
            self.location_name, self.system_name,
            self.faction_name,
            self.ship_name,
        )
    }
}

// ---------------------------------------------------------------------------
// Personality description builder
// ---------------------------------------------------------------------------

/// Convert numeric personality axes into a natural language description.
/// This is the bridge between the game model and the LLM prompt.
pub fn describe_personality(warmth: f32, boldness: f32, idealism: f32) -> String {
    let warmth_desc = if warmth > 0.7 {
        "warm and open"
    } else if warmth > 0.4 {
        "measured"
    } else {
        "guarded and transactional"
    };

    let boldness_desc = if boldness > 0.7 {
        "direct and decisive"
    } else if boldness > 0.4 {
        "careful"
    } else {
        "cautious and hedging"
    };

    let idealism_desc = if idealism > 0.7 {
        "principled"
    } else if idealism > 0.4 {
        "pragmatic but fair"
    } else {
        "survival-focused"
    };

    format!("{}, {}, {}", warmth_desc, boldness_desc, idealism_desc)
}

// ---------------------------------------------------------------------------
// System prompts
// ---------------------------------------------------------------------------

const GREETING_SYSTEM_PROMPT: &str = "\
You are writing dialogue for a named NPC in a space exploration game. \
The player has just approached this person at a station.

Write the NPC's greeting — what they say or do when the player arrives.

VOICE: People in this world speak like actual people. Trailing off, \
mundane observations, important things buried in casual conversation. \
Quiet and restrained. No melodrama.

RULES:
- Write exactly 2-3 sentences. One of dialogue, one of action/description.
- The greeting MUST reflect the NPC's disposition toward the player.
- Hostile = refuses to engage. Cold = minimum words. Neutral = professional. \
  Warm = welcoming. Friendly = genuine pleasure. Trusted = pulls you aside.
- Use the NPC's name naturally — not every sentence needs it.
- Use correct pronouns throughout.
- DO NOT use exclamation marks. This world is understated.
- DO NOT invent facts about the world, the player, or their history.
- NEVER use: chipped mugs, steepled fingers, 'measured gaze/look', \
  'the hum of X filled the silence', 'something flickered in their eyes', \
  'the weight of', drumming fingers, or tracing the rim of anything. \
  Use concrete sensory details specific to THIS location instead.
- Respond with ONLY the greeting text. No JSON, no labels, no preamble.";

const KNOWLEDGE_SYSTEM_PROMPT: &str = "\
You are writing dialogue for a named NPC sharing information with the \
player in a space exploration game.

The NPC is sharing specific knowledge items. Your job is to weave them \
into natural conversation — not a bullet-point briefing.

VOICE: People in this world speak like actual people. Trailing off, \
mundane observations, important things buried in casual conversation. \
Quiet and restrained.

RULES:
- Deliver ALL knowledge items provided. Do not skip any.
- Weave them into 3-6 sentences of natural conversation. Not a list.
- The NPC's personality shapes HOW they share: warm NPCs volunteer info \
  freely, cold NPCs are terse, bold NPCs are blunt, cautious NPCs hedge.
- Use correct pronouns throughout.
- DO NOT add knowledge items that aren't provided. Stick to the facts given.
- DO NOT use exclamation marks.
- NEVER use: chipped mugs, steepled fingers, 'measured gaze/look', \
  'the hum of X filled the silence', 'something flickered in their eyes', \
  'the weight of', drumming fingers, or tracing the rim of anything. \
  Use concrete sensory details specific to THIS character and location.
- Respond with ONLY the dialogue/narration. No JSON, no labels, no preamble.";

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Generate a flavored NPC greeting.
///
/// Returns the flavored text on success, or None if the LLM is
/// unavailable or fails. The caller falls back to template text.
pub fn flavor_npc_greeting(
    config: &LlmConfig,
    npc: &NpcContext,
    memory_summary: Option<&str>,
) -> Option<String> {
    if !config.is_available() {
        return None;
    }

    let mut user_msg = npc.character_brief();

    if let Some(memory) = memory_summary {
        user_msg.push_str(&format!(
            "\n\nPLAYER HISTORY: Last time the player was here, they {}. \
             The NPC remembers this.",
            memory,
        ));
    }

    user_msg.push_str("\n\nWrite the NPC's greeting.");

    match client::chat_completion(config, GREETING_SYSTEM_PROMPT, &user_msg) {
        Ok(result) => {
            let text = result.content.trim().to_string();
            if text.len() > 600 || text.starts_with('{') {
                eprintln!("  [LLM] NPC greeting too long or malformed, using template");
                return None;
            }
            Some(text)
        }
        Err(e) => {
            eprintln!("  [LLM] NPC greeting failed: {}", e);
            None
        }
    }
}

/// Generate flavored knowledge delivery.
///
/// Takes the NPC context and a list of raw knowledge items. Returns
/// the knowledge woven into natural conversation, or None on failure.
pub fn flavor_npc_knowledge(
    config: &LlmConfig,
    npc: &NpcContext,
    knowledge_items: &[String],
) -> Option<String> {
    if !config.is_available() || knowledge_items.is_empty() {
        return None;
    }

    let mut user_msg = npc.character_brief();

    user_msg.push_str("\n\nKNOWLEDGE TO SHARE:\n");
    for (i, item) in knowledge_items.iter().enumerate() {
        user_msg.push_str(&format!("{}. {}\n", i + 1, item));
    }
    user_msg.push_str("\nDeliver these as natural conversation.");

    match client::chat_completion(config, KNOWLEDGE_SYSTEM_PROMPT, &user_msg) {
        Ok(result) => {
            let text = result.content.trim().to_string();
            if text.len() > 1200 || text.starts_with('{') {
                eprintln!("  [LLM] NPC knowledge too long or malformed, using template");
                return None;
            }
            Some(text)
        }
        Err(e) => {
            eprintln!("  [LLM] NPC knowledge failed: {}", e);
            None
        }
    }
}
