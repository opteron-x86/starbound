// file: crates/llm/src/client.rs
//! HTTP client for OpenRouter API calls.
//!
//! Uses blocking reqwest — synchronous, simple, no async runtime needed.
//! The player waits during generation. This is acceptable for a CLI
//! prototype; async pre-generation during travel comes later.

use serde::{Deserialize, Serialize};

use crate::config::LlmConfig;

// ---------------------------------------------------------------------------
// Request / response types (OpenAI-compatible format)
// ---------------------------------------------------------------------------

/// A chat message in the OpenAI format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Request body for the chat completions endpoint.
#[derive(Debug, Serialize)]
struct CompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    temperature: f32,
}

/// Top-level API response.
#[derive(Debug, Deserialize)]
struct CompletionResponse {
    choices: Vec<CompletionChoice>,
    #[serde(default)]
    usage: Option<UsageInfo>,
    // OpenRouter may include additional fields — ignore them.
}

#[derive(Debug, Deserialize)]
struct CompletionChoice {
    message: ChatMessage,
    // May include finish_reason, index, etc.
}

/// Token usage information (for cost tracking).
#[derive(Debug, Deserialize)]
pub struct UsageInfo {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Result of an API call.
pub struct ApiResult {
    /// The assistant's response text.
    pub content: String,
    /// Token usage, if the API reported it.
    pub usage: Option<UsageInfo>,
}

/// Error from the API layer.
#[derive(Debug)]
pub enum ApiError {
    /// No API key configured.
    NoApiKey,
    /// HTTP request failed.
    RequestFailed(String),
    /// API returned an error status.
    ApiStatus { status: u16, body: String },
    /// Response couldn't be parsed.
    ParseError(String),
    /// No choices in the response.
    EmptyResponse,
    /// Request timed out.
    Timeout,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::NoApiKey => write!(f, "No API key configured"),
            ApiError::RequestFailed(e) => write!(f, "Request failed: {}", e),
            ApiError::ApiStatus { status, body } => {
                write!(f, "API error ({}): {}", status, &body[..body.len().min(200)])
            }
            ApiError::ParseError(e) => write!(f, "Response parse error: {}", e),
            ApiError::EmptyResponse => write!(f, "API returned no choices"),
            ApiError::Timeout => write!(f, "Request timed out"),
        }
    }
}

/// Send a chat completion request to OpenRouter.
///
/// Takes a system message and a user message. Returns the assistant's
/// response text.
pub fn chat_completion(
    config: &LlmConfig,
    system_message: &str,
    user_message: &str,
) -> Result<ApiResult, ApiError> {
    let api_key = config.resolve_api_key().ok_or(ApiError::NoApiKey)?;

    let messages = vec![
        ChatMessage {
            role: "system".into(),
            content: system_message.to_string(),
        },
        ChatMessage {
            role: "user".into(),
            content: user_message.to_string(),
        },
    ];

    let request_body = CompletionRequest {
        model: config.model.clone(),
        messages,
        max_tokens: config.max_tokens,
        temperature: config.temperature,
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(config.timeout_secs))
        .build()
        .map_err(|e| ApiError::RequestFailed(e.to_string()))?;

    let response = client
        .post(&config.endpoint)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .header("HTTP-Referer", "https://github.com/opteron-x86/starbound")
        .header("X-Title", "Starbound")
        .json(&request_body)
        .send()
        .map_err(|e| {
            if e.is_timeout() {
                ApiError::Timeout
            } else {
                ApiError::RequestFailed(e.to_string())
            }
        })?;

    let status = response.status().as_u16();

    // Read raw bytes first — more robust than .text() which can fail on encoding.
    let bytes = response.bytes().map_err(|e| {
        ApiError::RequestFailed(format!("Failed to read response body: {}", e))
    })?;
    let body = String::from_utf8_lossy(&bytes).to_string();

    if status != 200 {
        return Err(ApiError::ApiStatus { status, body });
    }

    let parsed: CompletionResponse = serde_json::from_str(&body).map_err(|e| {
        // Include the first part of the body for debugging.
        let preview = &body[..body.len().min(500)];
        ApiError::ParseError(format!("{}\n  Response preview: {}", e, preview))
    })?;

    let choice = parsed.choices.into_iter().next().ok_or(ApiError::EmptyResponse)?;

    Ok(ApiResult {
        content: choice.message.content,
        usage: parsed.usage,
    })
}