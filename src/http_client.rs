//! HTTP Client 构建模块
//!
//! 提供统一的 HTTP Client 构建功能，支持代理配置、代理池（Round-Robin / Random / Per-Credential）

use reqwest::{Client, Proxy};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::model::config::TlsBackend;

/// 代理配置
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ProxyConfig {
    /// 代理地址，支持 http/https/socks5
    pub url: String,
    /// 代理认证用户名
    pub username: Option<String>,
    /// 代理认证密码
    pub password: Option<String>,
}

impl ProxyConfig {
    /// 从 url 创建代理配置
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            username: None,
            password: None,
        }
    }

    /// 设置认证信息
    pub fn with_auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }
}

/// 代理池选择策略
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyStrategy {
    /// 按顺序轮询（默认）
    RoundRobin,
    /// 每次随机选择
    Random,
    /// 每个凭据固定绑定一个代理（根据凭据 ID 取模）
    PerCredential,
}

impl Default for ProxyStrategy {
    fn default() -> Self {
        Self::RoundRobin
    }
}

impl ProxyStrategy {
    pub fn from_str_lossy(value: &str) -> Self {
        let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
        match normalized.as_str() {
            "random" => Self::Random,
            "per-credential" | "percred" | "per_credential" => Self::PerCredential,
            _ => Self::RoundRobin,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RoundRobin => "round-robin",
            Self::Random => "random",
            Self::PerCredential => "per-credential",
        }
    }
}

/// 代理池
///
/// 维护一组代理，根据策略选择一个代理
/// - RoundRobin: 原子计数器递增
/// - Random: 每次随机
/// - PerCredential: 按凭据 ID 取模（无 ID 时回退到 RoundRobin）
///
/// 支持"代理限流冷却"：当某个代理被上游标记为 429/限流时，
/// 可调用 [`ProxyPool::mark_rate_limited`] 将其放入冷却期；`pick()` 会自动跳过冷却中的代理，
/// 直到所有代理都处于冷却状态时，选择最早到期的代理（避免完全不可用）。
#[derive(Debug)]
pub struct ProxyPool {
    entries: Vec<ProxyConfig>,
    strategy: ProxyStrategy,
    counter: AtomicUsize,
    /// 每个代理的冷却到期时间（毫秒 since UNIX epoch），0 表示未冷却
    cooldown_until_ms: Vec<AtomicU64>,
    /// 默认冷却时长（由 config 注入）
    default_cooldown: Duration,
}

