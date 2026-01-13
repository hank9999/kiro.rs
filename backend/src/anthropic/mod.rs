//! Anthropic API 兼容服务模块
//!
//! 提供与 Anthropic Claude API 兼容的 HTTP 服务端点。
//!
//! # 支持的端点
//! - `GET /v1/models` - 获取可用模型列表
//! - `POST /v1/messages` - 创建消息（对话）
//! - `POST /v1/messages/count_tokens` - 计算 token 数量
//!
//! # 使用示例
//! ```rust,ignore
//! use kiro_rs::anthropic;
//!
//! let app = anthropic::create_router("your-api-key");
//! let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
//! axum::serve(listener, app).await?;
//! ```

mod converter;
mod handlers;
mod middleware;
mod router;
mod stream;
pub mod types;
mod websearch;

pub use router::create_router_with_provider;
