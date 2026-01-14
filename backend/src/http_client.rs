//! HTTP Client 构建模块
//!
//! Input: 代理 URL
//! Output: reqwest::Client
//! Pos: HTTP 客户端构建，支持代理

use reqwest::{Client, Proxy};
use std::time::Duration;

/// 构建 HTTP Client
///
/// # Arguments
/// * `proxy_url` - 可选的代理 URL，支持格式:
///   - http://host:port
///   - http://user:pass@host:port
///   - socks5://host:port
///   - socks5://user:pass@host:port
/// * `timeout_secs` - 超时时间（秒）
///
/// # Returns
/// 配置好的 reqwest::Client
pub fn build_client(proxy_url: Option<&str>, timeout_secs: u64) -> anyhow::Result<Client> {
    let mut builder = Client::builder().timeout(Duration::from_secs(timeout_secs));

    if let Some(url) = proxy_url {
        let proxy = Proxy::all(url)?;
        builder = builder.proxy(proxy);
        tracing::debug!("HTTP Client 使用代理: {}", url);
    }

    Ok(builder.build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_client_without_proxy() {
        let client = build_client(None, 30);
        assert!(client.is_ok());
    }

    #[test]
    fn test_build_client_with_http_proxy() {
        let client = build_client(Some("http://127.0.0.1:7890"), 30);
        assert!(client.is_ok());
    }

    #[test]
    fn test_build_client_with_socks5_proxy() {
        let client = build_client(Some("socks5://127.0.0.1:1080"), 30);
        assert!(client.is_ok());
    }

    #[test]
    fn test_build_client_with_auth_proxy() {
        let client = build_client(Some("http://user:pass@127.0.0.1:7890"), 30);
        assert!(client.is_ok());
    }
}
