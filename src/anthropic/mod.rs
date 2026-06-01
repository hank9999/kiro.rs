//! Anthropic API 兼容服务模块
//!
//! 提供与 Anthropic Claude API 兼容的 HTTP 服务端点。
//!
//! # 支持的端点
//!
//! ## 标准端点 (/v1)
//! - `GET /v1/models` - 获取可用模型列表
//! - `POST /v1/messages` - 创建消息（对话）
//! - `POST /v1/messages/count_tokens` - 计算 token 数量
//!
//! ## Claude Code 兼容端点 (/cc/v1)
//! - `POST /cc/v1/messages` - 创建消息（流式响应会等待 contextUsageEvent 后再发送 message_start，确保 input_tokens 准确）
//! - `POST /cc/v1/messages/count_tokens` - 计算 token 数量（与 /v1 相同）
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
pub mod prompt_cache;
mod router;
pub mod stream;
pub mod types;
mod websearch;
mod websearch_loop;

// `create_router_with_provider` 是公开扩展点（允许外部以自定义 provider 构造路由），
// 项目内默认走 `create_router_with_shared_key`，因此本身不会触发该函数。
#[allow(unused_imports)]
pub use router::create_router_with_provider;
pub use router::create_router_with_shared_key;