/// 获取当前毫秒时间戳（自 UNIX 纪元）
#[inline]
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl ProxyPool {
    /// 默认冷却时长
    pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30);

    /// 从代理列表构造
    pub fn new(entries: Vec<ProxyConfig>, strategy: ProxyStrategy) -> Self {
        let cooldown_until_ms = entries.iter().map(|_| AtomicU64::new(0)).collect();
        Self {
            entries,
            strategy,
            counter: AtomicUsize::new(0),
            cooldown_until_ms,
            default_cooldown: Self::DEFAULT_COOLDOWN,
        }
    }

    /// 链式设置默认冷却时长
    pub fn with_cooldown(mut self, cooldown: Duration) -> Self {
        if !cooldown.is_zero() {
            self.default_cooldown = cooldown;
        }
        self
    }

    /// 获取默认冷却时长
    pub fn default_cooldown(&self) -> Duration {
        self.default_cooldown
    }

    /// 从端口范围模板构造（如 socks5h://host:10001-10010）
    ///
    /// # Arguments
    /// * `protocol` - 协议，例如 "socks5h" / "socks5" / "http" / "https"
    /// * `host` - 主机名
    /// * `port_start` - 起始端口（含）
    /// * `port_end` - 结束端口（含）
    /// * `username` - 可选的全局认证用户名
    /// * `password` - 可选的全局认证密码
    /// * `strategy` - 选择策略
    pub fn from_template(
        protocol: &str,
        host: &str,
        port_start: u16,
        port_end: u16,
        username: Option<&str>,
        password: Option<&str>,
        strategy: ProxyStrategy,
    ) -> Self {
        let proto = if protocol.trim().is_empty() {
            "socks5h"
        } else {
            protocol.trim()
        };
        let (lo, hi) = if port_start <= port_end {
            (port_start, port_end)
        } else {
            (port_end, port_start)
        };

        let mut entries = Vec::with_capacity((hi - lo + 1) as usize);
        for port in lo..=hi {
            let url = format!("{}://{}:{}", proto, host, port);
            let mut cfg = ProxyConfig::new(url);
            if let (Some(u), Some(p)) = (username, password) {
                cfg = cfg.with_auth(u, p);
            }
            entries.push(cfg);
        }

        Self::new(entries, strategy)
    }

    /// 池大小
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 返回所有代理条目的只读视图
    pub fn entries(&self) -> &[ProxyConfig] {
        &self.entries
    }

    /// 选择策略
    pub fn strategy(&self) -> ProxyStrategy {
        self.strategy
    }

    /// 根据策略挑选一个代理
    ///
    /// # Arguments
    /// * `credential_id` - 可选的凭据 ID（PerCredential 策略使用）
    ///
    /// 会自动跳过当前处于冷却期的代理；如果所有代理都在冷却，
    /// 返回冷却到期时间最早的代理作为兜底（避免完全不可用）。
    pub fn pick(&self, credential_id: Option<u64>) -> Option<ProxyConfig> {
        if self.entries.is_empty() {
            return None;
        }

        let now = now_ms();
        let available: Vec<usize> = (0..self.entries.len())
            .filter(|&i| self.cooldown_until_ms[i].load(Ordering::Relaxed) <= now)
            .collect();

        // 全部在冷却：选冷却到期时间最早的（避免完全不可用）
        if available.is_empty() {
            let earliest = (0..self.entries.len())
                .min_by_key(|&i| self.cooldown_until_ms[i].load(Ordering::Relaxed))?;
            tracing::warn!(
                "代理池所有代理均处于冷却中，回退选择最早到期的索引 {}",
                earliest
            );
            return self.entries.get(earliest).cloned();
        }

        let pick_from = |slot: usize| -> Option<ProxyConfig> {
            available.get(slot).and_then(|&i| self.entries.get(i).cloned())
        };

        match self.strategy {
            ProxyStrategy::RoundRobin => {
                let slot = self.counter.fetch_add(1, Ordering::Relaxed) % available.len();
                pick_from(slot)
            }
            ProxyStrategy::Random => {
                let slot = fastrand::usize(..available.len());
                pick_from(slot)
            }
            ProxyStrategy::PerCredential => match credential_id {
                Some(id) => {
                    let slot = (id as usize) % available.len();
                    pick_from(slot)
                }
                None => {
                    let slot = self.counter.fetch_add(1, Ordering::Relaxed) % available.len();
                    pick_from(slot)
                }
            },
        }
    }

    /// 将指定代理标记为限流，进入默认冷却时长
    ///
    /// 返回 true 表示找到了对应代理并完成标记，false 表示代理不属于此池
    pub fn mark_rate_limited(&self, proxy: &ProxyConfig) -> bool {
        self.mark_rate_limited_for(proxy, self.default_cooldown)
    }

    /// 将指定代理标记为限流，自定义冷却时长
    pub fn mark_rate_limited_for(&self, proxy: &ProxyConfig, cooldown: Duration) -> bool {
        if cooldown.is_zero() {
            return false;
        }
        let Some(idx) = self.entries.iter().position(|e| e == proxy) else {
            return false;
        };
        let until = now_ms().saturating_add(cooldown.as_millis() as u64);
        self.cooldown_until_ms[idx].store(until, Ordering::Relaxed);
        true
    }

    /// 清除指定代理的冷却标记
    pub fn clear_cooldown(&self, proxy: &ProxyConfig) -> bool {
        let Some(idx) = self.entries.iter().position(|e| e == proxy) else {
            return false;
        };
        self.cooldown_until_ms[idx].store(0, Ordering::Relaxed);
        true
    }

    /// 清除所有代理的冷却状态
    pub fn clear_all_cooldowns(&self) {
        for slot in &self.cooldown_until_ms {
            slot.store(0, Ordering::Relaxed);
        }
    }

    /// 获取冷却状态快照
    ///
    /// 返回每个代理当前的冷却到期时间（毫秒时间戳）
    /// - `0` 表示未冷却
    /// - 大于当前时间表示在冷却中
    /// - 小于等于当前时间表示已过期（应视为未冷却）
    pub fn cooldown_snapshot(&self) -> Vec<u64> {
        self.cooldown_until_ms
            .iter()
            .map(|a| a.load(Ordering::Relaxed))
            .collect()
    }
}

/// 构建 HTTP Client
///
/// # Arguments
/// * `proxy` - 可选的代理配置
/// * `timeout_secs` - 超时时间（秒）
///
/// # Returns
/// 配置好的 reqwest::Client
pub fn build_client(
    proxy: Option<&ProxyConfig>,
    timeout_secs: u64,
    tls_backend: TlsBackend,
) -> anyhow::Result<Client> {
    let mut builder = Client::builder().timeout(Duration::from_secs(timeout_secs));

    match tls_backend {
        TlsBackend::Rustls => {
            builder = builder.use_rustls_tls();
        }
        TlsBackend::NativeTls => {
            #[cfg(feature = "native-tls")]
            {
                builder = builder.use_native_tls();
            }
            #[cfg(not(feature = "native-tls"))]
            {
                anyhow::bail!("此构建版本未包含 native-tls 后端，请在配置中改用 rustls");
            }
        }
    }

    if let Some(proxy_config) = proxy {
        let mut proxy = Proxy::all(&proxy_config.url)?;

        // 设置代理认证
        if let (Some(username), Some(password)) = (&proxy_config.username, &proxy_config.password) {
            proxy = proxy.basic_auth(username, password);
        }

        builder = builder.proxy(proxy);
        tracing::debug!("HTTP Client 使用代理: {}", proxy_config.url);
    }

    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_config_new() {
        let config = ProxyConfig::new("http://127.0.0.1:7890");
        assert_eq!(config.url, "http://127.0.0.1:7890");
        assert!(config.username.is_none());
        assert!(config.password.is_none());
    }

    #[test]
    fn test_proxy_config_with_auth() {
        let config = ProxyConfig::new("socks5://127.0.0.1:1080").with_auth("user", "pass");
        assert_eq!(config.url, "socks5://127.0.0.1:1080");
        assert_eq!(config.username, Some("user".to_string()));
        assert_eq!(config.password, Some("pass".to_string()));
    }

    #[test]
    fn test_build_client_without_proxy() {
        let client = build_client(None, 30, TlsBackend::Rustls);
        assert!(client.is_ok());
    }

    #[test]
    fn test_build_client_with_proxy() {
        let config = ProxyConfig::new("http://127.0.0.1:7890");
        let client = build_client(Some(&config), 30, TlsBackend::Rustls);
        assert!(client.is_ok());
    }
}
