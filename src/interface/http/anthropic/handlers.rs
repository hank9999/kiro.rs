//! Anthropic API Handler 函数

use std::convert::Infallible;

use crate::domain::error::{KiroError, ProviderError};
use crate::domain::event::Event;
use crate::domain::request::kiro::KiroRequest;
use crate::infra::parser::decoder::EventStreamDecoder;
use crate::service::conversation::tokens as token;
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

use super::dto::{
    CountTokensRequest, CountTokensResponse, ErrorResponse, MessagesRequest, ModelsResponse,
    OutputConfig, Thinking,
};
use super::middleware::AppState;
use super::models::supported_models;
use crate::interface::http::error::kiro_error_response;
use crate::service::conversation::converter::{ConversionError, convert_request, map_model};
use crate::service::conversation::delivery::DeliveryMode;
use crate::service::conversation::error::{FatalKiroError, is_fatal_exception};
use crate::service::conversation::reducer::SseEvent;
use crate::service::conversation::stream::{BufferedStreamContext, StreamContext};
use crate::service::conversation::websearch;

/// 将 ProviderError 包装为 KiroError 后委托给统一的错误响应映射。
fn map_provider_error(err: ProviderError) -> Response {
    let kiro: KiroError = err.into();
    kiro_error_response(&kiro)
}

/// GET /v1/models
///
/// 返回可用的模型列表
pub async fn get_models() -> impl IntoResponse {
    tracing::info!("Received GET /v1/models request");
    Json(ModelsResponse {
        object: "list".to_string(),
        data: supported_models(),
    })
}

/// POST /v1/messages
///
/// 创建消息（对话）。流式分支走 [`DeliveryMode::Live`]：每收到上游 chunk 立即推 SSE。
pub async fn post_messages(
    state: State<AppState>,
    payload: JsonExtractor<MessagesRequest>,
) -> Response {
    post_messages_impl(state, payload, DeliveryMode::Live).await
}

/// POST /cc/v1/messages
///
/// Claude Code 兼容端点。流式分支走 [`DeliveryMode::Buffered`]：等流结束后再发 message_start，
/// 用 contextUsageEvent 计算的 input_tokens 替代估算值。
pub async fn post_messages_cc(
    state: State<AppState>,
    payload: JsonExtractor<MessagesRequest>,
) -> Response {
    post_messages_impl(state, payload, DeliveryMode::Buffered).await
}

/// 共用的 `/v1/messages` 与 `/cc/v1/messages` 实现：
/// 仅在流式分支按 [`DeliveryMode`] 选择 SSE 推送策略，其它逻辑（鉴权前置、
/// websearch 短路、请求转换、token 估算、非流式路径）完全相同。
async fn post_messages_impl(
    State(state): State<AppState>,
    JsonExtractor(mut payload): JsonExtractor<MessagesRequest>,
    mode: DeliveryMode,
) -> Response {
    let endpoint = match mode {
        DeliveryMode::Live => "/v1/messages",
        DeliveryMode::Buffered => "/cc/v1/messages",
    };
    tracing::info!(
        model = %payload.model,
        max_tokens = %payload.max_tokens,
        stream = %payload.stream,
        message_count = %payload.messages.len(),
        endpoint = endpoint,
        "Received POST {} request",
        endpoint
    );

    // 所有请求路径（包括 WebSearch）必须先经过同一模型校验。
    if map_model(&payload.model).is_none() {
        tracing::warn!(model = %payload.model, "请求使用了不支持的模型");
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::new(
                "invalid_request_error",
                format!("模型不支持: {}", payload.model),
            )),
        )
            .into_response();
    }

    // 检查 KiroProvider 是否可用
    let provider = match &state.kiro_client {
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

    // 检测模型名是否包含 "thinking" 后缀，若包含则覆写 thinking 配置
    override_thinking_from_model_name(&mut payload);

    // 检查是否为 WebSearch 请求
    if websearch::has_web_search_tool(&payload) {
        tracing::info!("检测到 WebSearch 工具，路由到 WebSearch 处理");

        // 估算输入 tokens
        let input_tokens = token::count_all_tokens(
            payload.system.clone(),
            payload.messages.clone(),
            payload.tools.clone(),
        )
        .await as i32;

        return websearch::handle_websearch_request(provider, &payload, input_tokens).await;
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
            };
            tracing::warn!("请求转换失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(error_type, message)),
            )
                .into_response();
        }
    };

    // 构建 Kiro 请求（profile_arn 由 provider 层根据实际凭据注入）
    let kiro_request = KiroRequest {
        conversation_state: conversion_result.conversation_state,
        profile_arn: None,
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
    let input_tokens =
        token::count_all_tokens(payload.system, payload.messages, payload.tools).await as i32;

    // 检查是否启用了thinking
    let thinking_enabled = payload
        .thinking
        .as_ref()
        .map(|t| t.is_enabled())
        .unwrap_or(false);

    let tool_name_map = conversion_result.tool_name_map;

    if payload.stream {
        // 流式响应：根据 mode 选择 Live / Buffered 推送
        handle_stream_request_unified(
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            thinking_enabled,
            tool_name_map,
            mode,
        )
        .await
    } else {
        // 非流式响应：仅在配置开启时提取 thinking 块
        let extract_thinking = state.extract_thinking && thinking_enabled;
        handle_non_stream_request(
            provider,
            &request_body,
            &payload.model,
            input_tokens,
            extract_thinking,
            tool_name_map,
        )
        .await
    }
}

