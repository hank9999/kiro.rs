//! Anthropic API 中间件

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};

use crate::common::auth;
use crate::kiro::provider::KiroProvider;

use super::types::ErrorResponse;

/// 会话级别的 token 统计
/// 用于跟踪单个 Claude Code 会话的上下文使用情况
#[derive(Debug, Default)]
struct SessionTokens {
    /// 累计的 input_tokens
    input_tokens: std::sync::atomic::AtomicI32,
    /// 累计的 output_tokens
    output_tokens: std::sync::atomic::AtomicI32,
    /// 上次更新时间戳（用于检测 /compact 后的重置）
    last_update_ms: std::sync::atomic::AtomicI64,
}

/// 会话 token 管理器
/// 按会话 ID 隔离，避免不同 Claude Code 会话之间的数据串扰
#[derive(Default)]
pub struct SessionTokenManager {
    /// 会话 ID -> token 统计
    sessions: std::sync::RwLock<std::collections::HashMap<String, Arc<SessionTokens>>>,
}

impl SessionTokenManager {
    /// 获取或创建会话的 token 统计
    fn get_or_create_session(&self, session_id: &str) -> Arc<SessionTokens> {
        // 先尝试读取
        {
            let sessions = self.sessions.read().unwrap_or_else(|e| e.into_inner());
            if let Some(session) = sessions.get(session_id) {
                return session.clone();
            }
        }
        
        // 不存在则创建
        let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        sessions
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(SessionTokens::default()))
            .clone()
    }
    
    /// 更新会话的 token 统计
    /// 
    /// 当远程返回的 context_usage_percentage 对应的 input_tokens 小于当前值时，
    /// 说明用户执行了 /compact 或上下文被压缩，此时应该接受新的较小值。
    /// 
    /// 返回最终使用的 (input_tokens, output_tokens)
    pub fn update_tokens(&self, session_id: &str, input_tokens: i32, output_tokens: i32) -> (i32, i32) {
        use std::sync::atomic::Ordering;
        
        let session = self.get_or_create_session(session_id);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        
        let current_input = session.input_tokens.load(Ordering::Acquire);
        let _last_update = session.last_update_ms.load(Ordering::Acquire);
        
        // 如果新的 input_tokens 比当前值小很多（超过 20%），说明可能执行了 /compact
        // 此时应该接受新的较小值，而不是保持旧的大值
        let significant_decrease = input_tokens < (current_input * 80 / 100);
        
        let final_input = if significant_decrease {
            // 检测到显著下降，可能是 /compact 后的结果
            // 直接使用新值
            tracing::info!(
                session_id = %session_id,
                old_input = current_input,
                new_input = input_tokens,
                "检测到 input_tokens 显著下降（可能是 /compact），接受新值"
            );
            session.input_tokens.store(input_tokens, Ordering::Release);
            input_tokens
        } else {
            // 正常情况：只增不减
            loop {
                let current = session.input_tokens.load(Ordering::Acquire);
                if input_tokens <= current {
                    break current;
                }
                match session.input_tokens.compare_exchange(
                    current,
                    input_tokens,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break input_tokens,
                    Err(_) => continue,
                }
            }
        };
        
        // output_tokens 在 /compact 后也应该重置
        let final_output = if significant_decrease {
            session.output_tokens.store(output_tokens, Ordering::Release);
            output_tokens
        } else {
            // 正常情况：只增不减
            loop {
                let current = session.output_tokens.load(Ordering::Acquire);
                if output_tokens <= current {
                    break current;
                }
                match session.output_tokens.compare_exchange(
                    current,
                    output_tokens,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break output_tokens,
                    Err(_) => continue,
                }
            }
        };
        
        // 更新时间戳
        session.last_update_ms.store(now_ms, Ordering::Release);
        
        (final_input, final_output)
    }
    
    /// 清理过期的会话（可选，用于内存管理）
    #[allow(dead_code)]
    pub fn cleanup_old_sessions(&self, max_sessions: usize) {
        let mut sessions = self.sessions.write().unwrap_or_else(|e| e.into_inner());
        if sessions.len() > max_sessions {
            // 简单策略：清空所有会话（实际应用中可以用 LRU）
            sessions.clear();
        }
    }
}

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    /// API 密钥
    pub api_key: String,
    /// Kiro Provider（可选，用于实际 API 调用）
    /// 内部使用 MultiTokenManager，已支持线程安全的多凭据管理
    pub kiro_provider: Option<Arc<KiroProvider>>,
    /// Profile ARN（可选，用于请求）
    pub profile_arn: Option<String>,
    /// 会话 token 管理器（按会话 ID 隔离）
    pub session_tokens: Arc<SessionTokenManager>,
    /// 智能摘要使用的模型
    pub summary_model: Arc<std::sync::RwLock<String>>,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            kiro_provider: None,
            profile_arn: None,
            session_tokens: Arc::new(SessionTokenManager::default()),
            summary_model: Arc::new(std::sync::RwLock::new("claude-sonnet-4.5".to_string())),
        }
    }

    /// 设置 KiroProvider
    pub fn with_kiro_provider(mut self, provider: KiroProvider) -> Self {
        self.kiro_provider = Some(Arc::new(provider));
        self
    }

    /// 设置 Profile ARN
    pub fn with_profile_arn(mut self, arn: impl Into<String>) -> Self {
        self.profile_arn = Some(arn.into());
        self
    }

    /// 设置摘要模型
    pub fn with_summary_model(mut self, model: impl Into<String>) -> Self {
        self.summary_model = Arc::new(std::sync::RwLock::new(model.into()));
        self
    }

    /// 获取当前摘要模型
    pub fn get_summary_model(&self) -> String {
        self.summary_model
            .read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| "claude-sonnet-4.5".to_string())
    }

    /// 设置摘要模型（运行时）
    pub fn set_summary_model(&self, model: impl Into<String>) {
        if let Ok(mut m) = self.summary_model.write() {
            *m = model.into();
        }
    }
    
    /// 更新会话的 token 统计，返回一致的值（只增不减）
    /// session_id 通常从请求头或消息 ID 中提取
    pub fn update_session_tokens(&self, session_id: &str, input_tokens: i32, output_tokens: i32) -> (i32, i32) {
        self.session_tokens.update_tokens(session_id, input_tokens, output_tokens)
    }
}

/// API Key 认证中间件
pub async fn auth_middleware(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    match auth::extract_api_key(&request) {
        Some(key) if auth::constant_time_eq(&key, &state.api_key) => next.run(request).await,
        _ => {
            let error = ErrorResponse::authentication_error();
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
        }
    }
}

/// CORS 中间件层
///
/// **安全说明**：当前配置允许所有来源（Any），这是为了支持公开 API 服务。
/// 如果需要更严格的安全控制，请根据实际需求配置具体的允许来源、方法和头信息。
///
/// # 配置说明
/// - `allow_origin(Any)`: 允许任何来源的请求
/// - `allow_methods(Any)`: 允许任何 HTTP 方法
/// - `allow_headers(Any)`: 允许任何请求头
pub fn cors_layer() -> tower_http::cors::CorsLayer {
    use tower_http::cors::{Any, CorsLayer};

    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}
