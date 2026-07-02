//! OpenAI Chat Completions 兼容层

mod converter;
mod handlers;
mod types;

pub use handlers::{post_chat_completions, post_responses};