/// 处理流式请求（Live / Buffered 二合一）
///
/// - [`DeliveryMode::Live`]：上游每个 chunk 立即推 SSE，使用 [`StreamContext`]。
/// - [`DeliveryMode::Buffered`]：等流结束后再统一推 SSE，使用 [`BufferedStreamContext`]
///   以便用 contextUsageEvent 修正 message_start 的 input_tokens。
async fn handle_stream_request_unified(
    provider: std::sync::Arc<crate::service::KiroClient>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    mode: DeliveryMode,
) -> Response {
    // 调用 Kiro API（支持多凭据故障转移）
    let response = match provider.call_api(request_body, Some(model)).await {
        Ok(resp) => resp,
        Err(e) => return map_provider_error(e),
    };

    match mode {
        DeliveryMode::Live => {
            let mut ctx = StreamContext::new_with_thinking(
                model,
                input_tokens,
                thinking_enabled,
                tool_name_map,
            );
            let initial_events = ctx.generate_initial_events();
            let stream = create_sse_stream(response, ctx, initial_events);
            build_sse_response(stream)
        }
        DeliveryMode::Buffered => {
            let ctx =
                BufferedStreamContext::new(model, input_tokens, thinking_enabled, tool_name_map);
            let stream = create_buffered_sse_stream(response, ctx);
            build_sse_response(stream)
        }
    }
}

