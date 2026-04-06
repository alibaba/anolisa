//! Agent trajectory evaluation module (ported from agentevals)
//!
//! Provides LLM-as-Judge evaluation for ATIF trajectories.
//! Uses the `genai` crate for multi-provider LLM access.

pub mod types;
pub mod llm;
pub mod converter;

pub use types::{EvaluatorResult, Score, FewShotExample};
pub use llm::TrajectoryLlmJudge;
pub use converter::atif_to_chat_messages;

