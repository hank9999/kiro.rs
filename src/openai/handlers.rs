//! OpenAI Chat Completions Handler

use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};

use anyhow::Error;
use axum::{
    Json as JsonExtractor,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use chrono::Utc;
use futures::{Stream, StreamExt, stream};
use serde_json::json;
use tokio::time::interval;
use uuid::Uuid;

use crate::{
    anthropic::{
        converter::{ConversionError as AnthropicConversionError, convert_request},
        middleware::AppState,
        stream::{SseEvent, StreamContext},
        types::{MessagesRequest, OutputConfig, Thinking},
    },
    kiro::{
        model::{events::Event, requests::kiro::KiroRequest},
        parser::decoder::EventStreamDecoder,
        provider::KiroProvider,
    },
    token,
};

use super::{
    converter::{convert_chat_request, convert_responses_request},
    types::{
        AssistantMessage, ChatCompletionChunk, ChatCompletionResponse, ChatCompletionsRequest,
        Choice, ChunkChoice, DeltaMessage, DeltaToolCall, DeltaToolCallFunction, ErrorResponse,
        ResponsesRequest, ToolCall, ToolCallFunction, Usage,
    },
};

/// POST /v1/chat/completions
pub async fn post_chat_completions(
    State(state): State<AppState>,
    JsonExtractor(payload): JsonExtractor<ChatCompletionsRequest>,
) -> Response {
    tracing::info!(
        model = %payload.model,
        stream = %payload.stream,
        message_count = %payload.messages.len(),
        "Received POST /v1/chat/completions request"
    );

    let provider = match &state.kiro_provider {
        Some(provider) => provider.clone(),
        None => {
            return openai_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable_error",
                "Kiro API provider not configured",
            );
        }
    };

    let include_usage = payload
        .stream_options
        .as_ref()
        .map(|opts| opts.include_usage)
        .unwrap_or(false);

    let mut anthropic_payload = match convert_chat_request(&payload) {
        Ok(req) => req,
        Err(err) => {
            return openai_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                err.to_string(),
            );
        }
    };

    apply_thinking_defaults_from_model_name(&mut anthropic_payload);

    let conversion_result = match convert_request(&anthropic_payload) {
        Ok(result) => result,
        Err(err) => {
            let message = match err {
                AnthropicConversionError::UnsupportedModel(model) => {
                    format!("模型不支持: {}", model)
                }
                AnthropicConversionError::EmptyMessages => "消息列表为空".to_string(),
            };
            return openai_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                message,
            );
        }
    };

    let kiro_request = KiroRequest {
        conversation_state: conversion_result.conversation_state,
        profile_arn: state.profile_arn.clone(),
    };

    let request_body = match serde_json::to_string(&kiro_request) {
        Ok(body) => body,
        Err(e) => {
            return openai_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("序列化请求失败: {}", e),
            );
        }
    };

    let input_tokens = token::count_all_tokens(
        anthropic_payload.model.clone(),
        anthropic_payload.system.clone(),
        anthropic_payload.messages.clone(),
        anthropic_payload.tools.clone(),
    ) as i32;

    let thinking_enabled = anthropic_payload
        .thinking
        .as_ref()
        .map(|thinking| thinking.is_enabled())
        .unwrap_or(false);

    if payload.stream {
        handle_stream_request(
            provider,
            &request_body,
            &anthropic_payload.model,
            input_tokens,
            thinking_enabled,
            conversion_result.tool_name_map,
            include_usage,
        )
        .await
    } else {
        handle_non_stream_request(
            provider,
            &request_body,
            &anthropic_payload.model,
            input_tokens,
            thinking_enabled,
            conversion_result.tool_name_map,
        )
        .await
    }
}