/// 用 SSE 标准头封装一个字节流为 axum `Response`。
fn build_sse_response<S>(stream: S) -> Response
where
    S: Stream<Item = Result<Bytes, Infallible>> + Send + 'static,
{
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

/// Ping 事件间隔（25秒）
const PING_INTERVAL_SECS: u64 = 25;

/// 创建 ping 事件的 SSE 字符串
fn create_ping_sse() -> Bytes {
    Bytes::from("event: ping\ndata: {\"type\": \"ping\"}\n\n")
}

/// 创建 Anthropic 标准 `event: error` SSE 字节，用于 fail-fast 终止流。
///
/// 输出格式与 Anthropic streaming 规范一致：
/// `event: error\ndata: {"type":"error","error":{"type":"<error_type>","message":"<message>"}}\n\n`
fn create_error_sse(error_type: &str, message: &str) -> Bytes {
    let payload = json!({
        "type": "error",
        "error": { "type": error_type, "message": message }
    });
    Bytes::from(format!(
        "event: error\ndata: {}\n\n",
        serde_json::to_string(&payload).expect("serialize SSE error literal cannot fail")
    ))
}

/// 把已收集的合法 SSE 事件转字节，并在末尾追加一条 fatal `event: error`。
///
/// 用于 live 模式 fail-fast：此时之前已 flush 的事件无法撤回，仅追加 error 通知客户端，
/// 不再发 `final_events` / `message_stop`——流的内容完整性已不可信。
fn flush_events_with_error(
    events: Vec<SseEvent>,
    err: &FatalKiroError,
) -> Vec<Result<Bytes, Infallible>> {
    let mut bytes: Vec<Result<Bytes, Infallible>> = events
        .into_iter()
        .map(|e| Ok(Bytes::from(e.to_sse_string())))
        .collect();
    bytes.push(Ok(create_error_sse(
        err.anthropic_error_type(),
        &err.client_message(),
    )));
    bytes
}

/// 将 [`FatalKiroError`] 映射为非流式 HTTP 错误响应（502 + Anthropic ErrorResponse JSON）。
fn fatal_to_response(err: &FatalKiroError) -> Response {
    (
        err.http_status(),
        Json(ErrorResponse::new(
            err.anthropic_error_type(),
            err.client_message(),
        )),
    )
        .into_response()
}

/// 创建 SSE 事件流
fn create_sse_stream(
    response: reqwest::Response,
    ctx: StreamContext,
    initial_events: Vec<SseEvent>,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    // 先发送初始事件
    let initial_stream = stream::iter(
        initial_events
            .into_iter()
            .map(|e| Ok(Bytes::from(e.to_sse_string()))),
    );

    // 然后处理 Kiro 响应流，同时每25秒发送 ping 保活
    let body_stream = response.bytes_stream();

    let processing_stream = stream::unfold(
        (body_stream, ctx, EventStreamDecoder::new(), false, interval(Duration::from_secs(PING_INTERVAL_SECS))),
        |(mut body_stream, mut ctx, mut decoder, finished, mut ping_interval)| async move {
            if finished {
                return None;
            }

            // 使用 select! 同时等待数据和 ping 定时器
            tokio::select! {
                // 处理数据流
                chunk_result = body_stream.next() => {
                    match chunk_result {
                        Some(Ok(chunk)) => {
                            // feed 失败 = 缓冲区溢出，fail-fast
                            if let Err(e) = decoder.feed(&chunk) {
                                let err = FatalKiroError::BufferOverflow(e.to_string());
                                tracing::error!(kind = err.kind(), mode = "live", error = %err, "SSE 流终止");
                                let bytes = flush_events_with_error(Vec::new(), &err);
                                return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval)));
                            }

                            let mut events = Vec::new();
                            let mut fatal: Option<FatalKiroError> = None;
                            for result in decoder.decode_iter() {
                                match result {
                                    Ok(frame) => match Event::from_frame(frame) {
                                        Ok(event) => match ctx.process_kiro_event(&event) {
                                            Ok(sse_events) => events.extend(sse_events),
                                            Err(e) => { fatal = Some(e); break; }
                                        },
                                        Err(e) => {
                                            fatal = Some(FatalKiroError::EventParseFailed(e.to_string()));
                                            break;
                                        }
                                    },
                                    Err(e) => {
                                        fatal = Some(FatalKiroError::DecodeFailed(e.to_string()));
                                        break;
                                    }
                                }
                            }

                            if let Some(err) = fatal {
                                tracing::error!(kind = err.kind(), mode = "live", error = %err, "SSE 流终止");
                                let bytes = flush_events_with_error(events, &err);
                                return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval)));
                            }

                            // 转换为 SSE 字节流
                            let bytes: Vec<Result<Bytes, Infallible>> = events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();

                            Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval)))
                        }
                        Some(Err(e)) => {
                            // 上游字节流读取失败：fail-fast，不再补 final_events 伪装正常结束
                            let err = FatalKiroError::UpstreamBodyRead(e.to_string());
                            tracing::error!(kind = err.kind(), mode = "live", error = %err, "SSE 流终止");
                            let bytes = flush_events_with_error(Vec::new(), &err);
                            Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval)))
                        }
                        None => {
                            // 流正常结束，发送最终事件
                            let final_events = ctx.generate_final_events();
                            let bytes: Vec<Result<Bytes, Infallible>> = final_events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();
                            Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval)))
                        }
                    }
                }
                // 发送 ping 保活
                _ = ping_interval.tick() => {
                    tracing::trace!("发送 ping 保活事件");
                    let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                    Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval)))
                }
            }
        },
    )
    .flatten();

    initial_stream.chain(processing_stream)
}

