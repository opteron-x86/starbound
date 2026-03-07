// file: crates/llm/src/rumor_flavor.rs
//! LLM flavor pass for rumors — optional atmospheric delivery.
//!
//! Takes a single rumor's mechanical content and generates 2-3 sentences
//! of in-world delivery. Much cheaper than full encounter generation:
//! shorter prompt, shorter response, less context needed.
//!
//! Without LLM: rumors display as template text ("X is selling for Y at Z").
//! With LLM: the bartender's tone, the dock worker's nervousness.

use crate::client;
use crate::config::LlmConfig;

/// Source type for flavor delivery — shapes how the rumor is communicated.
pub enum RumorSource {
    /// Overheard at a bar or common area.
    Overheard,
    /// A dock worker or station hand mentioning it casually.
    DockWorker,
    /// A news terminal or bulletin board.
    NewsTerminal,
    /// A faction contact sharing information.
    FactionContact { name: String, title: String },
}

impl RumorSource {
    /// Description for the LLM prompt.
    fn description(&self) -> String {
        match self {
            RumorSource::Overheard => "overheard in a common area by an anonymous voice".into(),
            RumorSource::DockWorker => "mentioned casually by a dock worker or station hand".into(),
            RumorSource::NewsTerminal => "displayed on a news terminal or bulletin board".into(),
            RumorSource::FactionContact { name, title } =>
                format!("shared by {} ({}), a faction contact", name, title),
        }
    }
}

const RUMOR_SYSTEM_PROMPT: &str = "\
You are a prose writer for a space exploration game. Your job is to take \
a mechanical fact about the game world and deliver it as a short piece of \
in-world dialogue or narration.

VOICE: Awe tinged with loneliness. Quiet and restrained. People in this \
world speak like actual people — trailing off, mundane observations, \
important things buried in casual phrasing.

RULES:
- Write exactly 2-3 sentences. No more.
- Deliver the FACT accurately — don't add information that isn't there.
- Wrap the delivery in atmosphere — who's speaking, their tone, the setting.
- Use quotation marks for dialogue. No dialogue tags beyond the first sentence.
- DO NOT use exclamation marks. This world is understated.
- DO NOT explain what the fact means for the player. Just deliver it.
- Respond with ONLY the flavored text. No JSON, no labels, no preamble.";

/// Attempt to generate flavored delivery for a single rumor.
///
/// Returns the flavored text on success, or None if the LLM is
/// unavailable or fails. The caller falls back to template text.
pub fn flavor_rumor(
    config: &LlmConfig,
    mechanical_fact: &str,
    source: &RumorSource,
    location_name: &str,
) -> Option<String> {
    if !config.is_available() {
        return None;
    }

    let user_msg = format!(
        "LOCATION: {}\n\
         SOURCE: {}\n\
         FACT: {}\n\n\
         Write 2-3 sentences delivering this fact in-world.",
        location_name,
        source.description(),
        mechanical_fact,
    );

    // Single attempt — rumors aren't critical enough for retries.
    match client::chat_completion(config, RUMOR_SYSTEM_PROMPT, &user_msg) {
        Ok(result) => {
            let text = result.content.trim().to_string();
            // Basic validation: should be short and not JSON.
            if text.len() > 500 || text.starts_with('{') {
                eprintln!("  [LLM] Rumor flavor too long or malformed, using template");
                return None;
            }
            Some(text)
        }
        Err(e) => {
            eprintln!("  [LLM] Rumor flavor failed: {}", e);
            None
        }
    }
}
