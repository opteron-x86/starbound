// file: crates/llm/src/config.rs
//! Configuration for LLM API calls via OpenRouter.

use serde::{Deserialize, Serialize};

/// Configuration for the LLM integration layer.
///
/// API key can be provided directly or read from the
/// `OPENROUTER_API_KEY` environment variable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// OpenRouter API endpoint.
    pub endpoint: String,
    /// API key. If empty, reads from `OPENROUTER_API_KEY` env var.
    pub api_key: String,
    /// Model identifier (OpenRouter format).
    /// e.g. "anthropic/claude-sonnet-4", "google/gemini-2.5-flash",
    ///      "openai/gpt-4o", "meta-llama/llama-3-70b-instruct"
    pub model: String,
    /// Maximum tokens for the response. Must be high enough for
    /// full JSON event output (~2000-2500 tokens typical).
    pub max_tokens: u32,
    /// Temperature (0.0–2.0). Lower = more deterministic.
    pub temperature: f32,
    /// Whether LLM generation is enabled. When false, the system
    /// falls back to seed events only.
    pub enabled: bool,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://openrouter.ai/api/v1/chat/completions".into(),
            api_key: String::new(),
            model: "xiaomi/mimo-v2-flash".into(),
            max_tokens: 2000,
            temperature: 0.8,
            enabled: false,
            timeout_secs: 30,
        }
    }
}

impl LlmConfig {
    /// Resolve the API key — uses the configured key if present,
    /// otherwise reads from environment.
    pub fn resolve_api_key(&self) -> Option<String> {
        if !self.api_key.is_empty() {
            Some(self.api_key.clone())
        } else {
            std::env::var("OPENROUTER_API_KEY").ok()
        }
    }

    /// Resolve the model — uses the configured model if non-empty,
    /// otherwise reads from `OPENROUTER_MODEL` env var, then falls
    /// back to the compiled-in default.
    pub fn resolve_model(&self) -> String {
        if !self.model.is_empty() {
            return self.model.clone();
        }
        std::env::var("OPENROUTER_MODEL")
            .unwrap_or_else(|_| Self::default().model)
    }

    /// Whether the LLM is available (enabled + has API key).
    pub fn is_available(&self) -> bool {
        self.enabled && self.resolve_api_key().is_some()
    }
}