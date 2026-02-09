//! 流量监控存储和异步服务

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use rusqlite::Connection;
use tokio::sync::mpsc;

use super::model::FlowRecord;
use super::types::{FlowQuery, FlowStatsResponse, ModelStats, FlowListResponse, FlowRecordResponse};

/// 底层 SQLite 存储（同步）
struct FlowStore {
    conn: std::sync::Mutex<Connection>,
}

impl FlowStore {
    fn new(db_path: &str) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS flow_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                request_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                method TEXT NOT NULL DEFAULT 'POST',
                path TEXT NOT NULL,
                model TEXT NOT NULL,
                stream INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER,
                output_tokens INTEGER,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                status_code INTEGER NOT NULL DEFAULT 200,
                error TEXT,
                user_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_flow_timestamp ON flow_records(timestamp);
            CREATE INDEX IF NOT EXISTS idx_flow_model ON flow_records(model);
            CREATE INDEX IF NOT EXISTS idx_flow_status ON flow_records(status_code);",
        )?;
        Ok(Self {
            conn: std::sync::Mutex::new(conn),
        })
    }

    fn insert_batch(&self, records: &[FlowRecord]) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        for record in records {
            tx.execute(
                "INSERT INTO flow_records (request_id, timestamp, method, path, model, stream, input_tokens, output_tokens, duration_ms, status_code, error, user_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                rusqlite::params![
                    record.request_id,
                    record.timestamp,
                    record.method,
                    record.path,
                    record.model,
                    record.stream as i32,
                    record.input_tokens,
                    record.output_tokens,
                    record.duration_ms,
                    record.status_code as i32,
                    record.error,
                    record.user_id,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn query(&self, filter: &FlowQuery) -> Result<FlowListResponse> {
        let conn = self.conn.lock().unwrap();
        let page = filter.page.unwrap_or(1).max(1);
        let page_size = filter.page_size.unwrap_or(50).clamp(1, 200);
        let offset = (page - 1) * page_size;

        let mut where_clauses = Vec::new();
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(ref model) = filter.model {
            where_clauses.push(format!("model = ?{}", params.len() + 1));
            params.push(Box::new(model.clone()));
        }
        if let Some(ref status) = filter.status {
            match status.as_str() {
                "success" => {
                    where_clauses.push(format!("status_code < ?{}", params.len() + 1));
                    params.push(Box::new(400i32));
                }
                "error" => {
                    where_clauses.push(format!("status_code >= ?{}", params.len() + 1));
                    params.push(Box::new(400i32));
                }
                _ => {}
            }
        }
        if let Some(ref start_time) = filter.start_time {
            let normalized = chrono::DateTime::parse_from_rfc3339(start_time)
                .map(|dt| dt.with_timezone(&Utc).to_rfc3339())
                .unwrap_or_else(|_| start_time.clone());
            where_clauses.push(format!("timestamp >= ?{}", params.len() + 1));
            params.push(Box::new(normalized));
        }
        if let Some(ref end_time) = filter.end_time {
            let normalized = chrono::DateTime::parse_from_rfc3339(end_time)
                .map(|dt| dt.with_timezone(&Utc).to_rfc3339())
                .unwrap_or_else(|_| end_time.clone());
            where_clauses.push(format!("timestamp <= ?{}", params.len() + 1));
            params.push(Box::new(normalized));
        }

        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        // Count total
        let count_sql = format!("SELECT COUNT(*) FROM flow_records {}", where_sql);
        let total: u64 = conn.query_row(
            &count_sql,
            rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
            |row| row.get(0),
        )?;

        // Query records
        let query_sql = format!(
            "SELECT id, request_id, timestamp, method, path, model, stream, input_tokens, output_tokens, duration_ms, status_code, error, user_id
             FROM flow_records {} ORDER BY id DESC LIMIT ?{} OFFSET ?{}",
            where_sql,
            params.len() + 1,
            params.len() + 2
        );
        params.push(Box::new(page_size as i64));
        params.push(Box::new(offset as i64));

        let mut stmt = conn.prepare(&query_sql)?;
        let records = stmt
            .query_map(
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                |row| {
                    let input_tokens: Option<i64> = row.get(7)?;
                    let output_tokens: Option<i64> = row.get(8)?;
                    let total_tokens = match (input_tokens, output_tokens) {
                        (Some(i), Some(o)) => Some(i + o),
                        _ => None,
                    };
                    Ok(FlowRecordResponse {
                        id: row.get(0)?,
                        request_id: row.get(1)?,
                        timestamp: row.get(2)?,
                        path: row.get(4)?,
                        model: row.get(5)?,
                        stream: row.get::<_, i32>(6)? != 0,
                        input_tokens,
                        output_tokens,
                        total_tokens,
                        duration_ms: row.get(9)?,
                        status_code: row.get::<_, i32>(10)? as u16,
                        error: row.get(11)?,
                        user_id: row.get(12)?,
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(FlowListResponse {
            total,
            page,
            page_size,
            records,
        })
    }

    fn get_stats(&self) -> Result<FlowStatsResponse> {
        let conn = self.conn.lock().unwrap();

        let (total_requests, total_input, total_output, avg_duration, error_count): (u64, i64, i64, f64, u64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(AVG(duration_ms), 0), COUNT(CASE WHEN status_code >= 400 THEN 1 END) FROM flow_records",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )?;

        let error_rate = if total_requests > 0 {
            error_count as f64 / total_requests as f64
        } else {
            0.0
        };

        let mut stmt = conn.prepare(
            "SELECT model, COUNT(*), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(AVG(duration_ms), 0) FROM flow_records GROUP BY model ORDER BY COUNT(*) DESC"
        )?;
        let models = stmt
            .query_map([], |row| {
                Ok(ModelStats {
                    model: row.get(0)?,
                    count: row.get(1)?,
                    total_input_tokens: row.get(2)?,
                    total_output_tokens: row.get(3)?,
                    avg_duration_ms: row.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(FlowStatsResponse {
            total_requests,
            total_input_tokens: total_input,
            total_output_tokens: total_output,
            total_tokens: total_input + total_output,
            avg_duration_ms: avg_duration,
            error_count,
            error_rate,
            models,
        })
    }

    fn clear(&self, before: Option<&str>) -> Result<u64> {
        let conn = self.conn.lock().unwrap();
        let count = if let Some(before) = before {
            let normalized = chrono::DateTime::parse_from_rfc3339(before)
                .map(|dt| dt.with_timezone(&Utc).to_rfc3339())
                .unwrap_or_else(|_| before.to_string());
            conn.execute("DELETE FROM flow_records WHERE timestamp < ?1", [&normalized])?
        } else {
            conn.execute("DELETE FROM flow_records", [])?
        };
        Ok(count as u64)
    }
}

/// 异步流量监控服务（公开 API）
pub struct FlowMonitor {
    sender: mpsc::Sender<FlowRecord>,
    store: Arc<FlowStore>,
}

impl FlowMonitor {
    /// 创建新的 FlowMonitor，启动后台写入任务
    pub fn new(db_path: &str) -> Result<Self> {
        let store = Arc::new(FlowStore::new(db_path)?);
        let (sender, mut receiver) = mpsc::channel::<FlowRecord>(10_000);

        let write_store = store.clone();
        tokio::spawn(async move {
            while let Some(first) = receiver.recv().await {
                // Drain all available records into a batch
                let mut batch = vec![first];
                while let Ok(record) = receiver.try_recv() {
                    batch.push(record);
                    if batch.len() >= 500 {
                        break;
                    }
                }
                let store = write_store.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = store.insert_batch(&batch) {
                        tracing::error!("批量写入流量记录失败: {}", e);
                    }
                }).await;
            }
        });

        Ok(Self { sender, store })
    }

    /// 非阻塞记录流量（发送到 channel）
    pub fn record(&self, record: FlowRecord) {
        if self.sender.try_send(record).is_err() {
            tracing::warn!("流量记录通道已满，丢弃记录");
        }
    }

    /// 查询流量记录
    pub async fn query(&self, filter: FlowQuery) -> Result<FlowListResponse> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.query(&filter)).await?
    }

    /// 获取统计信息
    pub async fn get_stats(&self) -> Result<FlowStatsResponse> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.get_stats()).await?
    }

    /// 清空记录
    pub async fn clear(&self, before: Option<String>) -> Result<u64> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.clear(before.as_deref())).await?
    }
}
