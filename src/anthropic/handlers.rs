//! Anthropic API Handler 函数

use std::{convert::Infallible, sync::Arc};

use crate::kiro::model::events::Event;
use crate::kiro::model::requests::kiro::KiroRequest;
use crate::kiro::parser::decoder::EventStreamDecoder;
use crate::token;
use axum::{
    Json as JsonExtractor,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use serde_json::json;
use std::time::Duration;
use tokio::time::interval;
use uuid::Uuid;

use crate::stats::StatsStore;

use super::converter::{ConversionError, ConversionResult, convert_request, rebuild_with_truncated_history};
use super::history_manager::{HistoryManager, KiroSummaryGenerator};
use super::history_store::global_store;
use super::middleware::AppState;
use super::stream::{BufferedStreamContext, SseEvent, StreamContext};
use super::types::{
    CountTokensRequest, CountTokensResponse, ErrorResponse, MessagesRequest, Model, ModelsResponse,
};
use super::websearch;

/// GET /v1/models
///
/// 返回可用的模型列表
pub async fn get_models() -> impl IntoResponse {
    tracing::info!("Received GET /v1/models request");

    let models = vec![
        Model {
            id: "claude-sonnet-4-5-20250929".to_string(),
            object: "model".to_string(),
            created: 1727568000,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
        Model {
            id: "claude-opus-4-5-20251101".to_string(),
            object: "model".to_string(),
            created: 1730419200,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
        Model {
            id: "claude-haiku-4-5-20251001".to_string(),
            object: "model".to_string(),
            created: 1727740800,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Haiku 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
        },
    ];

    Json(ModelsResponse {
        object: "list".to_string(),
        data: models,
    })
}

/// POST /v1/messages
///
/// 创建消息（对话）
pub async fn post_messages(
    State(state): State<AppState>,
    JsonExtractor(payload): JsonExtractor<MessagesRequest>,
) -> Response {
    // 从 metadata.user_id 中提取会话 ID
    // 格式: user_xxx_account__session_0b4445e1-f5be-49e1-87ce-62bbc28ad705
    let session_id = payload
        .metadata
        .as_ref()
        .and_then(|m| m.user_id.as_ref())
        .and_then(|uid| uid.split("__session_").nth(1))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "default".to_string());
    
    tracing::info!(
        model = %payload.model,
        max_tokens = %payload.max_tokens,
        stream = %payload.stream,
        message_count = %payload.messages.len(),
        session_id = %session_id,
        "Received POST /v1/messages request"
    );
    // 检查 KiroProvider 是否可用
    let provider = match &state.kiro_provider {
        Some(p) => p.clone(),
        None => {
            tracing::error!("KiroProvider 未配置");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "service_unavailable",
                    "Kiro API provider not configured",
                )),
            )
                .into_response();
        }
    };

    // 检查是否为 WebSearch 请求
    if websearch::has_web_search_tool(&payload) {
        tracing::info!("检测到 WebSearch 工具，路由到 WebSearch 处理");

        // 估算输入 tokens
        let input_tokens = token::count_all_tokens(
            payload.model.clone(),
            payload.system.clone(),
            payload.messages.clone(),
            payload.tools.clone(),
        ) as i32;

        return websearch::handle_websearch_request(state, provider, &payload, input_tokens, &session_id).await;
    }

    // 转换请求
    let conversion_result = match convert_request(&payload) {
        Ok(result) => result,
        Err(e) => {
            let (error_type, message) = match &e {
                ConversionError::UnsupportedModel(model) => {
                    ("invalid_request_error", format!("模型不支持: {}", model))
                }
                ConversionError::EmptyMessages => {
                    ("invalid_request_error", "消息列表为空".to_string())
                }
                ConversionError::InvalidRequest(msg) => {
                    ("invalid_request_error", msg.clone())
                }
            };
            tracing::warn!("请求转换失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(error_type, message)),
            )
                .into_response();
        }
    };

    // 保存历史记录到文件
    if let Err(e) = global_store().save(&session_id, conversion_result.original_history.clone()) {
        tracing::warn!(session_id = %session_id, "保存历史记录失败: {}", e);
    }

    // 构建 Kiro 请求
    let kiro_request = KiroRequest {
        conversation_state: conversion_result.conversation_state.clone(),
        profile_arn: state.profile_arn.clone(),
    };

    let request_body = match serde_json::to_string(&kiro_request) {
        Ok(body) => body,
        Err(e) => {
            tracing::error!("序列化请求失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::new(
                    "internal_error",
                    format!("序列化请求失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    tracing::debug!("Kiro request body: {}", request_body);

    // 估算输入 tokens
    let input_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system,
        payload.messages,
        payload.tools,
    ) as i32;

    // 检查是否启用了thinking
    let thinking_enabled = payload
        .thinking
        .as_ref()
        .map(|t| t.thinking_type == "enabled")
        .unwrap_or(false);

    if payload.stream {
        // 流式响应
        handle_stream_request(
            state,
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            thinking_enabled,
            &session_id,
            conversion_result,
        )
        .await
    } else {
        // 非流式响应
        handle_non_stream_request(
            state,
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            &session_id,
            conversion_result,
        )
        .await
    }
}

/// 处理流式请求
async fn handle_stream_request(
    state: AppState,
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    thinking_enabled: bool,
    session_id: &str,
    conversion_result: ConversionResult,
) -> Response {
    // 截断重试配置（带摘要支持）
    let mut history_manager = HistoryManager::with_defaults()
        .with_cache_key(session_id.to_string());
    let max_retries = 2;
    let mut retry_count = 0;
    let mut current_history = conversion_result.original_history.clone();
    let mut current_request_body = request_body.to_string();

    // 创建摘要生成器（使用配置的模型）
    let summary_model = state.get_summary_model();
    let summary_generator = KiroSummaryGenerator::new(
        provider.clone(),
        state.profile_arn.clone(),
    ).with_model(&summary_model);

    loop {
        // 调用 Kiro API（支持多凭据故障转移）
        let api_result = provider
            .call_api_stream_with_credential_id(&current_request_body, Some(model))
            .await;

        match api_result {
            Ok((credential_id, response)) => {
                let stats = provider.stats_store();
                let model = model.to_string();
                let request_body_clone = current_request_body.clone();

                // 创建流处理上下文
                let mut ctx = StreamContext::new_with_thinking(model.clone(), input_tokens, thinking_enabled);

                // 生成初始事件
                let initial_events = ctx.generate_initial_events();

                // 创建 SSE 流（支持流中断自动重试）
                let stream = create_sse_stream(
                    response,
                    ctx,
                    initial_events,
                    stats,
                    credential_id,
                    model,
                    provider,
                    request_body_clone,
                    state,
                    session_id.to_string(),
                );

                // 返回 SSE 响应
                return match Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .header(header::CACHE_CONTROL, "no-cache")
                    .header(header::CONNECTION, "keep-alive")
                    .body(Body::from_stream(stream))
                {
                    Ok(resp) => resp,
                    Err(e) => {
                        tracing::error!("构建 SSE 响应失败: {}", e);
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(ErrorResponse::new(
                                "internal_error",
                                format!("构建 SSE 响应失败: {}", e),
                            )),
                        )
                            .into_response()
                    }
                };
            }
            Err(e) => {
                let error_msg = e.to_string();

                // 检查是否为内容长度超限错误
                if error_msg.starts_with("ContentLengthExceeded:") {
                    // 尝试截断重试（带摘要）
                    if retry_count < max_retries {
                        let (truncated_history, should_retry) = history_manager
                            .handle_length_error_with_summary(
                                current_history.clone(),
                                &conversion_result.model_id,
                                retry_count,
                                Some(&summary_generator),
                            )
                            .await;

                        if should_retry {
                            tracing::info!(
                                "内容长度超限（流式），尝试{}重试 (第 {} 次): {}",
                                if history_manager.truncate_info().used_summary { "摘要" } else { "截断" },
                                retry_count + 1,
                                history_manager.truncate_info().message
                            );

                            // 修复截断后的历史消息
                            let fixed_history =
                                HistoryManager::fix_history_after_truncate(truncated_history);

                            // 重新构建请求
                            let new_state =
                                rebuild_with_truncated_history(&conversion_result, fixed_history.clone());
                            let new_request = KiroRequest {
                                conversation_state: new_state,
                                profile_arn: state.profile_arn.clone(),
                            };

                            match serde_json::to_string(&new_request) {
                                Ok(body) => {
                                    current_request_body = body;
                                    current_history = fixed_history;
                                    retry_count += 1;
                                    continue;
                                }
                                Err(e) => {
                                    tracing::error!("重新序列化请求失败: {}", e);
                                }
                            }
                        }
                    }

                    // 截断重试失败或达到最大重试次数，返回提示信息
                    tracing::info!("内容长度超限（流式），截断重试失败，返回提示信息");

                    // 返回一个 SSE 流，包含提示信息
                    let model_clone = model.to_string();
                    let stream = stream::iter(vec![
                        // message_start
                        Ok::<Bytes, Infallible>(Bytes::from(format!(
                            "event: message_start\ndata: {}\n\n",
                            serde_json::to_string(&json!({
                                "type": "message_start",
                                "message": {
                                    "id": format!("msg_{}", Uuid::new_v4().simple()),
                                    "type": "message",
                                    "role": "assistant",
                                    "content": [],
                                    "model": &model_clone,
                                    "stop_reason": null,
                                    "stop_sequence": null,
                                    "usage": {
                                        "input_tokens": input_tokens,
                                        "output_tokens": 0
                                    }
                                }
                            })).unwrap_or_default()
                        ))),
                        // content_block_start
                        Ok(Bytes::from(format!(
                            "event: content_block_start\ndata: {}\n\n",
                            serde_json::to_string(&json!({
                                "type": "content_block_start",
                                "index": 0,
                                "content_block": {
                                    "type": "text",
                                    "text": ""
                                }
                            })).unwrap_or_default()
                        ))),
                        // content_block_delta
                        Ok(Bytes::from(format!(
                            "event: content_block_delta\ndata: {}\n\n",
                            serde_json::to_string(&json!({
                                "type": "content_block_delta",
                                "index": 0,
                                "delta": {
                                    "type": "text_delta",
                                    "text": "⚠️ 上下文长度超过限制。\n\n建议操作：\n1. 使用 /compact 命令压缩对话历史\n2. 使用 /context 查看当前上下文使用情况\n3. 考虑将长对话拆分为多个会话"
                                }
                            })).unwrap_or_default()
                        ))),
                        // content_block_stop
                        Ok(Bytes::from(format!(
                            "event: content_block_stop\ndata: {}\n\n",
                            serde_json::to_string(&json!({
                                "type": "content_block_stop",
                                "index": 0
                            })).unwrap_or_default()
                        ))),
                        // message_delta
                        Ok(Bytes::from(format!(
                            "event: message_delta\ndata: {}\n\n",
                            serde_json::to_string(&json!({
                                "type": "message_delta",
                                "delta": {
                                    "stop_reason": "end_turn",
                                    "stop_sequence": null
                                },
                                "usage": {
                                    "input_tokens": input_tokens,
                                    "output_tokens": 100
                                }
                            })).unwrap_or_default()
                        ))),
                        // message_stop
                        Ok(Bytes::from(format!(
                            "event: message_stop\ndata: {}\n\n",
                            serde_json::to_string(&json!({
                                "type": "message_stop"
                            })).unwrap_or_default()
                        ))),
                    ]);

                    return Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/event-stream")
                        .header(header::CACHE_CONTROL, "no-cache")
                        .header(header::CONNECTION, "keep-alive")
                        .body(Body::from_stream(stream))
                        .unwrap_or_else(|e| {
                            tracing::error!("构建 SSE 响应失败: {}", e);
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(ErrorResponse::new(
                                    "internal_error",
                                    format!("构建 SSE 响应失败: {}", e),
                                )),
                            )
                                .into_response()
                        });
                }

                // 其他错误返回 502
                tracing::error!("Kiro API 调用失败: {}", error_msg);
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(ErrorResponse::new(
                        "api_error",
                        format!("上游 API 调用失败: {}", error_msg),
                    )),
                )
                    .into_response();
            }
        }
    }
}