/// POST /v1/responses
pub async fn post_responses(
    State(state): State<AppState>,
    JsonExtractor(payload): JsonExtractor<ResponsesRequest>,
) -> Response {
    tracing::info!(
        model = %payload.model,
        stream = %payload.stream,
        "Received POST /v1/responses request"
    );

    let provider = match &state.kiro_provider {
        Some(provider) => provider.clone(),
        None => {
            return openai_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable_error",
                "Kiro API provider not configured",
            );
        }
    };

    let include_usage = payload
        .stream_options
        .as_ref()
        .map(|opts| opts.include_usage)
        .unwrap_or(false);

    let mut anthropic_payload = match convert_responses_request(&payload) {
        Ok(req) => req,
        Err(err) => {
            return openai_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                err.to_string(),
            );
        }
    };

    apply_thinking_defaults_from_model_name(&mut anthropic_payload);

    let conversion_result = match convert_request(&anthropic_payload) {
        Ok(result) => result,
        Err(err) => {
            let message = match err {
                AnthropicConversionError::UnsupportedModel(model) => {
                    format!("模型不支持: {}", model)
                }
                AnthropicConversionError::EmptyMessages => "消息列表为空".to_string(),
            };
            return openai_error_response(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                message,
            );
        }
    };

    let kiro_request = KiroRequest {
        conversation_state: conversion_result.conversation_state,
        profile_arn: state.profile_arn.clone(),
    };

    let request_body = match serde_json::to_string(&kiro_request) {
        Ok(body) => body,
        Err(e) => {
            return openai_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("序列化请求失败: {}", e),
            );
        }
    };

    let input_tokens = token::count_all_tokens(
        anthropic_payload.model.clone(),
        anthropic_payload.system.clone(),
        anthropic_payload.messages.clone(),
        anthropic_payload.tools.clone(),
    ) as i32;

    let thinking_enabled = anthropic_payload
        .thinking
        .as_ref()
        .map(|thinking| thinking.is_enabled())
        .unwrap_or(false);

    if payload.stream {
        handle_responses_stream_request(
            provider,
            &request_body,
            &anthropic_payload.model,
            input_tokens,
            thinking_enabled,
            conversion_result.tool_name_map,
            include_usage,
        )
        .await
    } else {
        handle_responses_non_stream_request(
            provider,
            &request_body,
            &anthropic_payload.model,
            input_tokens,
            thinking_enabled,
            conversion_result.tool_name_map,
        )
        .await
    }
}

fn openai_error_response(
    status: StatusCode,
    error_type: impl Into<String>,
    message: impl Into<String>,
) -> Response {
    (status, Json(ErrorResponse::new(error_type, message))).into_response()
}

fn map_provider_error(err: Error) -> Response {
    let err_str = err.to_string();

    if err_str.contains("CONTENT_LENGTH_EXCEEDS_THRESHOLD") {
        return openai_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "Context window is full. Reduce conversation history, system prompt, or tools.",
        );
    }

    if err_str.contains("Input is too long") {
        return openai_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "Input is too long. Reduce the size of your messages.",
        );
    }

    tracing::error!("Kiro API 调用失败: {}", err);
    openai_error_response(
        StatusCode::BAD_GATEWAY,
        "api_error",
        format!("上游 API 调用失败: {}", err),
    )
}

async fn handle_non_stream_request(
    provider: Arc<KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: HashMap<String, String>,
) -> Response {
    let response = match provider.call_api(request_body).await {
        Ok(resp) => resp,
        Err(e) => return map_provider_error(e),
    };

    let body_bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            return openai_error_response(
                StatusCode::BAD_GATEWAY,
                "api_error",
                format!("读取响应失败: {}", e),
            );
        }
    };

    let mut ctx =
        StreamContext::new_with_thinking(model, input_tokens, thinking_enabled, tool_name_map);
    let mut state = OpenAiResponseState::new(model, thinking_enabled, false);

    let _ = state.consume_events(ctx.generate_initial_events());

    let mut decoder = EventStreamDecoder::new();
    if let Err(e) = decoder.feed(&body_bytes) {
        tracing::warn!("缓冲区溢出: {}", e);
    }

    for result in decoder.decode_iter() {
        match result {
            Ok(frame) => match Event::from_frame(frame) {
                Ok(event) => {
                    let sse_events = ctx.process_kiro_event(&event);
                    let _ = state.consume_events(sse_events);
                }
                Err(e) => tracing::warn!("解码事件失败: {}", e),
            },
            Err(e) => tracing::warn!("解码事件失败: {}", e),
        }
    }

    let _ = state.consume_events(ctx.generate_final_events());

    (StatusCode::OK, Json(state.build_response())).into_response()
}

async fn handle_stream_request(
    provider: Arc<KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: HashMap<String, String>,
    include_usage: bool,
) -> Response {
    let response = match provider.call_api_stream(request_body).await {
        Ok(resp) => resp,
        Err(e) => return map_provider_error(e),
    };

    let mut ctx =
        StreamContext::new_with_thinking(model, input_tokens, thinking_enabled, tool_name_map);
    let mut state = OpenAiResponseState::new(model, thinking_enabled, include_usage);
    let initial_bytes = state.consume_events(ctx.generate_initial_events());
    let stream = create_openai_stream(response, ctx, state, initial_bytes);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn handle_responses_non_stream_request(
    provider: Arc<KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: HashMap<String, String>,
) -> Response {
    let response = match provider.call_api(request_body).await {
        Ok(resp) => resp,
        Err(e) => return map_provider_error(e),
    };

    let body_bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            return openai_error_response(
                StatusCode::BAD_GATEWAY,
                "api_error",
                format!("读取响应失败: {}", e),
            );
        }
    };

    let mut ctx =
        StreamContext::new_with_thinking(model, input_tokens, thinking_enabled, tool_name_map);
    let mut state = ResponsesState::new(model, false);

    let _ = state.consume_events(ctx.generate_initial_events());

    let mut decoder = EventStreamDecoder::new();
    if let Err(e) = decoder.feed(&body_bytes) {
        tracing::warn!("缓冲区溢出: {}", e);
    }

    for result in decoder.decode_iter() {
        match result {
            Ok(frame) => match Event::from_frame(frame) {
                Ok(event) => {
                    let sse_events = ctx.process_kiro_event(&event);
                    let _ = state.consume_events(sse_events);
                }
                Err(e) => tracing::warn!("解码事件失败: {}", e),
            },
            Err(e) => tracing::warn!("解码事件失败: {}", e),
        }
    }

    let _ = state.consume_events(ctx.generate_final_events());

    (StatusCode::OK, Json(state.build_response_json())).into_response()
}

