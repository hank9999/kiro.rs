use std::{collections::VecDeque, future::Future, sync::Arc, time::Instant};

use axum::{http::StatusCode, response::Response};
use chrono::Utc;
use parking_lot::Mutex;
use serde::Serialize;

const DEFAULT_CAPACITY: usize = 500;
const SLOW_REQUEST_WARN_THRESHOLD_MS: u64 = 30_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecord {
    pub id: u64,
    pub request_id: String,
    pub method: String,
    pub path: String,
    pub endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_count: Option<usize>,
    pub stream: bool,
    pub status_code: u16,
    pub success: bool,
    pub duration_ms: u64,
    pub started_at: String,
    pub finished_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_for: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_proto: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub referer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfer_encoding: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSummary {
    pub total_requests: u64,
    pub success_requests: u64,
    pub failed_requests: u64,
    pub in_flight: u64,
    pub success_rate: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestActivitySnapshot {
    pub summary: RequestSummary,
    pub records: Vec<RequestRecord>,
}

#[derive(Debug, Clone)]
pub struct RequestMetadata {
    method: String,
    path: String,
    endpoint: String,
    model: Option<String>,
    message_count: Option<usize>,
    stream: bool,
    client_ip: Option<String>,
    forwarded_for: Option<String>,
    real_ip: Option<String>,
    forwarded_proto: Option<String>,
    user_agent: Option<String>,
    referer: Option<String>,
    origin: Option<String>,
    transfer_encoding: Option<String>,
    content_length: Option<u64>,
    client_request_id: Option<String>,
}

impl RequestMetadata {
    pub fn new(method: impl Into<String>, path: impl Into<String>) -> Self {
        let path = path.into();
        Self {
            method: method.into(),
            endpoint: endpoint_name(&path),
            path,
            model: None,
            message_count: None,
            stream: false,
            client_ip: None,
            forwarded_for: None,
            real_ip: None,
            forwarded_proto: None,
            user_agent: None,
            referer: None,
            origin: None,
            transfer_encoding: None,
            content_length: None,
            client_request_id: None,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_message_count(mut self, message_count: usize) -> Self {
        self.message_count = Some(message_count);
        self
    }

    pub fn with_stream(mut self, stream: bool) -> Self {
        self.stream = stream;
        self
    }

    pub fn with_client_ip(mut self, client_ip: Option<String>) -> Self {
        self.client_ip = client_ip;
        self
    }

    pub fn with_forwarded_for(mut self, forwarded_for: Option<String>) -> Self {
        self.forwarded_for = forwarded_for;
        self
    }

    pub fn with_real_ip(mut self, real_ip: Option<String>) -> Self {
        self.real_ip = real_ip;
        self
    }

    pub fn with_forwarded_proto(mut self, forwarded_proto: Option<String>) -> Self {
        self.forwarded_proto = forwarded_proto;
        self
    }

    pub fn with_user_agent(mut self, user_agent: Option<String>) -> Self {
        self.user_agent = user_agent;
        self
    }

    pub fn with_referer(mut self, referer: Option<String>) -> Self {
        self.referer = referer;
        self
    }

    pub fn with_origin(mut self, origin: Option<String>) -> Self {
        self.origin = origin;
        self
    }

    pub fn with_transfer_encoding(mut self, transfer_encoding: Option<String>) -> Self {
        self.transfer_encoding = transfer_encoding;
        self
    }

    pub fn with_content_length(mut self, content_length: Option<u64>) -> Self {
        self.content_length = content_length;
        self
    }

    pub fn with_client_request_id(mut self, client_request_id: Option<String>) -> Self {
        self.client_request_id = client_request_id;
        self
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn client_ip(&self) -> Option<&str> {
        self.client_ip.as_deref()
    }

    pub fn client_request_id(&self) -> Option<&str> {
        self.client_request_id.as_deref()
    }
}

#[derive(Debug, Clone)]
pub struct RequestMonitor {
    inner: Arc<Mutex<RequestMonitorState>>,
}

#[derive(Debug)]
struct RequestMonitorState {
    next_id: u64,
    total_requests: u64,
    success_requests: u64,
    failed_requests: u64,
    in_flight: u64,
    last_updated_at: Option<String>,
    capacity: usize,
    records: VecDeque<RequestRecord>,
}

impl Default for RequestMonitor {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl RequestMonitor {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RequestMonitorState {
                next_id: 0,
                total_requests: 0,
                success_requests: 0,
                failed_requests: 0,
                in_flight: 0,
                last_updated_at: None,
                capacity: capacity.max(1),
                records: VecDeque::with_capacity(capacity.max(1)),
            })),
        }
    }

    pub fn start(&self, metadata: RequestMetadata) -> RequestTracker {
        let started_at = Utc::now().to_rfc3339();

        let id = {
            let mut inner = self.inner.lock();
            inner.next_id += 1;
            inner.in_flight += 1;
            inner.next_id
        };

        let tracker = RequestTracker {
            monitor: self.clone(),
            metadata,
            id,
            request_id: format!("req-{id:06}"),
            started_at,
            started: Instant::now(),
            finished: false,
        };
        tracker.log_started();
        tracker
    }

    pub fn snapshot(&self, limit: usize) -> RequestActivitySnapshot {
        let inner = self.inner.lock();
        let total_requests = inner.total_requests;
        let success_requests = inner.success_requests;
        let failed_requests = inner.failed_requests;
        let success_rate = if total_requests == 0 {
            0.0
        } else {
            (success_requests as f64 / total_requests as f64) * 100.0
        };

        let records = inner
            .records
            .iter()
            .rev()
            .take(limit.max(1))
            .cloned()
            .collect();

        RequestActivitySnapshot {
            summary: RequestSummary {
                total_requests,
                success_requests,
                failed_requests,
                in_flight: inner.in_flight,
                success_rate,
                last_updated_at: inner.last_updated_at.clone(),
            },
            records,
        }
    }

    fn finish_request(
        &self,
        id: u64,
        request_id: String,
        metadata: RequestMetadata,
        started_at: String,
        duration_ms: u64,
        status_code: u16,
        error: Option<String>,
    ) {
        let success = status_code < 400;
        let finished_at = Utc::now().to_rfc3339();
        let error = error.or_else(|| {
            if success {
                None
            } else {
                StatusCode::from_u16(status_code)
                    .ok()
                    .and_then(|status| status.canonical_reason().map(|reason| reason.to_string()))
            }
        });

        let record = RequestRecord {
            id,
            request_id,
            method: metadata.method,
            path: metadata.path,
            endpoint: metadata.endpoint,
            model: metadata.model,
            message_count: metadata.message_count,
            stream: metadata.stream,
            status_code,
            success,
            duration_ms,
            started_at,
            finished_at: finished_at.clone(),
            error,
            client_ip: metadata.client_ip,
            forwarded_for: metadata.forwarded_for,
            real_ip: metadata.real_ip,
            forwarded_proto: metadata.forwarded_proto,
            user_agent: metadata.user_agent,
            referer: metadata.referer,
            origin: metadata.origin,
            transfer_encoding: metadata.transfer_encoding,
            content_length: metadata.content_length,
            client_request_id: metadata.client_request_id,
        };

        log_request_completion(&record);

        let mut inner = self.inner.lock();
        inner.in_flight = inner.in_flight.saturating_sub(1);
        inner.total_requests += 1;
        if success {
            inner.success_requests += 1;
        } else {
            inner.failed_requests += 1;
        }
        inner.last_updated_at = Some(finished_at);
        inner.records.push_back(record);

        while inner.records.len() > inner.capacity {
            inner.records.pop_front();
        }
    }
}

pub struct RequestTracker {
    monitor: RequestMonitor,
    metadata: RequestMetadata,
    id: u64,
    request_id: String,
    started_at: String,
    started: Instant,
    finished: bool,
}

impl RequestTracker {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    fn log_started(&self) {
        tracing::info!(
            request_id = %self.request_id,
            method = %self.metadata.method,
            path = %self.metadata.path,
            endpoint = %self.metadata.endpoint,
            client_ip = %self.metadata.client_ip.as_deref().unwrap_or("-"),
            forwarded_for = %self.metadata.forwarded_for.as_deref().unwrap_or("-"),
            real_ip = %self.metadata.real_ip.as_deref().unwrap_or("-"),
            client_request_id = %self.metadata.client_request_id.as_deref().unwrap_or("-"),
            user_agent = %self.metadata.user_agent.as_deref().unwrap_or("-"),
            content_length = ?self.metadata.content_length,
            stream = self.metadata.stream,
            "API request started"
        );
    }

    pub fn finish(mut self, status_code: u16, error: Option<String>) {
        if self.finished {
            return;
        }

        self.finished = true;
        self.monitor.finish_request(
            self.id,
            self.request_id.clone(),
            self.metadata.clone(),
            self.started_at.clone(),
            self.started.elapsed().as_millis() as u64,
            status_code,
            error,
        );
    }
}

impl Drop for RequestTracker {
    fn drop(&mut self) {
        if self.finished {
            return;
        }

        self.finished = true;
        self.monitor.finish_request(
            self.id,
            self.request_id.clone(),
            self.metadata.clone(),
            self.started_at.clone(),
            self.started.elapsed().as_millis() as u64,
            499,
            Some(
                "Request dropped before completion (likely client disconnect, timeout, or handler cancellation)"
                    .to_string(),
            ),
        );
    }
}

pub async fn track_response<F>(
    monitor: RequestMonitor,
    metadata: RequestMetadata,
    future: F,
) -> Response
where
    F: Future<Output = Response>,
{
    let tracker = monitor.start(metadata);
    let response = future.await;
    let status = response.status().as_u16();
    tracker.finish(status, None);
    response
}

fn endpoint_name(path: &str) -> String {
    match path {
        "/v1/chat/completions" | "/chat/completions" => "OpenAI Chat".to_string(),
        "/v1/responses" | "/responses" => "OpenAI Responses".to_string(),
        "/v1/messages" | "/messages" => "Anthropic Messages".to_string(),
        "/cc/v1/messages" | "/cc/messages" => "Claude Code Messages".to_string(),
        "/v1/messages/count_tokens" | "/cc/v1/messages/count_tokens" | "/messages/count_tokens" => {
            "Count Tokens".to_string()
        }
        "/v1/models" | "/models" => "Model List".to_string(),
        _ => path.to_string(),
    }
}

fn log_request_completion(record: &RequestRecord) {
    let client_ip = record.client_ip.as_deref().unwrap_or("-");
    let forwarded_for = record.forwarded_for.as_deref().unwrap_or("-");
    let client_request_id = record.client_request_id.as_deref().unwrap_or("-");
    let user_agent = record.user_agent.as_deref().unwrap_or("-");
    let error = record.error.as_deref().unwrap_or("-");

    if record.status_code == 499 {
        tracing::warn!(
            request_id = %record.request_id,
            method = %record.method,
            path = %record.path,
            endpoint = %record.endpoint,
            status_code = record.status_code,
            duration_ms = record.duration_ms,
            client_ip = %client_ip,
            forwarded_for = %forwarded_for,
            client_request_id = %client_request_id,
            user_agent = %user_agent,
            error = %error,
            "API request dropped before completion"
        );
        return;
    }

    if !record.success {
        tracing::warn!(
            request_id = %record.request_id,
            method = %record.method,
            path = %record.path,
            endpoint = %record.endpoint,
            status_code = record.status_code,
            duration_ms = record.duration_ms,
            client_ip = %client_ip,
            forwarded_for = %forwarded_for,
            client_request_id = %client_request_id,
            user_agent = %user_agent,
            error = %error,
            "API request failed"
        );
        return;
    }

    if record.duration_ms >= SLOW_REQUEST_WARN_THRESHOLD_MS {
        tracing::warn!(
            request_id = %record.request_id,
            method = %record.method,
            path = %record.path,
            endpoint = %record.endpoint,
            status_code = record.status_code,
            duration_ms = record.duration_ms,
            client_ip = %client_ip,
            user_agent = %user_agent,
            "API request completed slowly"
        );
        return;
    }

    tracing::info!(
        request_id = %record.request_id,
        method = %record.method,
        path = %record.path,
        endpoint = %record.endpoint,
        status_code = record.status_code,
        duration_ms = record.duration_ms,
        "API request completed"
    );
}

#[cfg(test)]
mod tests {
    use super::{RequestMetadata, RequestMonitor, endpoint_name};

    #[test]
    fn dropped_request_is_recorded_as_499_with_request_id() {
        let monitor = RequestMonitor::new(10);

        {
            let _tracker = monitor.start(
                RequestMetadata::new("POST", "/v1/chat/completions")
                    .with_client_ip(Some("127.0.0.1".to_string())),
            );
        }

        let snapshot = monitor.snapshot(10);
        let record = &snapshot.records[0];

        assert_eq!(record.status_code, 499);
        assert_eq!(record.request_id, "req-000001");
        assert_eq!(record.client_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(record.endpoint, "OpenAI Chat");
    }

    #[test]
    fn nested_route_paths_are_normalized_for_display() {
        assert_eq!(endpoint_name("/chat/completions"), "OpenAI Chat");
        assert_eq!(endpoint_name("/models"), "Model List");
    }
}
