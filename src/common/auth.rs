//! 公共认证工具函数

use axum::{
    body::Body,
    http::{Request, header},
};
use subtle::ConstantTimeEq;

/// 从请求中提取 API Key
///
/// 支持两种认证方式：
/// - `x-api-key` header
/// - `Authorization: Bearer <token>` header
pub fn extract_api_key(request: &Request<Body>) -> Option<String> {
    // 优先检查 x-api-key
    if let Some(key) = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
    {
        return Some(key.to_string());
    }

    // 其次检查 Authorization: Bearer
    request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

/// 常量时间字符串比较，防止时序攻击
///
/// 无论字符串内容如何，比较所需的时间都是恒定的，
/// 这可以防止攻击者通过测量响应时间来猜测 API Key。
///
/// 使用经过安全审计的 `subtle` crate 实现
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

/// 解析 API Key，提取凭证 ID（如果有）
///
/// 认证逻辑：
/// 1. 如果 `api_key` 完全等于 `base_api_key` → 返回 `None`（使用系统配置的策略）
/// 2. 如果 `api_key` 以 `{base_api_key}-` 开头 → 尝试解析后缀为凭证 ID
///    - 解析成功 → 返回 `Some(credential_id)`
///    - 解析失败 → 返回 `None`（后续认证会失败）
/// 3. 其他情况 → 返回 `None`（后续认证会失败）
///
/// # 参数
/// * `api_key` - 完整的 API key 字符串
/// * `base_api_key` - 配置的基础 API key
///
/// # 返回
/// * `Some(credential_id)` - 指定使用凭证 ID
/// * `None` - 使用系统配置的策略或认证失败
///
/// # 示例
/// ```
/// use crate::common::auth::parse_credential_id;
///
/// // 完全匹配，使用系统配置的策略
/// let id = parse_credential_id("sk-ant-1234567890", "sk-ant-1234567890");
/// assert_eq!(id, None);
///
/// // 指定凭证 ID
/// let id = parse_credential_id("sk-ant-1234567890-5", "sk-ant-1234567890");
/// assert_eq!(id, Some(5));
///
/// // 前缀不匹配，返回 None（后续认证会失败）
/// let id = parse_credential_id("sk-wrong-key-5", "sk-ant-1234567890");
/// assert_eq!(id, None);
/// ```
pub fn parse_credential_id(api_key: &str, base_api_key: &str) -> Option<u64> {
    // 1. 先检查是否完全匹配（使用系统配置的策略）
    if api_key == base_api_key {
        return None;
    }

    // 2. 检查是否以 {base_api_key}- 开头
    let prefix = format!("{}-", base_api_key);
    if let Some(suffix) = api_key.strip_prefix(&prefix) {
        // 尝试将后缀解析为 u64
        if let Ok(credential_id) = suffix.parse::<u64>() {
            return Some(credential_id);
        }
    }

    // 3. 其他情况返回 None（后续认证会失败）
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_API_KEY: &str = "sk-ant-1234567890";

    #[test]
    fn test_parse_credential_id_exact_match() {
        // 完全匹配，使用系统配置的策略
        let id = parse_credential_id(BASE_API_KEY, BASE_API_KEY);
        assert_eq!(id, None);
    }

    #[test]
    fn test_parse_credential_id_with_id() {
        // 指定凭证 ID
        let id = parse_credential_id("sk-ant-1234567890-5", BASE_API_KEY);
        assert_eq!(id, Some(5));
    }

    #[test]
    fn test_parse_credential_id_with_large_id() {
        // 大凭证 ID（不限位数）
        let id = parse_credential_id("sk-ant-1234567890-123456", BASE_API_KEY);
        assert_eq!(id, Some(123456));
    }

    #[test]
    fn test_parse_credential_id_with_very_large_id() {
        // 非常大的凭证 ID
        let id = parse_credential_id("sk-ant-1234567890-9999999999", BASE_API_KEY);
        assert_eq!(id, Some(9999999999));
    }

    #[test]
    fn test_parse_credential_id_non_numeric_suffix() {
        // 后缀不是数字，返回 None（后续认证会失败）
        let id = parse_credential_id("sk-ant-1234567890-abc", BASE_API_KEY);
        assert_eq!(id, None);
    }

    #[test]
    fn test_parse_credential_id_wrong_prefix() {
        // 前缀不匹配，返回 None（后续认证会失败）
        let id = parse_credential_id("sk-wrong-key-5", BASE_API_KEY);
        assert_eq!(id, None);
    }

    #[test]
    fn test_parse_credential_id_empty_suffix() {
        // 空后缀（以 - 结尾），返回 None
        let id = parse_credential_id("sk-ant-1234567890-", BASE_API_KEY);
        assert_eq!(id, None);
    }

    #[test]
    fn test_parse_credential_id_no_dash() {
        // 完全不同的 key，返回 None
        let id = parse_credential_id("completely-different-key", BASE_API_KEY);
        assert_eq!(id, None);
    }

    #[test]
    fn test_parse_credential_id_partial_match() {
        // 部分匹配但不是完整前缀，返回 None
        let id = parse_credential_id("sk-ant-123", BASE_API_KEY);
        assert_eq!(id, None);
    }

    #[test]
    fn test_parse_credential_id_zero() {
        // 凭证 ID 为 0
        let id = parse_credential_id("sk-ant-1234567890-0", BASE_API_KEY);
        assert_eq!(id, Some(0));
    }
}
