// file: crates/llm/src/lib.rs
//! LLM integration — prompt assembly, API calls, validation.
//!
//! The LLM layer sits between the encounter pipeline and the player.
//! The pipeline decides WHAT happens (trigger, context, tone).
//! The LLM decides HOW it reads (prose, choices, atmosphere).
//!
//! ## Architecture
//!
//! 1. Game state → `prompt::build_*` → system + user messages
//! 2. Messages → `client::chat_completion` → raw API response
//! 3. Response → `response::parse_llm_response` → validated `SeedEvent`
//! 4. `SeedEvent` → existing CLI encounter flow (identical to seed library)
//!
//! ## Fallback
//!
//! If the LLM is unavailable, the API fails, or the response fails
//! validation, `generate::generate_encounter` returns `None` and the
//! caller falls back to the seed library. The game always works without
//! an LLM — it just has less variety.
//!
//! ## Usage
//!
//! ```ignore
//! use starbound_llm::config::LlmConfig;
//! use starbound_llm::generate::generate_encounter;
//!
//! let config = LlmConfig { enabled: true, ..Default::default() };
//! let result = generate_encounter(&config, &trigger, &system, &journey, ...);
//! match result {
//!     Some(gen) => run_encounter(&gen.event),
//!     None => /* fall back to seed library */,
//! }
//! ```

pub mod config;
pub mod client;
pub mod prompt;
pub mod response;
pub mod generate;
pub mod rumor_flavor;
pub mod npc_dialogue;