use crate::service::conversation::converter::get_context_window_size;

/// 处理非流式请求
async fn handle_non_stream_request(
    provider: std::sync::Arc<crate::service::KiroClient>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
) -> Response {
    // 调用 Kiro API（支持多凭据故障转移）
    let response = match provider.call_api(request_body, Some(model)).await {
        Ok(resp) => resp,
        Err(e) => return map_provider_error(e),
    };

    // 读取响应体
    let body_bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            let err = FatalKiroError::UpstreamBodyRead(e.to_string());
            tracing::error!(kind = err.kind(), mode = "non_stream", error = %err, "非流式响应失败");
            return fatal_to_response(&err);
        }
    };

    // 解析事件流
    let mut decoder = EventStreamDecoder::new();
    if let Err(e) = decoder.feed(&body_bytes) {
        let err = FatalKiroError::BufferOverflow(e.to_string());
        tracing::error!(kind = err.kind(), mode = "non_stream", error = %err, "非流式响应失败");
        return fatal_to_response(&err);
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

    for result in decoder.decode_iter() {
        let frame = match result {
            Ok(frame) => frame,
            Err(e) => {
                let err = FatalKiroError::DecodeFailed(e.to_string());
                tracing::error!(kind = err.kind(), mode = "non_stream", error = %err, "非流式响应失败");
                return fatal_to_response(&err);
            }
        };
        let event = match Event::from_frame(frame) {
            Ok(event) => event,
            Err(e) => {
                let err = FatalKiroError::EventParseFailed(e.to_string());
                tracing::error!(kind = err.kind(), mode = "non_stream", error = %err, "非流式响应失败");
                return fatal_to_response(&err);
            }
        };
        match event {
            Event::AssistantResponse(resp) => {
                text_content.push_str(&resp.content);
            }
            Event::ToolUse(tool_use) => {
                has_tool_use = true;

                // 累积工具的 JSON 输入
                let buffer = tool_json_buffers
                    .entry(tool_use.tool_use_id.clone())
                    .or_default();
                buffer.push_str(&tool_use.input);

                // 如果是完整的工具调用，添加到列表
                if tool_use.stop {
                    let input: serde_json::Value = if buffer.is_empty() {
                        serde_json::json!({})
                    } else {
                        serde_json::from_str(buffer).unwrap_or_else(|e| {
                            tracing::warn!(
                                "工具输入 JSON 解析失败: {}, tool_use_id: {}",
                                e,
                                tool_use.tool_use_id
                            );
                            serde_json::json!({})
                        })
                    };

                    let original_name = tool_name_map
                        .get(&tool_use.name)
                        .cloned()
                        .unwrap_or_else(|| tool_use.name.clone());

                    tool_uses.push(json!({
                        "type": "tool_use",
                        "id": tool_use.tool_use_id,
                        "name": original_name,
                        "input": input
                    }));
                }
            }
            Event::ContextUsage(context_usage) => {
                // 从上下文使用百分比计算实际的 input_tokens
                let window_size = get_context_window_size(model);
                let actual_input_tokens =
                    (context_usage.context_usage_percentage * (window_size as f64) / 100.0) as i32;
                context_input_tokens = Some(actual_input_tokens);
                // 上下文使用量达到 100% 时，设置 stop_reason 为 model_context_window_exceeded
                if context_usage.context_usage_percentage >= 100.0 {
                    stop_reason = "model_context_window_exceeded".to_string();
                }
                tracing::debug!(
                    "收到 contextUsageEvent: {}%, 计算 input_tokens: {}",
                    context_usage.context_usage_percentage,
                    actual_input_tokens
                );
            }
            Event::Exception {
                exception_type,
                message,
            } => {
                if is_fatal_exception(&exception_type) {
                    let err = FatalKiroError::UpstreamException {
                        exception_type,
                        message,
                    };
                    tracing::error!(kind = err.kind(), mode = "non_stream", error = %err, "非流式响应失败");
                    return fatal_to_response(&err);
                }
                stop_reason = "max_tokens".to_string();
            }
            Event::Error {
                error_code,
                error_message,
            } => {
                let err = FatalKiroError::UpstreamError {
                    error_code,
                    error_message,
                };
                tracing::error!(kind = err.kind(), mode = "non_stream", error = %err, "非流式响应失败");
                return fatal_to_response(&err);
            }
            _ => {}
        }
    }

    // 确定 stop_reason
    if has_tool_use && stop_reason == "end_turn" {
        stop_reason = "tool_use".to_string();
    }

    // 构建响应内容
    let mut content: Vec<serde_json::Value> = Vec::new();

    if thinking_enabled {
        // 从完整文本中提取 thinking 块
        let (thinking, remaining_text) =
            crate::service::conversation::thinking::extract_thinking_from_complete_text(
                &text_content,
            );

        if let Some(thinking_text) = thinking {
            content.push(json!({
                "type": "thinking",
                "thinking": thinking_text
            }));
        }

        if !remaining_text.is_empty() {
            content.push(json!({
                "type": "text",
                "text": remaining_text
            }));
        }
    } else if !text_content.is_empty() {
        content.push(json!({
            "type": "text",
            "text": text_content
        }));
    }

    content.extend(tool_uses);

    // 估算输出 tokens：与流式 delivery.rs 路径对齐，基于原始 text_content
    // （含 `<thinking>` 标签与内容）+ 所有 tool_use.input 原始片段。
    // 不能基于上方拼装的 content blocks —— thinking block 字段名是 "thinking"
    // 而非 "text"，按 block 统计会漏算 thinking 的 token 消耗。
    let mut tool_input_concat = String::new();
    for buf in tool_json_buffers.values() {
        tool_input_concat.push_str(buf);
        tool_input_concat.push('\n');
    }
    let output_tokens = (token::count_tokens(&text_content)
        + token::count_tokens(&tool_input_concat))
    .max(1) as i32;

    // 使用从 contextUsageEvent 计算的 input_tokens，如果没有则使用估算值
    let final_input_tokens = context_input_tokens.unwrap_or(input_tokens);

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
            "input_tokens": final_input_tokens,
            "output_tokens": output_tokens
        }
    });

    (StatusCode::OK, Json(response_body)).into_response()
}

