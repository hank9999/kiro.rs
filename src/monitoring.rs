use std::{
    collections::VecDeque,
    future::Future,
    sync::Arc,
    time::Instant,
};

use axum::{http::StatusCode, response::Response};
use chrono::Utc;
use parking_lot::Mutex;
use serde::Serialize;

const DEFAULT_CAPACITY: usize = 500;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestRecord {
    pub id: u64,
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

        RequestTracker {
            monitor: self.clone(),
            metadata,
            id,
            started_at,
            started: Instant::now(),
            finished: false,
        }
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
        };

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
    started_at: String,
    started: Instant,
    finished: bool,
}

impl RequestTracker {
    pub fn finish(mut self, status_code: u16, error: Option<String>) {
        if self.finished {
            return;
        }

        self.finished = true;
        self.monitor.finish_request(
            self.id,
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
            self.metadata.clone(),
            self.started_at.clone(),
            self.started.elapsed().as_millis() as u64,
            499,
            Some("Request dropped before completion".to_string()),
        );
    }
}

pub async fn track_response<F>(monitor: RequestMonitor, metadata: RequestMetadata, future: F) -> Response
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
        "/v1/chat/completions" => "OpenAI Chat".to_string(),
        "/v1/responses" => "OpenAI Responses".to_string(),
        "/v1/messages" => "Anthropic Messages".to_string(),
        "/cc/v1/messages" => "Claude Code Messages".to_string(),
        "/v1/messages/count_tokens" | "/cc/v1/messages/count_tokens" => {
            "Count Tokens".to_string()
        }
        "/v1/models" => "Model List".to_string(),
        _ => path.to_string(),
    }
}
