//! Prompt Caching 模块 - 使用 Redis 实现前缀 hash 匹配

use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::anthropic::types::{CacheControl, Message, SystemMessage, Tool};
use crate::token;

/// 全局 Redis 连接管理器
static REDIS_CONN: OnceLock<ConnectionManager> = OnceLock::new();

/// 默认 TTL: 5 分钟
const DEFAULT_TTL_SECS: u64 = 5 * 60;
/// 1 小时 TTL
const EXTENDED_TTL_SECS: u64 = 60 * 60;

/// 缓存断点信息
#[derive(Debug, Clone)]
pub struct CacheBreakpoint {
    pub hash: String,   // 累积 hash 值
    pub tokens: i32,    // 累积 token 数
    pub ttl: u64,       // TTL（秒）
}

/// 缓存查询结果
#[derive(Debug, Clone, Default)]
pub struct CacheResult {
    pub cache_read_input_tokens: i32,      // 命中时的 token 数
    pub cache_creation_input_tokens: i32,  // 未命中时写入的 token 数
    pub uncached_input_tokens: i32,        // 断点之后的 token 数
}

/// 初始化 Redis 连接
pub async fn init_redis(redis_url: &str) -> anyhow::Result<()> {
    let client = redis::Client::open(redis_url)?;
    let conn = ConnectionManager::new(client).await?;
    REDIS_CONN
        .set(conn)
        .map_err(|_| anyhow::anyhow!("Redis already initialized"))?;
    tracing::info!("Redis cache initialized: {}", redis_url);
    Ok(())
}

/// 检查 Redis 是否已初始化
pub fn is_redis_available() -> bool {
    REDIS_CONN.get().is_some()
}

/// 计算请求的缓存断点
pub fn compute_cache_breakpoints(
    tools: &Option<Vec<Tool>>,
    system: &Option<Vec<SystemMessage>>,
    messages: &[Message],
) -> Vec<CacheBreakpoint> {
    // 统计 cache_control 字段的存在情况
    let tools_with_cache_control = tools
        .as_ref()
        .map(|t| t.iter().filter(|tool| tool.cache_control.is_some()).count())
        .unwrap_or(0);

    let system_with_cache_control = system
        .as_ref()
        .map(|s| s.iter().filter(|msg| msg.cache_control.is_some()).count())
        .unwrap_or(0);

    let messages_with_cache_control = messages
        .iter()
        .filter(|msg| {
            msg.content
                .as_array()
                .map(|blocks| blocks.iter().any(|b| b.get("cache_control").is_some()))
                .unwrap_or(false)
        })
        .count();

    tracing::debug!(
        "Cache control in request: tools={}/{}, system={}/{}, messages={}/{}",
        tools_with_cache_control,
        tools.as_ref().map(|t| t.len()).unwrap_or(0),
        system_with_cache_control,
        system.as_ref().map(|s| s.len()).unwrap_or(0),
        messages_with_cache_control,
        messages.len()
    );

    let mut hasher = Sha256::new();
    let mut breakpoints = Vec::new();
    let mut cumulative_tokens: i32 = 0;

    // 1. 处理 tools（按 name 排序，确保顺序稳定）
    if let Some(tools) = tools {
        let mut sorted_tools: Vec<_> = tools.iter().collect();
        sorted_tools.sort_by(|a, b| a.name.cmp(&b.name));

        for tool in sorted_tools {
            // 使用规范化的 tool 表示更新 hash
            let normalized = normalize_tool(tool);
            hasher.update(normalized.as_bytes());
            cumulative_tokens += token::count_tokens(&normalized) as i32;

            // 检查 cache_control
            if let Some(cc) = &tool.cache_control {
                let ttl = parse_ttl(cc);
                breakpoints.push(CacheBreakpoint {
                    hash: format!("{:x}", hasher.clone().finalize()),
                    tokens: cumulative_tokens,
                    ttl,
                });
            }
        }
    }

    // 2. 处理 system
    if let Some(system) = system {
        for msg in system {
            hasher.update(msg.text.as_bytes());
            cumulative_tokens += token::count_tokens(&msg.text) as i32;

            if let Some(cc) = &msg.cache_control {
                let ttl = parse_ttl(cc);
                breakpoints.push(CacheBreakpoint {
                    hash: format!("{:x}", hasher.clone().finalize()),
                    tokens: cumulative_tokens,
                    ttl,
                });
            }
        }
    }

    // 3. 处理 messages（遍历所有消息内容块）
    for msg in messages {
        if let Some(blocks) = msg.content.as_array() {
            for block in blocks {
                // 更新 hash
                let block_json = serde_json::to_string(block).unwrap_or_default();
                hasher.update(block_json.as_bytes());

                // 估算 tokens
                if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                    cumulative_tokens += token::count_tokens(text) as i32;
                }

                // 检查 cache_control
                if let Some(cc) = block.get("cache_control") {
                    if let Ok(cache_control) =
                        serde_json::from_value::<CacheControl>(cc.clone())
                    {
                        let ttl = parse_ttl(&cache_control);
                        breakpoints.push(CacheBreakpoint {
                            hash: format!("{:x}", hasher.clone().finalize()),
                            tokens: cumulative_tokens,
                            ttl,
                        });
                    }
                }
            }
        } else if let Some(text) = msg.content.as_str() {
            hasher.update(text.as_bytes());
            cumulative_tokens += token::count_tokens(text) as i32;
        }
    }

    tracing::debug!(
        "Cache breakpoints computed: count={}, tools={}, system={}, messages={}",
        breakpoints.len(),
        tools.as_ref().map(|t| t.len()).unwrap_or(0),
        system.as_ref().map(|s| s.len()).unwrap_or(0),
        messages.len()
    );

    breakpoints
}