/// 检测模型名是否包含 "thinking" 后缀，若包含则覆写 thinking 配置
///
/// - Opus 4.6、Sonnet 5、Opus 5：覆写为 adaptive 类型
/// - 其他模型：覆写为 enabled 类型
/// - budget_tokens 固定为 20000
fn override_thinking_from_model_name(payload: &mut MessagesRequest) {
    let model_lower = payload.model.to_lowercase();
    if !model_lower.contains("thinking") {
        return;
    }

    let is_adaptive_thinking = matches!(
        map_model(&payload.model).as_deref(),
        Some("claude-opus-4.6" | "claude-sonnet-5" | "claude-opus-5")
    );

    let thinking_type = if is_adaptive_thinking {
        "adaptive"
    } else {
        "enabled"
    };

    tracing::info!(
        model = %payload.model,
        thinking_type = thinking_type,
        "模型名包含 thinking 后缀，覆写 thinking 配置"
    );

    payload.thinking = Some(Thinking {
        thinking_type: thinking_type.to_string(),
        budget_tokens: 20000,
    });

    if is_adaptive_thinking {
        payload.output_config = Some(OutputConfig {
            effort: "high".to_string(),
        });
    }
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

    let total_tokens =
        token::count_all_tokens(payload.system, payload.messages, payload.tools).await as i32;

    Json(CountTokensResponse {
        input_tokens: total_tokens.max(1) as i32,
    })
}