async fn handle_responses_stream_request(
    provider: Arc<KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: HashMap<String, String>,
    include_usage: bool,
) -> Response {
    let response = match provider.call_api_stream(request_body).await {
        Ok(resp) => resp,
        Err(e) => return map_provider_error(e),
    };

    let mut ctx =
        StreamContext::new_with_thinking(model, input_tokens, thinking_enabled, tool_name_map);
    let mut state = ResponsesState::new(model, include_usage);
    let initial_bytes = state.consume_events(ctx.generate_initial_events());
    let stream = create_responses_stream(response, ctx, state, initial_bytes);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

const PING_INTERVAL_SECS: u64 = 25;

fn create_openai_stream(
    response: reqwest::Response,
    ctx: StreamContext,
    state: OpenAiResponseState,
    initial_bytes: Vec<Bytes>,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let initial_stream = stream::iter(initial_bytes.into_iter().map(Ok));
    let body_stream = response.bytes_stream();

    let processing_stream = stream::unfold(
        (
            body_stream,
            ctx,
            state,
            EventStreamDecoder::new(),
            false,
            interval(Duration::from_secs(PING_INTERVAL_SECS)),
        ),
        |(mut body_stream, mut ctx, mut state, mut decoder, finished, mut ping_interval)| async move {
            if finished {
                return None;
            }

            tokio::select! {
                chunk_result = body_stream.next() => {
                    match chunk_result {
                        Some(Ok(chunk)) => {
                            if let Err(e) = decoder.feed(&chunk) {
                                tracing::warn!("缓冲区溢出: {}", e);
                            }

                            let mut bytes = Vec::new();
                            for result in decoder.decode_iter() {
                                match result {
                                    Ok(frame) => match Event::from_frame(frame) {
                                        Ok(event) => {
                                            let sse_events = ctx.process_kiro_event(&event);
                                            bytes.extend(state.consume_events(sse_events));
                                        }
                                        Err(e) => tracing::warn!("解码事件失败: {}", e),
                                    },
                                    Err(e) => tracing::warn!("解码事件失败: {}", e),
                                }
                            }

                            let stream = stream::iter(bytes.into_iter().map(Ok));
                            Some((stream, (body_stream, ctx, state, decoder, false, ping_interval)))
                        }
                        Some(Err(e)) => {
                            tracing::error!("读取响应流失败: {}", e);
                            let mut bytes = state.consume_events(ctx.generate_final_events());
                            bytes.push(Bytes::from("data: [DONE]\n\n"));
                            let stream = stream::iter(bytes.into_iter().map(Ok));
                            Some((stream, (body_stream, ctx, state, decoder, true, ping_interval)))
                        }
                        None => {
                            let mut bytes = state.consume_events(ctx.generate_final_events());
                            bytes.push(Bytes::from("data: [DONE]\n\n"));
                            let stream = stream::iter(bytes.into_iter().map(Ok));
                            Some((stream, (body_stream, ctx, state, decoder, true, ping_interval)))
                        }
                    }
                }
                _ = ping_interval.tick() => {
                    let stream = stream::iter(vec![Bytes::from(": ping\n\n")].into_iter().map(Ok));
                    Some((stream, (body_stream, ctx, state, decoder, false, ping_interval)))
                }
            }
        },
    )
    .flatten();

    initial_stream.chain(processing_stream)
}

fn create_responses_stream(
    response: reqwest::Response,
    ctx: StreamContext,
    state: ResponsesState,
    initial_bytes: Vec<Bytes>,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let initial_stream = stream::iter(initial_bytes.into_iter().map(Ok));
    let body_stream = response.bytes_stream();

    let processing_stream = stream::unfold(
        (
            body_stream,
            ctx,
            state,
            EventStreamDecoder::new(),
            false,
            interval(Duration::from_secs(PING_INTERVAL_SECS)),
        ),
        |(mut body_stream, mut ctx, mut state, mut decoder, finished, mut ping_interval)| async move {
            if finished {
                return None;
            }

            tokio::select! {
                chunk_result = body_stream.next() => {
                    match chunk_result {
                        Some(Ok(chunk)) => {
                            if let Err(e) = decoder.feed(&chunk) {
                                tracing::warn!("缓冲区溢出: {}", e);
                            }

                            let mut bytes = Vec::new();
                            for result in decoder.decode_iter() {
                                match result {
                                    Ok(frame) => match Event::from_frame(frame) {
                                        Ok(event) => {
                                            let sse_events = ctx.process_kiro_event(&event);
                                            bytes.extend(state.consume_events(sse_events));
                                        }
                                        Err(e) => tracing::warn!("解码事件失败: {}", e),
                                    },
                                    Err(e) => tracing::warn!("解码事件失败: {}", e),
                                }
                            }

                            let stream = stream::iter(bytes.into_iter().map(Ok));
                            Some((stream, (body_stream, ctx, state, decoder, false, ping_interval)))
                        }
                        Some(Err(e)) => {
                            tracing::error!("读取响应流失败: {}", e);
                            let bytes = state.finalize_stream(ctx.generate_final_events());
                            let stream = stream::iter(bytes.into_iter().map(Ok));
                            Some((stream, (body_stream, ctx, state, decoder, true, ping_interval)))
                        }
                        None => {
                            let bytes = state.finalize_stream(ctx.generate_final_events());
                            let stream = stream::iter(bytes.into_iter().map(Ok));
                            Some((stream, (body_stream, ctx, state, decoder, true, ping_interval)))
                        }
                    }
                }
                _ = ping_interval.tick() => {
                    let stream = stream::iter(vec![Bytes::from(": ping\n\n")].into_iter().map(Ok));
                    Some((stream, (body_stream, ctx, state, decoder, false, ping_interval)))
                }
            }
        },
    )
    .flatten();

    initial_stream.chain(processing_stream)
}

fn apply_thinking_defaults_from_model_name(payload: &mut MessagesRequest) {
    let model_lower = payload.model.to_lowercase();
    if !model_lower.contains("thinking") {
        return;
    }

    let is_opus_4_6 =
        model_lower.contains("opus") && (model_lower.contains("4-6") || model_lower.contains("4.6"));

    let thinking_type = if is_opus_4_6 {
        "adaptive"
    } else {
        "enabled"
    };

    payload.thinking = Some(Thinking {
        thinking_type: thinking_type.to_string(),
        budget_tokens: 20000,
    });

    if is_opus_4_6 {
        payload.output_config = Some(OutputConfig {
            effort: "high".to_string(),
        });
    }
}

#[derive(Debug, Clone)]
struct ResponseFunctionCallItem {
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    output_index: usize,
    done: bool,
}

#[derive(Debug, Clone, Copy)]
enum ResponseOutputRef {
    Message,
    FunctionCall(usize),
}

#[derive(Debug)]
struct ResponsesState {
    response_id: String,
    created_at: i64,
    model: String,
    include_usage: bool,
    message_item_id: String,
    message_output_index: Option<usize>,
    message_started: bool,
    text_content: String,
    function_calls: Vec<ResponseFunctionCallItem>,
    block_to_tool_index: HashMap<i32, usize>,
    output_order: Vec<ResponseOutputRef>,
    usage: Option<Usage>,
}

impl ResponsesState {
    fn new(model: impl Into<String>, include_usage: bool) -> Self {
        Self {
            response_id: format!("resp_{}", Uuid::new_v4().simple()),
            created_at: Utc::now().timestamp(),
            model: model.into(),
            include_usage,
            message_item_id: format!("msg_{}", Uuid::new_v4().simple()),
            message_output_index: None,
            message_started: false,
            text_content: String::new(),
            function_calls: Vec::new(),
            block_to_tool_index: HashMap::new(),
            output_order: Vec::new(),
            usage: None,
        }
    }

    fn consume_events(&mut self, events: Vec<SseEvent>) -> Vec<Bytes> {
        let mut bytes = Vec::new();
        for event in events {
            bytes.extend(self.consume_event(event));
        }
        bytes
    }

    fn consume_event(&mut self, event: SseEvent) -> Vec<Bytes> {
        match event.event.as_str() {
            "message_start" => self.handle_message_start(),
            "content_block_start" => self.handle_block_start(event.data),
            "content_block_delta" => self.handle_block_delta(event.data),
            "content_block_stop" => self.handle_block_stop(event.data),
            "message_delta" => {
                self.handle_message_delta(event.data);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn finalize_stream(&mut self, final_events: Vec<SseEvent>) -> Vec<Bytes> {
        let mut bytes = self.consume_events(final_events);
        bytes.extend(self.finish_output_items());
        bytes.push(self.sse_event("response.completed", json!({
            "type": "response.completed",
            "response": self.build_response_json()
        })));
        bytes
    }

    fn build_response_json(&self) -> serde_json::Value {
        let output: Vec<serde_json::Value> = self
            .output_order
            .iter()
            .map(|item| match item {
                ResponseOutputRef::Message => self.build_message_item_json("completed"),
                ResponseOutputRef::FunctionCall(index) => {
                    self.build_function_call_item_json(*index, "completed")
                }
            })
            .collect();

        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "status": "completed",
            "error": null,
            "incomplete_details": null,
            "model": self.model,
            "output": output,
            "parallel_tool_calls": true,
            "tool_choice": "auto",
            "usage": self.build_usage_json()
        })
    }

    fn handle_message_start(&mut self) -> Vec<Bytes> {
        vec![self.sse_event("response.created", json!({
            "type": "response.created",
            "response": {
                "id": self.response_id,
                "object": "response",
                "created_at": self.created_at,
                "status": "in_progress",
                "model": self.model,
                "output": []
            }
        }))]
    }

    fn handle_block_start(&mut self, data: serde_json::Value) -> Vec<Bytes> {
        let block_type = data["content_block"]["type"].as_str().unwrap_or("");
        if block_type != "tool_use" {
            return Vec::new();
        }

        let block_index = data["index"].as_i64().unwrap_or_default() as i32;
        let output_index = self.output_order.len();
        let fc_index = self.function_calls.len();
        self.output_order.push(ResponseOutputRef::FunctionCall(fc_index));
        self.block_to_tool_index.insert(block_index, fc_index);
        self.function_calls.push(ResponseFunctionCallItem {
            item_id: format!("fc_{}", Uuid::new_v4().simple()),
            call_id: data["content_block"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            name: data["content_block"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            arguments: String::new(),
            output_index,
            done: false,
        });

        vec![self.sse_event("response.output_item.added", json!({
            "type": "response.output_item.added",
            "response_id": self.response_id,
            "output_index": output_index,
            "item": self.build_function_call_item_json(fc_index, "in_progress")
        }))]
    }

    fn handle_block_delta(&mut self, data: serde_json::Value) -> Vec<Bytes> {
        let delta_type = data["delta"]["type"].as_str().unwrap_or("");
        match delta_type {
            "text_delta" => {
                let text = data["delta"]["text"].as_str().unwrap_or("");
                if text.is_empty() {
                    return Vec::new();
                }

                let mut bytes = Vec::new();
                if !self.message_started {
                    let output_index = self.output_order.len();
                    self.message_output_index = Some(output_index);
                    self.output_order.push(ResponseOutputRef::Message);
                    self.message_started = true;
                    bytes.push(self.sse_event("response.output_item.added", json!({
                        "type": "response.output_item.added",
                        "response_id": self.response_id,
                        "output_index": output_index,
                        "item": self.build_message_item_json("in_progress")
                    })));
                }

                self.text_content.push_str(text);
                bytes.push(self.sse_event("response.output_text.delta", json!({
                    "type": "response.output_text.delta",
                    "response_id": self.response_id,
                    "item_id": self.message_item_id,
                    "output_index": self.message_output_index.unwrap_or_default(),
                    "content_index": 0,
                    "delta": text
                })));
                bytes
            }
            "input_json_delta" => {
                let block_index = data["index"].as_i64().unwrap_or_default() as i32;
                let partial_json = data["delta"]["partial_json"]
                    .as_str()
                    .unwrap_or_default();
                if partial_json.is_empty() {
                    return Vec::new();
                }

                let Some(&fc_index) = self.block_to_tool_index.get(&block_index) else {
                    return Vec::new();
                };
                let (item_id, output_index) = {
                    let item = &mut self.function_calls[fc_index];
                    item.arguments.push_str(partial_json);
                    (item.item_id.clone(), item.output_index)
                };

                vec![self.sse_event("response.function_call_arguments.delta", json!({
                    "type": "response.function_call_arguments.delta",
                    "response_id": self.response_id,
                    "item_id": item_id,
                    "output_index": output_index,
                    "delta": partial_json
                }))]
            }
            _ => Vec::new(),
        }
    }

    fn handle_block_stop(&mut self, data: serde_json::Value) -> Vec<Bytes> {
        let block_index = data["index"].as_i64().unwrap_or_default() as i32;
        let Some(&fc_index) = self.block_to_tool_index.get(&block_index) else {
            return Vec::new();
        };

        self.finish_function_call_item(fc_index)
    }

    fn handle_message_delta(&mut self, data: serde_json::Value) {
        let prompt_tokens = data["usage"]["input_tokens"]
            .as_i64()
            .unwrap_or_default() as i32;
        let completion_tokens = data["usage"]["output_tokens"]
            .as_i64()
            .unwrap_or_default() as i32;
        self.usage = Some(Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        });
    }

    fn finish_output_items(&mut self) -> Vec<Bytes> {
        let mut bytes = Vec::new();

        if self.message_started {
            bytes.push(self.sse_event("response.output_text.done", json!({
                "type": "response.output_text.done",
                "response_id": self.response_id,
                "item_id": self.message_item_id,
                "output_index": self.message_output_index.unwrap_or_default(),
                "content_index": 0,
                "text": self.text_content
            })));
            bytes.push(self.sse_event("response.output_item.done", json!({
                "type": "response.output_item.done",
                "response_id": self.response_id,
                "output_index": self.message_output_index.unwrap_or_default(),
                "item": self.build_message_item_json("completed")
            })));
        }

        for index in 0..self.function_calls.len() {
            bytes.extend(self.finish_function_call_item(index));
        }

        if self.include_usage {
            if let Some(usage) = &self.usage {
                bytes.push(self.sse_event("response.usage", json!({
                    "type": "response.usage",
                    "response_id": self.response_id,
                    "usage": {
                        "input_tokens": usage.prompt_tokens,
                        "output_tokens": usage.completion_tokens,
                        "total_tokens": usage.total_tokens
                    }
                })));
            }
        }

        bytes
    }

    fn finish_function_call_item(&mut self, index: usize) -> Vec<Bytes> {
        let Some(item) = self.function_calls.get_mut(index) else {
            return Vec::new();
        };
        if item.done {
            return Vec::new();
        }
        item.done = true;
        let item_id = item.item_id.clone();
        let output_index = item.output_index;
        let arguments = item.arguments.clone();
        let item_json = json!({
            "id": item.item_id,
            "type": "function_call",
            "status": "completed",
            "call_id": item.call_id,
            "name": item.name,
            "arguments": item.arguments
        });

        vec![
            self.sse_event("response.function_call_arguments.done", json!({
                "type": "response.function_call_arguments.done",
                "response_id": self.response_id,
                "item_id": item_id,
                "output_index": output_index,
                "arguments": arguments
            })),
            self.sse_event("response.output_item.done", json!({
                "type": "response.output_item.done",
                "response_id": self.response_id,
                "output_index": output_index,
                "item": item_json
            })),
        ]
    }

    fn build_message_item_json(&self, status: &str) -> serde_json::Value {
        json!({
            "id": self.message_item_id,
            "type": "message",
            "status": status,
            "role": "assistant",
            "content": [
                {
                    "type": "output_text",
                    "text": self.text_content,
                    "annotations": []
                }
            ]
        })
    }

    fn build_function_call_item_json(&self, index: usize, status: &str) -> serde_json::Value {
        let item = &self.function_calls[index];
        json!({
            "id": item.item_id,
            "type": "function_call",
            "status": status,
            "call_id": item.call_id,
            "name": item.name,
            "arguments": item.arguments
        })
    }

    fn build_usage_json(&self) -> serde_json::Value {
        let usage = self.usage.clone().unwrap_or(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        });

        json!({
            "input_tokens": usage.prompt_tokens,
            "input_tokens_details": {
                "cached_tokens": 0
            },
            "output_tokens": usage.completion_tokens,
            "output_tokens_details": {
                "reasoning_tokens": 0
            },
            "total_tokens": usage.total_tokens
        })
    }

    fn sse_event(&self, event: &str, payload: serde_json::Value) -> Bytes {
        Bytes::from(format!(
            "event: {}\ndata: {}\n\n",
            event,
            serde_json::to_string(&payload).unwrap_or_default()
        ))
    }
}

#[derive(Debug)]
struct OpenAiResponseState {
    id: String,
    created: i64,
    model: String,
    thinking_enabled: bool,
    include_usage_chunk: bool,
    role_sent: bool,
    text_content: String,
    tool_calls: Vec<ToolCall>,
    tool_call_indices: HashMap<i32, usize>,
    usage: Option<Usage>,
    finish_reason: Option<String>,
}

impl OpenAiResponseState {
    fn new(model: impl Into<String>, thinking_enabled: bool, include_usage_chunk: bool) -> Self {
        Self {
            id: format!("chatcmpl-{}", Uuid::new_v4().simple()),
            created: Utc::now().timestamp(),
            model: model.into(),
            thinking_enabled,
            include_usage_chunk,
            role_sent: false,
            text_content: String::new(),
            tool_calls: Vec::new(),
            tool_call_indices: HashMap::new(),
            usage: None,
            finish_reason: None,
        }
    }

    fn consume_events(&mut self, events: Vec<SseEvent>) -> Vec<Bytes> {
        let mut bytes = Vec::new();
        for event in events {
            bytes.extend(self.consume_event(event));
        }
        bytes
    }

    fn consume_event(&mut self, event: SseEvent) -> Vec<Bytes> {
        match event.event.as_str() {
            "message_start" => self.handle_message_start(),
            "content_block_start" => self.handle_block_start(event.data),
            "content_block_delta" => self.handle_block_delta(event.data),
            "message_delta" => self.handle_message_delta(event.data),
            _ => Vec::new(),
        }
    }

    fn handle_message_start(&mut self) -> Vec<Bytes> {
        if self.role_sent {
            return Vec::new();
        }

        self.role_sent = true;
        vec![self.sse_chunk(ChatCompletionChunk {
            id: self.id.clone(),
            object: "chat.completion.chunk".to_string(),
            created: self.created,
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: DeltaMessage {
                    role: Some("assistant".to_string()),
                    ..Default::default()
                },
                finish_reason: None,
            }],
            usage: None,
        })]
    }

    fn handle_block_start(&mut self, data: serde_json::Value) -> Vec<Bytes> {
        let block_type = data["content_block"]["type"].as_str().unwrap_or("");
        if block_type != "tool_use" {
            return Vec::new();
        }

        let block_index = data["index"].as_i64().unwrap_or_default() as i32;
        let tool_call_index = self.tool_calls.len();
        let tool_call = ToolCall {
            id: data["content_block"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            call_type: "function".to_string(),
            function: ToolCallFunction {
                name: data["content_block"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                arguments: String::new(),
            },
        };

        self.tool_call_indices.insert(block_index, tool_call_index);
        self.tool_calls.push(tool_call.clone());

        vec![self.sse_chunk(ChatCompletionChunk {
            id: self.id.clone(),
            object: "chat.completion.chunk".to_string(),
            created: self.created,
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: DeltaMessage {
                    tool_calls: Some(vec![DeltaToolCall {
                        index: tool_call_index as i32,
                        id: Some(tool_call.id),
                        call_type: Some("function".to_string()),
                        function: Some(DeltaToolCallFunction {
                            name: Some(tool_call.function.name),
                            arguments: Some(String::new()),
                        }),
                    }]),
                    ..Default::default()
                },
                finish_reason: None,
            }],
            usage: None,
        })]
    }

    fn handle_block_delta(&mut self, data: serde_json::Value) -> Vec<Bytes> {
        let delta_type = data["delta"]["type"].as_str().unwrap_or("");
        match delta_type {
            "text_delta" => {
                let text = data["delta"]["text"].as_str().unwrap_or("");
                if text.is_empty() {
                    return Vec::new();
                }
                self.text_content.push_str(text);
                vec![self.sse_chunk(ChatCompletionChunk {
                    id: self.id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: self.created,
                    model: self.model.clone(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: DeltaMessage {
                            content: Some(text.to_string()),
                            ..Default::default()
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                })]
            }
            "input_json_delta" => {
                let block_index = data["index"].as_i64().unwrap_or_default() as i32;
                let partial_json = data["delta"]["partial_json"]
                    .as_str()
                    .unwrap_or_default();
                if partial_json.is_empty() {
                    return Vec::new();
                }

                let Some(&tool_call_index) = self.tool_call_indices.get(&block_index) else {
                    return Vec::new();
                };

                if let Some(tool_call) = self.tool_calls.get_mut(tool_call_index) {
                    tool_call.function.arguments.push_str(partial_json);
                }

                vec![self.sse_chunk(ChatCompletionChunk {
                    id: self.id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: self.created,
                    model: self.model.clone(),
                    choices: vec![ChunkChoice {
                        index: 0,
                        delta: DeltaMessage {
                            tool_calls: Some(vec![DeltaToolCall {
                                index: tool_call_index as i32,
                                id: None,
                                call_type: None,
                                function: Some(DeltaToolCallFunction {
                                    name: None,
                                    arguments: Some(partial_json.to_string()),
                                }),
                            }]),
                            ..Default::default()
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                })]
            }
            _ => Vec::new(),
        }
    }

    fn handle_message_delta(&mut self, data: serde_json::Value) -> Vec<Bytes> {
        let prompt_tokens = data["usage"]["input_tokens"]
            .as_i64()
            .unwrap_or_default() as i32;
        let completion_tokens = data["usage"]["output_tokens"]
            .as_i64()
            .unwrap_or_default() as i32;

        self.usage = Some(Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        });
        self.finish_reason = Some(map_stop_reason(
            data["delta"]["stop_reason"].as_str().unwrap_or("end_turn"),
        ));

        let mut bytes = vec![self.sse_chunk(ChatCompletionChunk {
            id: self.id.clone(),
            object: "chat.completion.chunk".to_string(),
            created: self.created,
            model: self.model.clone(),
            choices: vec![ChunkChoice {
                index: 0,
                delta: DeltaMessage::default(),
                finish_reason: self.finish_reason.clone(),
            }],
            usage: None,
        })];

        if self.include_usage_chunk {
            if let Some(usage) = self.usage.clone() {
                bytes.push(self.sse_chunk(ChatCompletionChunk {
                    id: self.id.clone(),
                    object: "chat.completion.chunk".to_string(),
                    created: self.created,
                    model: self.model.clone(),
                    choices: Vec::new(),
                    usage: Some(usage),
                }));
            }
        }

        bytes
    }

    fn build_response(self) -> ChatCompletionResponse {
        let content = match self.text_content.as_str() {
            "" => None,
            " " if self.thinking_enabled && self.tool_calls.is_empty() => None,
            _ => Some(self.text_content),
        };

        let tool_calls = (!self.tool_calls.is_empty()).then_some(self.tool_calls);
        let usage = self.usage.unwrap_or(Usage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        });
        let finish_reason = self.finish_reason.unwrap_or_else(|| {
            if tool_calls.is_some() {
                "tool_calls".to_string()
            } else {
                "stop".to_string()
            }
        });

        ChatCompletionResponse {
            id: self.id,
            object: "chat.completion".to_string(),
            created: self.created,
            model: self.model,
            choices: vec![Choice {
                index: 0,
                message: AssistantMessage {
                    role: "assistant".to_string(),
                    content,
                    tool_calls,
                },
                finish_reason,
            }],
            usage,
        }
    }

    fn sse_chunk(&self, chunk: ChatCompletionChunk) -> Bytes {
        let payload = serde_json::to_string(&chunk).unwrap_or_default();
        Bytes::from(format!("data: {}\n\n", payload))
    }
}

fn map_stop_reason(reason: &str) -> String {
    match reason {
        "tool_use" => "tool_calls".to_string(),
        "max_tokens" | "model_context_window_exceeded" => "length".to_string(),
        _ => "stop".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_response_state_builds_tool_calls() {
        let mut state = OpenAiResponseState::new("claude-sonnet-4-6", false, true);
        let _ = state.consume_events(vec![SseEvent::new(
            "message_start",
            json!({"type": "message_start"}),
        )]);
        let _ = state.consume_events(vec![SseEvent::new(
            "content_block_start",
            json!({
                "index": 1,
                "content_block": {
                    "type": "tool_use",
                    "id": "tool_1",
                    "name": "read_file"
                }
            }),
        )]);
        let _ = state.consume_events(vec![SseEvent::new(
            "content_block_delta",
            json!({
                "index": 1,
                "delta": {
                    "type": "input_json_delta",
                    "partial_json": "{\"path\":\"/tmp/a.txt\"}"
                }
            }),
        )]);
        let _ = state.consume_events(vec![SseEvent::new(
            "message_delta",
            json!({
                "delta": {"stop_reason": "tool_use"},
                "usage": {"input_tokens": 10, "output_tokens": 5}
            }),
        )]);

        let response = state.build_response();
        assert_eq!(response.choices[0].finish_reason, "tool_calls");
        assert_eq!(
            response.choices[0]
                .message
                .tool_calls
                .as_ref()
                .expect("tool calls")
                .len(),
            1
        );
        assert_eq!(response.usage.total_tokens, 15);
    }

    #[test]
    fn test_responses_state_builds_response_json() {
        let mut state = ResponsesState::new("claude-sonnet-4-6", true);
        let _ = state.consume_events(vec![SseEvent::new(
            "message_start",
            json!({"type": "message_start"}),
        )]);
        let _ = state.consume_events(vec![SseEvent::new(
            "content_block_delta",
            json!({
                "index": 0,
                "delta": {
                    "type": "text_delta",
                    "text": "hello"
                }
            }),
        )]);
        let _ = state.consume_events(vec![SseEvent::new(
            "message_delta",
            json!({
                "delta": {"stop_reason": "end_turn"},
                "usage": {"input_tokens": 10, "output_tokens": 5}
            }),
        )]);

        let response = state.build_response_json();
        assert_eq!(response["object"], "response");
        assert_eq!(response["status"], "completed");
        assert_eq!(response["output"][0]["type"], "message");
        assert_eq!(response["output"][0]["content"][0]["text"], "hello");
        assert_eq!(response["usage"]["total_tokens"], 15);
    }
}