/// Ping 事件间隔（25秒）
const PING_INTERVAL_SECS: u64 = 25;

/// 流中断重试的最小输出 tokens 阈值
/// 只有在流读取**出错**且输出少于此值时才尝试重试
/// 正常EOF结束不触发重试（即使输出很短）
const STREAM_RETRY_MIN_OUTPUT_TOKENS: i32 = 100;

/// 流中断最大重试次数
const STREAM_MAX_RETRIES: usize = 2;

/// 创建 ping 事件的 SSE 字符串
fn create_ping_sse() -> Bytes {
    Bytes::from("event: ping\ndata: {\"type\": \"ping\"}\n\n")
}

/// 流处理状态
struct StreamState {
    body_stream: futures::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>,
    ctx: StreamContext,
    decoder: EventStreamDecoder,
    finished: bool,
    ping_interval: tokio::time::Interval,
    stats: Option<Arc<StatsStore>>,
    credential_id: u64,
    model: String,
    provider: Arc<crate::kiro::provider::KiroProvider>,
    request_body: String,
    retry_count: usize,
    /// 应用状态，用于跨请求维护 token 一致性
    app_state: AppState,
    /// 会话 ID，用于隔离不同 Claude Code 会话
    session_id: String,
}

/// 创建 SSE 事件流（支持流中断自动重试）
fn create_sse_stream(
    response: reqwest::Response,
    ctx: StreamContext,
    initial_events: Vec<SseEvent>,
    stats: Option<Arc<StatsStore>>,
    credential_id: u64,
    model: String,
    provider: Arc<crate::kiro::provider::KiroProvider>,
    request_body: String,
    app_state: AppState,
    session_id: String,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    // 先发送初始事件
    let initial_stream = stream::iter(
        initial_events
            .into_iter()
            .map(|e| Ok(Bytes::from(e.to_sse_string()))),
    );

    // 然后处理 Kiro 响应流，同时每25秒发送 ping 保活
    let body_stream = response.bytes_stream();

    let state = StreamState {
        body_stream: Box::pin(body_stream),
        ctx,
        decoder: EventStreamDecoder::new(),
        finished: false,
        ping_interval: interval(Duration::from_secs(PING_INTERVAL_SECS)),
        stats,
        credential_id,
        model,
        provider,
        request_body,
        retry_count: 0,
        app_state,
        session_id,
    };

    let processing_stream = stream::unfold(state, |mut state| async move {
        if state.finished {
            return None;
        }

        // 使用 select! 同时等待数据和 ping 定时器
        tokio::select! {
            // 处理数据流
            chunk_result = state.body_stream.next() => {
                match chunk_result {
                    Some(Ok(chunk)) => {
                        // 解码事件
                        if let Err(e) = state.decoder.feed(&chunk) {
                            tracing::warn!("缓冲区溢出: {}", e);
                        }

                        let mut events = Vec::new();
                        for result in state.decoder.decode_iter() {
                            match result {
                                Ok(frame) => {
                                    let message_type = frame
                                        .message_type()
                                        .unwrap_or("unknown")
                                        .to_string();
                                    let event_type = frame
                                        .event_type()
                                        .unwrap_or("unknown")
                                        .to_string();
                                    let payload_len = frame.payload.len();

                                    match Event::from_frame(frame) {
                                        Ok(event) => {
                                            let sse_events = state.ctx.process_kiro_event(&event);
                                            events.extend(sse_events);
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                message_id = %state.ctx.message_id,
                                                message_type = %message_type,
                                                event_type = %event_type,
                                                payload_len = payload_len,
                                                "解析上游事件失败: {}",
                                                e
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("解码事件失败: {}", e);
                                }
                            }
                        }

                        // 转换为 SSE 字节流
                        let bytes: Vec<Result<Bytes, Infallible>> = events
                            .into_iter()
                            .map(|e| Ok(Bytes::from(e.to_sse_string())))
                            .collect();

                        Some((stream::iter(bytes), state))
                    }
                    Some(Err(e)) => {
                        // 流读取错误，检查是否可以重试
                        let can_retry = state.retry_count < STREAM_MAX_RETRIES
                            && state.ctx.output_tokens < STREAM_RETRY_MIN_OUTPUT_TOKENS;

                        if can_retry {
                            tracing::warn!(
                                message_id = %state.ctx.message_id,
                                output_tokens = state.ctx.output_tokens,
                                retry_count = state.retry_count,
                                decoded_frames = state.decoder.frames_decoded(),
                                "读取上游响应流失败，尝试重试: {}",
                                e
                            );

                            // 尝试重新建立连接
                            match state.provider
                                .call_api_stream_with_credential_id(&state.request_body, Some(&state.model))
                                .await
                            {
                                Ok((new_credential_id, new_response)) => {
                                    tracing::info!(
                                        message_id = %state.ctx.message_id,
                                        retry_count = state.retry_count + 1,
                                        new_credential_id = new_credential_id,
                                        "流重试成功，已重新建立连接"
                                    );

                                    // 更新状态
                                    state.body_stream = Box::pin(new_response.bytes_stream());
                                    state.decoder = EventStreamDecoder::new();
                                    state.credential_id = new_credential_id;
                                    state.retry_count += 1;

                                    // 发送 ping 保持连接活跃
                                    let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                                    return Some((stream::iter(bytes), state));
                                }
                                Err(retry_err) => {
                                    tracing::error!(
                                        message_id = %state.ctx.message_id,
                                        retry_count = state.retry_count,
                                        "流重试失败: {}",
                                        retry_err
                                    );
                                    // 重试失败，继续走正常的错误处理流程
                                }
                            }
                        }

                        // 无法重试或重试失败，记录错误并结束
                        tracing::error!(
                            message_id = %state.ctx.message_id,
                            output_tokens = state.ctx.output_tokens,
                            decoded_frames = state.decoder.frames_decoded(),
                            decoder_buffer_len = state.decoder.buffer_len(),
                            decoder_error_count = state.decoder.error_count(),
                            decoder_bytes_skipped = state.decoder.bytes_skipped(),
                            retry_count = state.retry_count,
                            "读取上游响应流失败（已放弃重试）: {}",
                            e
                        );

                        if let Some(s) = &state.stats {
                            s.record_error(state.credential_id, Some(&state.model), format!("读取上游响应流失败: {}", e));
                        }

                        // 使用会话级别状态确保 token 一致性（只增不减）
                        let raw_input_tokens = state.ctx.context_input_tokens.unwrap_or(state.ctx.input_tokens);
                        let (consistent_input, consistent_output) = state.app_state.update_session_tokens(
                            &state.session_id,
                            raw_input_tokens,
                            state.ctx.output_tokens,
                        );
                        state.ctx.context_input_tokens = Some(consistent_input);
                        state.ctx.output_tokens = consistent_output;

                        let final_events = state.ctx.generate_final_events();

                        // 记录用量（即使流中断，也尽量把已输出部分计入）
                        if let Some(s) = &state.stats {
                            s.add_usage(
                                state.credential_id,
                                Some(&state.model),
                                consistent_input as i64,
                                consistent_output as i64,
                            );
                        }

                        state.finished = true;
                        let bytes: Vec<Result<Bytes, Infallible>> = final_events
                            .into_iter()
                            .map(|e| Ok(Bytes::from(e.to_sse_string())))
                            .collect();
                        Some((stream::iter(bytes), state))
                    }
                    None => {
                        // 流正常结束（EOF）
                        // 检查是否收到上游错误事件，如果是则尝试重试或返回错误
                        let received_error = state.ctx.received_upstream_error;
                        let is_abnormally_short = state.ctx.output_tokens < STREAM_RETRY_MIN_OUTPUT_TOKENS;
                        
                        // 当收到上游错误事件且输出很短时，返回 SSE error 事件让 Claude Code 重试
                        if received_error && is_abnormally_short {
                            let error_msg = state.ctx.upstream_error_message
                                .as_deref()
                                .unwrap_or("Upstream service error");
                            
                            tracing::warn!(
                                message_id = %state.ctx.message_id,
                                output_tokens = state.ctx.output_tokens,
                                error_message = error_msg,
                                "收到上游错误事件且输出异常短，返回 SSE error 让 Claude Code 重试"
                            );

                            // 记录错误
                            if let Some(s) = &state.stats {
                                s.record_error(
                                    state.credential_id,
                                    Some(&state.model),
                                    format!("上游错误: {}", error_msg),
                                );
                            }

                            state.finished = true;
                            
                            // 返回 SSE error 事件，Claude Code 会自动重试
                            let error_event = format!(
                                "event: error\ndata: {}\n\n",
                                serde_json::json!({
                                    "type": "error",
                                    "error": {
                                        "type": "api_error",
                                        "message": error_msg
                                    }
                                })
                            );
                            let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(Bytes::from(error_event))];
                            return Some((stream::iter(bytes), state));
                        }

                        if is_abnormally_short {
                            tracing::warn!(
                                message_id = %state.ctx.message_id,
                                output_tokens = state.ctx.output_tokens,
                                input_tokens = state.ctx.input_tokens,
                                decoded_frames = state.decoder.frames_decoded(),
                                retry_count = state.retry_count,
                                received_upstream_error = received_error,
                                "检测到异常短响应，上游API可能提前终止。建议检查：1)上游服务状态 2)Token额度 3)请求频率限制"
                            );
                        }

                        tracing::info!(
                            message_id = %state.ctx.message_id,
                            output_tokens = state.ctx.output_tokens,
                            decoded_frames = state.decoder.frames_decoded(),
                            decoder_buffer_len = state.decoder.buffer_len(),
                            decoder_error_count = state.decoder.error_count(),
                            decoder_bytes_skipped = state.decoder.bytes_skipped(),
                            retry_count = state.retry_count,
                            abnormally_short = is_abnormally_short,
                            received_upstream_error = received_error,
                            "上游响应流结束（EOF）"
                        );

                        // 使用会话级别状态确保 token 一致性（只增不减）
                        let raw_input_tokens = state.ctx.context_input_tokens.unwrap_or(state.ctx.input_tokens);
                        let (consistent_input, consistent_output) = state.app_state.update_session_tokens(
                            &state.session_id,
                            raw_input_tokens,
                            state.ctx.output_tokens,
                        );
                        state.ctx.context_input_tokens = Some(consistent_input);
                        state.ctx.output_tokens = consistent_output;

                        let final_events = state.ctx.generate_final_events();

                        // 正常结束：记录成功 + 用量
                        if let Some(s) = &state.stats {
                            s.record_success(state.credential_id, Some(&state.model));
                            s.add_usage(
                                state.credential_id,
                                Some(&state.model),
                                consistent_input as i64,
                                consistent_output as i64,
                            );
                        }

                        state.finished = true;
                        let bytes: Vec<Result<Bytes, Infallible>> = final_events
                            .into_iter()
                            .map(|e| Ok(Bytes::from(e.to_sse_string())))
                            .collect();
                        Some((stream::iter(bytes), state))
                    }
                }
            }
            // 发送 ping 保活
            _ = state.ping_interval.tick() => {
                tracing::trace!("发送 ping 保活事件");
                let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                Some((stream::iter(bytes), state))
            }
        }
    })
    .flatten();

    initial_stream.chain(processing_stream)
}

/// Claude Code 上下文窗口大小（200k tokens）
const CONTEXT_WINDOW_SIZE: i32 = 200_000;

/// 输出警告的上下文使用率阈值（百分比）
const CONTEXT_WARNING_THRESHOLD: f64 = 80.0;

/// 处理非流式请求
async fn handle_non_stream_request(
    state: AppState,
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    session_id: &str,
    conversion_result: ConversionResult,
) -> Response {
    // 截断重试配置（带摘要支持）
    let mut history_manager = HistoryManager::with_defaults()
        .with_cache_key(session_id.to_string());
    let max_retries = 2;
    let mut retry_count = 0;
    let mut current_history = conversion_result.original_history.clone();
    let mut current_request_body = request_body.to_string();

    // 创建摘要生成器（使用配置的模型）
    let summary_model = state.get_summary_model();
    let summary_generator = KiroSummaryGenerator::new(
        provider.clone(),
        state.profile_arn.clone(),
    ).with_model(&summary_model);

    loop {
        // 调用 Kiro API（支持多凭据故障转移）
        let api_result = provider
            .call_api_with_credential_id(&current_request_body, Some(model))
            .await;

        match api_result {
            Ok((credential_id, response)) => {
                let stats = provider.stats_store();

                // 读取响应体
                let body_bytes = match response.bytes().await {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        tracing::error!("读取响应体失败: {}", e);
                        if let Some(s) = &stats {
                            s.record_error(credential_id, Some(model), format!("读取响应体失败: {}", e));
                        }
                        return (
                            StatusCode::BAD_GATEWAY,
                            Json(ErrorResponse::new(
                                "api_error",
                                format!("读取响应失败: {}", e),
                            )),
                        )
                            .into_response();
                    }
                };

                // 解析事件流并返回响应
                return process_non_stream_response(
                    body_bytes,
                    model,
                    input_tokens,
                    credential_id,
                    stats,
                    session_id,
                    &state,
                )
                .await;
            }
            Err(e) => {
                let error_msg = e.to_string();

                // 检查是否为内容长度超限错误
                if error_msg.starts_with("ContentLengthExceeded:") {
                    // 尝试截断重试（带摘要）
                    if retry_count < max_retries {
                        let (truncated_history, should_retry) = history_manager
                            .handle_length_error_with_summary(
                                current_history.clone(),
                                &conversion_result.model_id,
                                retry_count,
                                Some(&summary_generator),
                            )
                            .await;

                        if should_retry {
                            tracing::info!(
                                "内容长度超限，尝试{}重试 (第 {} 次): {}",
                                if history_manager.truncate_info().used_summary { "摘要" } else { "截断" },
                                retry_count + 1,
                                history_manager.truncate_info().message
                            );

                            // 修复截断后的历史消息
                            let fixed_history =
                                HistoryManager::fix_history_after_truncate(truncated_history);

                            // 重新构建请求
                            let new_state =
                                rebuild_with_truncated_history(&conversion_result, fixed_history.clone());
                            let new_request = KiroRequest {
                                conversation_state: new_state,
                                profile_arn: state.profile_arn.clone(),
                            };

                            match serde_json::to_string(&new_request) {
                                Ok(body) => {
                                    current_request_body = body;
                                    current_history = fixed_history;
                                    retry_count += 1;
                                    continue;
                                }
                                Err(e) => {
                                    tracing::error!("重新序列化请求失败: {}", e);
                                }
                            }
                        }
                    }

                    // 截断重试失败或达到最大重试次数，返回提示信息
                    tracing::info!("内容长度超限，截断重试失败，返回提示信息");
                    let response_body = json!({
                        "id": format!("msg_{}", Uuid::new_v4().simple()),
                        "type": "message",
                        "role": "assistant",
                        "content": [{
                            "type": "text",
                            "text": "⚠️ 上下文长度超过限制。\n\n建议操作：\n1. 使用 /compact 命令压缩对话历史\n2. 使用 /context 查看当前上下文使用情况\n3. 考虑将长对话拆分为多个会话"
                        }],
                        "model": model,
                        "stop_reason": "end_turn",
                        "stop_sequence": null,
                        "usage": {
                            "input_tokens": input_tokens,
                            "output_tokens": 100
                        }
                    });

                    return (StatusCode::OK, Json(response_body)).into_response();
                }

                // 其他错误返回 502
                tracing::error!("Kiro API 调用失败: {}", error_msg);
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(ErrorResponse::new(
                        "api_error",
                        format!("上游 API 调用失败: {}", error_msg),
                    )),
                )
                    .into_response();
            }
        }
    }
}