/// 创建缓冲 SSE 事件流
///
/// 工作流程：
/// 1. 等待上游流完成，期间只发送 ping 保活信号
/// 2. 使用 StreamContext 的事件处理逻辑处理所有 Kiro 事件，结果缓存
/// 3. 流结束后，用正确的 input_tokens 更正 message_start 事件
/// 4. 一次性发送所有事件
fn create_buffered_sse_stream(
    response: reqwest::Response,
    ctx: BufferedStreamContext,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let body_stream = response.bytes_stream();

    stream::unfold(
        (
            body_stream,
            ctx,
            EventStreamDecoder::new(),
            false,
            interval(Duration::from_secs(PING_INTERVAL_SECS)),
        ),
        |(mut body_stream, mut ctx, mut decoder, finished, mut ping_interval)| async move {
            if finished {
                return None;
            }

            loop {
                tokio::select! {
                    // 使用 biased 模式，优先检查 ping 定时器
                    // 避免在上游 chunk 密集时 ping 被"饿死"
                    biased;

                    // 优先检查 ping 保活（等待期间唯一发送的数据）
                    _ = ping_interval.tick() => {
                        tracing::trace!("发送 ping 保活事件（缓冲模式）");
                        let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                        return Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval)));
                    }

                    // 然后处理数据流
                    chunk_result = body_stream.next() => {
                        match chunk_result {
                            Some(Ok(chunk)) => {
                                // feed 失败 = 缓冲区溢出，fail-fast
                                if let Err(e) = decoder.feed(&chunk) {
                                    let err = FatalKiroError::BufferOverflow(e.to_string());
                                    tracing::error!(kind = err.kind(), mode = "buffered", error = %err, "SSE 流终止");
                                    let bytes = vec![Ok(create_error_sse(
                                        err.anthropic_error_type(),
                                        &err.client_message(),
                                    ))];
                                    return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval)));
                                }

                                let mut fatal: Option<FatalKiroError> = None;
                                for result in decoder.decode_iter() {
                                    match result {
                                        Ok(frame) => match Event::from_frame(frame) {
                                            Ok(event) => {
                                                if let Err(e) = ctx.process_and_buffer(&event) {
                                                    fatal = Some(e);
                                                    break;
                                                }
                                            }
                                            Err(e) => {
                                                fatal = Some(FatalKiroError::EventParseFailed(e.to_string()));
                                                break;
                                            }
                                        },
                                        Err(e) => {
                                            fatal = Some(FatalKiroError::DecodeFailed(e.to_string()));
                                            break;
                                        }
                                    }
                                }

                                if let Some(err) = fatal {
                                    // buffered 模式下尚未 flush 任何事件给客户端，直接丢弃 buffer，
                                    // 只发一条 event: error，避免给客户端"开了头但没收尾"的不完整结构。
                                    tracing::error!(kind = err.kind(), mode = "buffered", error = %err, "SSE 流终止");
                                    let bytes = vec![Ok(create_error_sse(
                                        err.anthropic_error_type(),
                                        &err.client_message(),
                                    ))];
                                    return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval)));
                                }
                                // 继续读取下一个 chunk，不发送任何数据
                            }
                            Some(Err(e)) => {
                                let err = FatalKiroError::UpstreamBodyRead(e.to_string());
                                tracing::error!(kind = err.kind(), mode = "buffered", error = %err, "SSE 流终止");
                                let bytes = vec![Ok(create_error_sse(
                                    err.anthropic_error_type(),
                                    &err.client_message(),
                                ))];
                                return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval)));
                            }
                            None => {
                                // 流结束，完成处理并返回所有事件（已更正 input_tokens）
                                let all_events = ctx.finish_and_get_all_events();
                                let bytes: Vec<Result<Bytes, Infallible>> = all_events
                                    .into_iter()
                                    .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                    .collect();
                                return Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval)));
                            }
                        }
                    }
                }
            }
        },
    )
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    async fn body_json(resp: Response) -> (StatusCode, Value) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        (status, json)
    }

    #[tokio::test]
    async fn map_provider_error_returns_400_for_context_window_full() {
        let resp = map_provider_error(ProviderError::ContextWindowFull);
        let (status, json) = body_json(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["type"], "invalid_request_error");
    }

    #[tokio::test]
    async fn map_provider_error_returns_400_for_input_too_long() {
        let resp = map_provider_error(ProviderError::InputTooLong);
        let (status, json) = body_json(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["type"], "invalid_request_error");
    }

    #[tokio::test]
    async fn map_provider_error_returns_503_for_endpoint_resolution() {
        let resp = map_provider_error(ProviderError::EndpointResolution(
            "ide endpoint missing".into(),
        ));
        let (status, json) = body_json(resp).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json["error"]["type"], "api_error");
    }

    #[tokio::test]
    async fn map_provider_error_returns_502_for_upstream_http() {
        let resp = map_provider_error(ProviderError::UpstreamHttp {
            status: 503,
            body: "upstream is down".into(),
        });
        let (status, json) = body_json(resp).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(json["error"]["type"], "api_error");
    }

    fn empty_request() -> MessagesRequest {
        MessagesRequest {
            model: "claude-opus-4-6".into(),
            max_tokens: 1024,
            messages: vec![],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    #[test]
    fn thinking_suffix_uses_adaptive_mode_for_supported_models() {
        for model in [
            "claude-opus-4-6-thinking",
            "claude-sonnet-5-thinking",
            "claude-opus-5-thinking",
        ] {
            let mut request = empty_request();
            request.model = model.to_string();

            override_thinking_from_model_name(&mut request);

            let thinking = request.thinking.as_ref().expect("thinking must be set");
            assert_eq!(thinking.thinking_type, "adaptive", "model={model}");
            assert_eq!(thinking.budget_tokens, 20_000, "model={model}");
            assert_eq!(
                request
                    .output_config
                    .as_ref()
                    .map(|config| config.effort.as_str()),
                Some("high"),
                "model={model}"
            );
        }
    }

    #[test]
    fn thinking_suffix_does_not_enable_adaptive_for_unsupported_versions() {
        for model in [
            "claude-sonnet-50-thinking",
            "claude-sonnet-5-1-thinking",
            "claude-opus-50-thinking",
            "claude-opus-4-6-future-thinking",
        ] {
            let mut request = empty_request();
            request.model = model.to_string();

            override_thinking_from_model_name(&mut request);

            let thinking = request.thinking.as_ref().expect("thinking must be set");
            assert_eq!(thinking.thinking_type, "enabled", "model={model}");
            assert!(request.output_config.is_none(), "model={model}");
        }
    }

    #[tokio::test]
    async fn post_messages_rejects_unsupported_websearch_model() {
        use super::super::dto::Tool;

        let state = AppState::new("test-key", false);
        let mut request = empty_request();
        request.model = "claude-sonnet-5-1-thinking".to_string();
        request.tools = Some(vec![Tool {
            tool_type: Some("web_search_20250305".to_string()),
            name: "web_search".to_string(),
            description: String::new(),
            input_schema: Default::default(),
            max_uses: Some(8),
        }]);

        let resp = post_messages(State(state), JsonExtractor(request)).await;
        let (status, json) = body_json(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["type"], "invalid_request_error");
    }

    /// 当 KiroClient 未配置时 `/v1/messages` 必须返回 503，且 body 携带
    /// `service_unavailable` 错误类型。
    #[tokio::test]
    async fn post_messages_returns_503_when_kiro_client_missing() {
        let state = AppState::new("test-key", false);
        let resp = post_messages(State(state), JsonExtractor(empty_request())).await;
        let (status, json) = body_json(resp).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json["error"]["type"], "service_unavailable");
    }

    /// `/cc/v1/messages` 必须复用 `/v1/messages` 的 503 错误路径——证明
    /// 双 handler 已收敛到同一 `post_messages_impl`，没有出现行为漂移。
    #[tokio::test]
    async fn post_messages_cc_returns_503_when_kiro_client_missing() {
        let state = AppState::new("test-key", false);
        let resp = post_messages_cc(State(state), JsonExtractor(empty_request())).await;
        let (status, json) = body_json(resp).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json["error"]["type"], "service_unavailable");
    }

    /// `/v1/messages` 与 `/cc/v1/messages` 共享 503 响应：状态码与错误 body
    /// 完全一致——双 handler 合并未引入分歧。
    #[tokio::test]
    async fn post_messages_and_cc_share_503_body_when_provider_missing() {
        let state_a = AppState::new("test-key", false);
        let state_b = AppState::new("test-key", false);
        let (status_a, body_a) =
            body_json(post_messages(State(state_a), JsonExtractor(empty_request())).await).await;
        let (status_b, body_b) =
            body_json(post_messages_cc(State(state_b), JsonExtractor(empty_request())).await).await;
        assert_eq!(status_a, status_b);
        assert_eq!(body_a, body_b);
    }

    /// 防护测试：DeliveryMode 必须保持 Live ≠ Buffered；当未来 PartialEq
    /// 被去掉或两个 mode 被误并时此测试失败，提示同步更新 dispatch。
    #[test]
    fn delivery_mode_dispatch_is_distinct() {
        assert_ne!(DeliveryMode::Live, DeliveryMode::Buffered);
    }

    fn bytes_to_strings(b: Vec<Result<Bytes, Infallible>>) -> Vec<String> {
        b.into_iter()
            .map(|r| String::from_utf8(r.unwrap().to_vec()).unwrap())
            .collect()
    }

    /// Live fail-fast：已收集的合法事件按原顺序出现，末尾追加一条 event: error，
    /// 且**绝不**出现 message_stop——流的内容完整性已不可信，不能伪装正常结束。
    #[test]
    fn flush_events_with_error_appends_error_and_never_emits_message_stop() {
        let events = vec![
            SseEvent::new(
                "message_start",
                json!({"type": "message_start", "message": {"id": "msg_x"}}),
            ),
            SseEvent::new(
                "content_block_delta",
                json!({"type": "content_block_delta", "index": 0,
                    "delta": {"type": "text_delta", "text": "hi"}}),
            ),
        ];
        let err = FatalKiroError::UpstreamError {
            error_code: "RateLimited".into(),
            error_message: "too many requests".into(),
        };

        let lines = bytes_to_strings(flush_events_with_error(events, &err));
        let combined = lines.concat();

        assert!(combined.contains("event: message_start"));
        assert!(combined.contains("event: content_block_delta"));
        assert!(combined.contains("event: error"));
        assert!(
            !combined.contains("event: message_stop"),
            "fatal 终止时不得再发 message_stop: {combined}"
        );
        // 末尾必须是 error 事件
        let last = lines.last().unwrap();
        assert!(
            last.starts_with("event: error"),
            "最后一条必须是 error: {last}"
        );
        // 上游 Error 的语义透传
        assert!(combined.contains("RateLimited"));
        assert!(combined.contains("too many requests"));
    }

    /// 本地解码错误的 client_message 应使用泛化文案，不泄漏内部细节。
    #[test]
    fn flush_events_with_error_redacts_local_decode_internals() {
        let err = FatalKiroError::BufferOverflow("size 16777217 > max 16777216".into());
        let lines = bytes_to_strings(flush_events_with_error(Vec::new(), &err));
        let combined = lines.concat();
        assert!(combined.contains("event: error"));
        assert!(
            !combined.contains("16777216"),
            "内部细节不得暴露给客户端: {combined}"
        );
    }

    /// `create_error_sse` 输出符合 Anthropic SSE 规范的 error 事件结构。
    #[test]
    fn create_error_sse_emits_anthropic_compliant_payload() {
        let raw = create_error_sse("api_error", "boom");
        let s = String::from_utf8(raw.to_vec()).unwrap();
        assert!(s.starts_with("event: error\ndata: "));
        assert!(s.ends_with("\n\n"));
        let data_line = s
            .lines()
            .find_map(|l| l.strip_prefix("data: "))
            .expect("must have data line");
        let v: Value = serde_json::from_str(data_line).unwrap();
        assert_eq!(v["type"], "error");
        assert_eq!(v["error"]["type"], "api_error");
        assert_eq!(v["error"]["message"], "boom");
    }

    /// 非流式 fatal 路径：返回 502 + Anthropic ErrorResponse JSON，**不返 200**。
    #[tokio::test]
    async fn fatal_to_response_returns_502_with_anthropic_error_json() {
        let err = FatalKiroError::DecodeFailed("crc mismatch at offset 42".into());
        let (status, json) = body_json(fatal_to_response(&err)).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(json["error"]["type"], "api_error");
        // 本地解码错误用泛化文案
        let msg = json["error"]["message"].as_str().unwrap();
        assert!(msg.contains("Stream decode failed"), "got: {msg}");
        assert!(!msg.contains("offset 42"), "internal detail leaked: {msg}");
    }
}