/// 解析 TTL
fn parse_ttl(cc: &CacheControl) -> u64 {
    match cc.ttl.as_deref() {
        Some("1h") => EXTENDED_TTL_SECS,
        _ => DEFAULT_TTL_SECS,
    }
}

/// 递归排序 JSON 对象的 key
fn sort_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut sorted: BTreeMap<String, serde_json::Value> = BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), sort_json_value(v));
            }
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sort_json_value).collect())
        }
        _ => value.clone(),
    }
}

/// 递归排序 JSON 对象的 key 并序列化为字符串
fn sort_json_keys(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    let sorted = sort_json_value(value);
    serde_json::to_string(&sorted)
}

/// 规范化 Tool 为稳定的字符串表示（用于 hash 计算）
fn normalize_tool(tool: &Tool) -> String {
    // 按固定顺序拼接关键字段
    let mut parts = Vec::new();
    parts.push(format!("name:{}", tool.name));
    if !tool.description.is_empty() {
        parts.push(format!("desc:{}", tool.description));
    }
    // input_schema 使用排序后的 JSON
    if !tool.input_schema.is_empty() {
        // 将 HashMap 转换为 JSON Value 再排序
        let schema_value = serde_json::to_value(&tool.input_schema).unwrap_or_default();
        if let Ok(sorted) = sort_json_keys(&schema_value) {
            parts.push(format!("schema:{}", sorted));
        }
    }
    parts.join("|")
}

/// 查询或创建缓存
pub async fn lookup_or_create(
    api_key: &str,
    breakpoints: &[CacheBreakpoint],
    total_input_tokens: i32,
) -> CacheResult {
    // 如果 Redis 不可用或没有断点，返回默认结果
    let Some(conn) = REDIS_CONN.get() else {
        tracing::debug!("Cache lookup skipped: Redis not available");
        return CacheResult {
            uncached_input_tokens: total_input_tokens,
            ..Default::default()
        };
    };

    if breakpoints.is_empty() {
        tracing::debug!("Cache lookup skipped: no breakpoints");
        return CacheResult {
            uncached_input_tokens: total_input_tokens,
            ..Default::default()
        };
    }

    let mut conn = conn.clone();
    let mut result = CacheResult::default();

    // 从最后一个断点向前查找缓存命中
    for (i, bp) in breakpoints.iter().enumerate().rev() {
        let key = format!("cache:{}:{}", api_key, bp.hash);

        // 尝试获取缓存
        let cached: Option<i32> = conn.get(&key).await.ok().flatten();

        if let Some(cached_tokens) = cached {
            // 缓存命中
            tracing::debug!(
                "Cache hit: key={}, cached_tokens={}",
                key, cached_tokens
            );
            result.cache_read_input_tokens = cached_tokens;

            // 刷新 TTL
            let _: Result<(), _> = conn.expire(&key, bp.ttl as i64).await;

            // 计算后续断点需要创建的缓存
            for later_bp in breakpoints.iter().skip(i + 1) {
                let later_key = format!("cache:{}:{}", api_key, later_bp.hash);
                let additional_tokens = later_bp.tokens - cached_tokens;

                // 写入新缓存
                let _: Result<(), _> = conn
                    .set_ex(&later_key, later_bp.tokens, later_bp.ttl)
                    .await;

                result.cache_creation_input_tokens += additional_tokens;
            }

            break;
        } else {
            tracing::debug!("Cache miss: key={}", key);
        }
    }

    // 如果完全没有命中，创建所有断点的缓存
    if result.cache_read_input_tokens == 0 && !breakpoints.is_empty() {
        let last_bp = breakpoints.last().unwrap();
        result.cache_creation_input_tokens = last_bp.tokens;

        for bp in breakpoints {
            let key = format!("cache:{}:{}", api_key, bp.hash);
            let _: Result<(), _> = conn.set_ex(&key, bp.tokens, bp.ttl).await;
        }
    }

    // 计算未缓存的 tokens
    let cached_tokens = result.cache_read_input_tokens + result.cache_creation_input_tokens;
    result.uncached_input_tokens = (total_input_tokens - cached_tokens).max(0);

    tracing::debug!(
        "Cache result: read={}, creation={}, uncached={}",
        result.cache_read_input_tokens,
        result.cache_creation_input_tokens,
        result.uncached_input_tokens
    );

    result
}