/// 处理非流式响应的解析和返回
async fn process_non_stream_response(
    body_bytes: Bytes,
    model: &str,
    input_tokens: i32,
    credential_id: u64,
    stats: Option<Arc<StatsStore>>,
    session_id: &str,
    app_state: &AppState,
) -> Response {
    // 解析事件流
    let mut decoder = EventStreamDecoder::new();
    if let Err(e) = decoder.feed(&body_bytes) {
        tracing::warn!("缓冲区溢出: {}", e);
    }

    let mut text_content = String::new();
    let mut tool_uses: Vec<serde_json::Value> = Vec::new();
    let mut has_tool_use = false;
    let mut stop_reason = "end_turn".to_string();
    // 从 contextUsageEvent 计算的实际输入 tokens
    let mut context_input_tokens: Option<i32> = None;

    // 收集工具调用的增量 JSON
    let mut tool_json_buffers: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // 工具名称缓存 (tool_use_id -> tool_name)
    let mut tool_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for result in decoder.decode_iter() {
        match result {
            Ok(frame) => {
                let message_type = frame.message_type().unwrap_or("unknown").to_string();
                let event_type = frame.event_type().unwrap_or("unknown").to_string();
                let payload_len = frame.payload.len();

                match Event::from_frame(frame) {
                    Ok(event) => match event {
                        Event::AssistantResponse(resp) => {
                            text_content.push_str(&resp.content);
                        }
                        Event::ToolUse(tool_use) => {
                            has_tool_use = true;

                            // 缓存工具名称
                            if !tool_use.name.is_empty() {
                                tool_names.insert(tool_use.tool_use_id.clone(), tool_use.name.clone());
                            }

                            // 累积工具的 JSON 输入
                            let buffer = tool_json_buffers
                                .entry(tool_use.tool_use_id.clone())
                                .or_insert_with(String::new);
                            buffer.push_str(&tool_use.input);

                            // 如果是完整的工具调用，添加到列表
                            if tool_use.stop {
                                let input: serde_json::Value = serde_json::from_str(buffer)
                                    .unwrap_or_else(|e| {
                                        tracing::warn!(
                                            "工具输入 JSON 解析失败: {}, tool_use_id: {}, 原始内容: {}",
                                            e, tool_use.tool_use_id, buffer
                                        );
                                        serde_json::json!({})
                                    });

                                // 获取工具名称（优先使用当前事件的 name，否则从缓存获取）
                                let tool_name = if !tool_use.name.is_empty() {
                                    tool_use.name.clone()
                                } else {
                                    tool_names.get(&tool_use.tool_use_id).cloned().unwrap_or_default()
                                };

                                tool_uses.push(json!({
                                    "type": "tool_use",
                                    "id": tool_use.tool_use_id,
                                    "name": tool_name,
                                    "input": input
                                }));
                            }
                        }
                        Event::ContextUsage(context_usage) => {
                            // 用 200K 窗口计算实际使用的 tokens
                            // 公式: percentage * 200000 / 100
                            let actual_tokens = (context_usage.context_usage_percentage
                                * (CONTEXT_WINDOW_SIZE as f64)
                                / 100.0)
                                as i32;
                            
                            context_input_tokens = Some(actual_tokens);
                            
                            // 当上下文使用率 >= 80% 时输出警告
                            if context_usage.context_usage_percentage >= CONTEXT_WARNING_THRESHOLD {
                                tracing::warn!(
                                    "⚠️ 上下文使用率较高: {:.1}% (约 {} tokens / 200K)",
                                    context_usage.context_usage_percentage,
                                    actual_tokens
                                );
                            }
                            
                            tracing::debug!(
                                "contextUsageEvent: {:.1}% -> {} tokens",
                                context_usage.context_usage_percentage,
                                actual_tokens
                            );
                        }
                        Event::Exception { exception_type, .. } => {
                            tracing::warn!("收到异常事件: {}", exception_type);
                        }
                        _ => {}
                    },
                    Err(e) => {
                        tracing::warn!(
                            credential_id = credential_id,
                            model = %model,
                            message_type = %message_type,
                            event_type = %event_type,
                            payload_len = payload_len,
                            "解析上游事件失败: {}",
                            e
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!("解码事件失败: {}", e);
            }
        }
    }

    // 确定 stop_reason
    if has_tool_use && stop_reason == "end_turn" {
        stop_reason = "tool_use".to_string();
    }

    // 构建响应内容
    let mut content: Vec<serde_json::Value> = Vec::new();

    if !text_content.is_empty() {
        content.push(json!({
            "type": "text",
            "text": text_content
        }));
    }

    content.extend(tool_uses);

    // 估算输出 tokens
    let output_tokens = token::estimate_output_tokens(&content);

    // 使用从 contextUsageEvent 计算的 input_tokens，如果没有则使用估算值
    let raw_input_tokens = context_input_tokens.unwrap_or(input_tokens);
    
    // 使用会话级别状态确保 token 一致性
    let (consistent_input, consistent_output) = app_state.update_session_tokens(
        session_id,
        raw_input_tokens,
        output_tokens,
    );

    // 记录成功 + 用量（按最终使用的凭据归集）
    if let Some(s) = &stats {
        s.record_success(credential_id, Some(model));
        s.add_usage(
            credential_id,
            Some(model),
            consistent_input as i64,
            consistent_output as i64,
        );
    }

    // 构建 Anthropic 响应
    let response_body = json!({
        "id": format!("msg_{}", Uuid::new_v4().to_string().replace('-', "")),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": consistent_input,
            "output_tokens": consistent_output
        }
    });

    (StatusCode::OK, Json(response_body)).into_response()
}

/// POST /v1/messages/count_tokens
///
/// 计算消息的 token 数量
pub async fn count_tokens(
    JsonExtractor(payload): JsonExtractor<CountTokensRequest>,
) -> impl IntoResponse {
    tracing::info!(
        model = %payload.model,
        message_count = %payload.messages.len(),
        "Received POST /v1/messages/count_tokens request"
    );

    let total_tokens = token::count_all_tokens(
        payload.model,
        payload.system,
        payload.messages,
        payload.tools,
    ) as i32;

    Json(CountTokensResponse {
        input_tokens: total_tokens.max(1) as i32,
    })
}

/// POST /cc/v1/messages
///
/// Claude Code 兼容端点，与 /v1/messages 的区别在于：
/// - 流式响应会等待 kiro 端返回 contextUsageEvent 后再发送 message_start
/// - message_start 中的 input_tokens 是从 contextUsageEvent 计算的准确值
pub async fn post_messages_cc(
    State(state): State<AppState>,
    JsonExtractor(payload): JsonExtractor<MessagesRequest>,
) -> Response {
    // 从 metadata.user_id 中提取会话 ID
    let session_id = payload
        .metadata
        .as_ref()
        .and_then(|m| m.user_id.as_ref())
        .and_then(|uid| uid.split("__session_").nth(1))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "default".to_string());

    tracing::info!(
        model = %payload.model,
        max_tokens = %payload.max_tokens,
        stream = %payload.stream,
        message_count = %payload.messages.len(),
        session_id = %session_id,
        "Received POST /cc/v1/messages request"
    );

    // 检查 KiroProvider 是否可用
    let provider = match &state.kiro_provider {
        Some(p) => p.clone(),
        None => {
            tracing::error!("KiroProvider 未配置");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "service_unavailable",
                    "Kiro API provider not configured",
                )),
            )
                .into_response();
        }
    };

    // 检查是否为 WebSearch 请求
    if websearch::has_web_search_tool(&payload) {
        tracing::info!("检测到 WebSearch 工具，路由到 WebSearch 处理");

        // 估算输入 tokens
        let input_tokens = token::count_all_tokens(
            payload.model.clone(),
            payload.system.clone(),
            payload.messages.clone(),
            payload.tools.clone(),
        ) as i32;

        return websearch::handle_websearch_request(state, provider, &payload, input_tokens, &session_id).await;
    }

    // 转换请求
    let conversion_result = match convert_request(&payload) {
        Ok(result) => result,
        Err(e) => {
            let (error_type, message) = match &e {
                ConversionError::UnsupportedModel(model) => {
                    ("invalid_request_error", format!("模型不支持: {}", model))
                }
                ConversionError::EmptyMessages => {
                    ("invalid_request_error", "消息列表为空".to_string())
                }
                ConversionError::InvalidRequest(msg) => {
                    ("invalid_request_error", msg.clone())
                }
            };
            tracing::warn!("请求转换失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(error_type, message)),
            )
                .into_response();
        }
    };

    // 构建 Kiro 请求
    let kiro_request = KiroRequest {
        conversation_state: conversion_result.conversation_state.clone(),
        profile_arn: state.profile_arn.clone(),
    };

    let request_body = match serde_json::to_string(&kiro_request) {
        Ok(body) => body,
        Err(e) => {
            tracing::error!("序列化请求失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::new(
                    "internal_error",
                    format!("序列化请求失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    tracing::debug!("Kiro request body: {}", request_body);

    // 估算输入 tokens
    let input_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system,
        payload.messages,
        payload.tools,
    ) as i32;

    // 检查是否启用了thinking
    let thinking_enabled = payload
        .thinking
        .as_ref()
        .map(|t| t.thinking_type == "enabled")
        .unwrap_or(false);

    if payload.stream {
        // 流式响应（缓冲模式）
        handle_stream_request_buffered(
            state,
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            thinking_enabled,
            &session_id,
        )
        .await
    } else {
        // 非流式响应
        handle_non_stream_request(
            state,
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            &session_id,
            conversion_result,
        )
        .await
    }
}

/// 处理流式请求（缓冲版本）
///
/// 与 `handle_stream_request` 不同，此函数会缓冲所有事件直到流结束，
/// 然后用从 contextUsageEvent 计算的正确 input_tokens 生成 message_start 事件。
async fn handle_stream_request_buffered(
    state: AppState,
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    request_body: &str,
    model: &str,
    estimated_input_tokens: i32,
    thinking_enabled: bool,
    session_id: &str,
) -> Response {
    // 调用 Kiro API（支持多凭据故障转移）
    let api_result = provider
        .call_api_stream_with_credential_id(request_body, Some(model))
        .await;

    let (credential_id, response) = match api_result {
        Ok((cid, resp)) => (cid, resp),
        Err(e) => {
            tracing::error!("Kiro API 调用失败: {}", e);
            // 记录错误统计（使用 credential_id = 0 表示未知凭据）
            if let Some(stats) = provider.stats_store() {
                stats.record_error(0, Some(model), format!("API 调用失败: {}", e));
            }
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse::new(
                    "api_error",
                    format!("上游 API 调用失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    let stats = provider.stats_store();

    // 创建缓冲流处理上下文
    let ctx = BufferedStreamContext::new(model, estimated_input_tokens, thinking_enabled);

    // 创建缓冲 SSE 流（带统计支持）
    let stream = create_buffered_sse_stream_with_stats(
        response,
        ctx,
        stats,
        credential_id,
        model.to_string(),
        state,
        session_id.to_string(),
    );

    // 返回 SSE 响应
    match Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
    {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!("构建 SSE 响应失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::new(
                    "internal_error",
                    format!("构建 SSE 响应失败: {}", e),
                )),
            )
                .into_response()
        }
    }
}

/// 缓冲流处理状态（带统计支持）
struct BufferedStreamState {
    body_stream: futures::stream::BoxStream<'static, Result<bytes::Bytes, reqwest::Error>>,
    ctx: BufferedStreamContext,
    decoder: EventStreamDecoder,
    finished: bool,
    ping_interval: tokio::time::Interval,
    stats: Option<Arc<StatsStore>>,
    credential_id: u64,
    model: String,
    /// 是否已记录统计（避免重复记录）
    stats_recorded: bool,
}

/// 创建缓冲 SSE 事件流（带统计支持）
///
/// 工作流程：
/// 1. 等待上游流完成，期间只发送 ping 保活信号
/// 2. 使用 StreamContext 的事件处理逻辑处理所有 Kiro 事件，结果缓存
/// 3. 流结束后，用正确的 input_tokens 更正 message_start 事件
/// 4. 记录统计信息（成功/失败、用量）
/// 5. 一次性发送所有事件
fn create_buffered_sse_stream_with_stats(
    response: reqwest::Response,
    ctx: BufferedStreamContext,
    stats: Option<Arc<StatsStore>>,
    credential_id: u64,
    model: String,
    _app_state: AppState,
    _session_id: String,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let body_stream = response.bytes_stream();

    let state = BufferedStreamState {
        body_stream: Box::pin(body_stream),
        ctx,
        decoder: EventStreamDecoder::new(),
        finished: false,
        ping_interval: interval(Duration::from_secs(PING_INTERVAL_SECS)),
        stats,
        credential_id,
        model,
        stats_recorded: false,
    };

    stream::unfold(state, |mut state| async move {
        if state.finished {
            return None;
        }

        loop {
            tokio::select! {
                // 使用 biased 模式，优先检查 ping 定时器
                // 避免在上游 chunk 密集时 ping 被"饿死"
                biased;

                // 优先检查 ping 保活（等待期间唯一发送的数据）
                _ = state.ping_interval.tick() => {
                    tracing::trace!("发送 ping 保活事件（缓冲模式）");
                    let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                    return Some((stream::iter(bytes), state));
                }

                // 然后处理数据流
                chunk_result = state.body_stream.next() => {
                    match chunk_result {
                        Some(Ok(chunk)) => {
                            // 解码事件
                            if let Err(e) = state.decoder.feed(&chunk) {
                                tracing::warn!("缓冲区溢出: {}", e);
                            }

                            for result in state.decoder.decode_iter() {
                                match result {
                                    Ok(frame) => {
                                        if let Ok(event) = Event::from_frame(frame) {
                                            // 缓冲事件（复用 StreamContext 的处理逻辑）
                                            state.ctx.process_and_buffer(&event);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("解码事件失败: {}", e);
                                    }
                                }
                            }
                            // 继续读取下一个 chunk，不发送任何数据
                        }
                        Some(Err(e)) => {
                            tracing::error!("读取响应流失败（缓冲模式）: {}", e);
                            
                            // 记录错误统计
                            if !state.stats_recorded {
                                if let Some(s) = &state.stats {
                                    s.record_error(
                                        state.credential_id,
                                        Some(&state.model),
                                        format!("读取响应流失败: {}", e),
                                    );
                                }
                                state.stats_recorded = true;
                            }

                            // 发生错误，完成处理并返回所有事件
                            let (all_events, input_tokens, output_tokens) = state.ctx.finish_and_get_all_events_with_tokens();
                            
                            // 记录用量（即使出错也记录已处理的部分）
                            if let Some(s) = &state.stats {
                                s.add_usage(
                                    state.credential_id,
                                    Some(&state.model),
                                    input_tokens as i64,
                                    output_tokens as i64,
                                );
                            }

                            state.finished = true;
                            let bytes: Vec<Result<Bytes, Infallible>> = all_events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();
                            return Some((stream::iter(bytes), state));
                        }
                        None => {
                            // 流结束，完成处理并返回所有事件（已更正 input_tokens）
                            let (all_events, input_tokens, output_tokens) = state.ctx.finish_and_get_all_events_with_tokens();
                            
                            // 记录成功统计和用量
                            if !state.stats_recorded {
                                if let Some(s) = &state.stats {
                                    // 检查是否收到上游错误
                                    if state.ctx.has_upstream_error() {
                                        let error_msg = state.ctx.get_upstream_error_message()
                                            .unwrap_or_else(|| "上游服务错误".to_string());
                                        s.record_error(
                                            state.credential_id,
                                            Some(&state.model),
                                            error_msg,
                                        );
                                    } else {
                                        s.record_success(state.credential_id, Some(&state.model));
                                    }
                                    s.add_usage(
                                        state.credential_id,
                                        Some(&state.model),
                                        input_tokens as i64,
                                        output_tokens as i64,
                                    );
                                }
                                state.stats_recorded = true;
                            }

                            tracing::info!(
                                credential_id = state.credential_id,
                                model = %state.model,
                                input_tokens = input_tokens,
                                output_tokens = output_tokens,
                                "缓冲流处理完成，已记录统计"
                            );

                            state.finished = true;
                            let bytes: Vec<Result<Bytes, Infallible>> = all_events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();
                            return Some((stream::iter(bytes), state));
                        }
                    }
                }
            }
        }
    })
    .flatten()
}
