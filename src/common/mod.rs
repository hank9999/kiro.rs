//! 公共工具模块

pub mod auth;

/// 安全地截断 UTF-8 字符串，确保不会在多字节字符中间截断
/// 
/// 返回不超过 `max_bytes` 字节的最长有效 UTF-8 子串
pub fn truncate_str_safe(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    
    // 从 max_bytes 位置向前查找有效的字符边界
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    
    &s[..end]
}

/// 安全地截断字符串并添加省略号后缀
pub fn truncate_with_ellipsis(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    
    // 为省略号预留空间
    let truncate_at = if max_bytes > 3 { max_bytes - 3 } else { max_bytes };
    let truncated = truncate_str_safe(s, truncate_at);
    format!("{}...", truncated)
}
