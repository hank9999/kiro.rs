//! Thinking 标签处理：标签搜索、引号过滤、完整文本提取
//!
//! 模型偶尔会在自然语言里提到 `<thinking>` / `</thinking>`（被反引号、引号包裹），
//! 本模块的 `find_real_thinking_*` 系列负责区分"真正的标签"与"被引用的字符串"，
//! 是 stream.rs 增量处理状态机和 handlers 非流式响应的共用基础。

/// 找到小于等于目标位置的最近有效 UTF-8 字符边界
///
/// UTF-8 字符可能占 1-4 个字节，直接按字节位置切片可能会切在多字节字符中间导致 panic。
/// 这个函数从目标位置向前搜索，找到最近的有效字符边界。
pub(crate) fn find_char_boundary(s: &str, target: usize) -> usize {
    if target >= s.len() {
        return s.len();
    }
    if target == 0 {
        return 0;
    }
    let mut pos = target;
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// 需要跳过的包裹字符
///
/// 当 thinking 标签被这些字符包裹时，认为是在引用标签而非真正的标签：
/// - 反引号 (`)：行内代码
/// - 双引号 (")：字符串
/// - 单引号 (')：字符串
const QUOTE_CHARS: &[u8] = b"`\"'\\#!@$%^&*()-_=+[]{};:<>,.?/";

/// 检查指定位置的字符是否是引用字符
fn is_quote_char(buffer: &str, pos: usize) -> bool {
    buffer
        .as_bytes()
        .get(pos)
        .map(|c| QUOTE_CHARS.contains(c))
        .unwrap_or(false)
}

/// 查找真正的 thinking 结束标签（不被引用字符包裹，且后面有双换行符）
///
/// 当模型在思考过程中提到 `</thinking>` 时，通常会用反引号、引号等包裹，
/// 或者在同一行有其他内容（如"关于 </thinking> 标签"）。
/// 这个函数会跳过这些情况，只返回真正的结束标签位置。
///
/// 跳过的情况：
/// - 被引用字符包裹（反引号、引号等）
/// - 后面没有双换行符（真正的结束标签后面会有 `\n\n`）
/// - 标签在缓冲区末尾（流式处理时需要等待更多内容）
///
/// # 参数
/// - `buffer`: 要搜索的字符串
///
/// # 返回值
/// - `Some(pos)`: 真正的结束标签的起始位置
/// - `None`: 没有找到真正的结束标签
pub(crate) fn find_real_thinking_end_tag(buffer: &str) -> Option<usize> {
    const TAG: &str = "</thinking>";
    let mut search_start = 0;

    while let Some(pos) = buffer[search_start..].find(TAG) {
        let absolute_pos = search_start + pos;

        let has_quote_before = absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1);
        let after_pos = absolute_pos + TAG.len();
        let has_quote_after = is_quote_char(buffer, after_pos);

        if has_quote_before || has_quote_after {
            search_start = absolute_pos + 1;
            continue;
        }

        let after_content = &buffer[after_pos..];

        if after_content.len() < 2 {
            return None;
        }

        if after_content.starts_with("\n\n") {
            return Some(absolute_pos);
        }

        search_start = absolute_pos + 1;
    }

    None
}

/// 查找缓冲区末尾的 thinking 结束标签（允许末尾只有空白字符）
///
/// 用于"边界事件"场景：例如 thinking 结束后立刻进入 tool_use，或流结束，
/// 此时 `</thinking>` 后面可能没有 `\n\n`，但结束标签依然应被识别并过滤。
///
/// 约束：只有当 `</thinking>` 之后全部都是空白字符时才认为是结束标签，
/// 以避免在 thinking 内容中提到 `</thinking>`（非结束标签）时误判。
pub(crate) fn find_real_thinking_end_tag_at_buffer_end(buffer: &str) -> Option<usize> {
    const TAG: &str = "</thinking>";
    let mut search_start = 0;

    while let Some(pos) = buffer[search_start..].find(TAG) {
        let absolute_pos = search_start + pos;

        let has_quote_before = absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1);
        let after_pos = absolute_pos + TAG.len();
        let has_quote_after = is_quote_char(buffer, after_pos);

        if has_quote_before || has_quote_after {
            search_start = absolute_pos + 1;
            continue;
        }

        if buffer[after_pos..].trim().is_empty() {
            return Some(absolute_pos);
        }

        search_start = absolute_pos + 1;
    }

    None
}

/// 查找真正的 thinking 开始标签（不被引用字符包裹）
///
/// 与 `find_real_thinking_end_tag` 类似，跳过被引用字符包裹的开始标签。
pub(crate) fn find_real_thinking_start_tag(buffer: &str) -> Option<usize> {
    const TAG: &str = "<thinking>";
    let mut search_start = 0;

    while let Some(pos) = buffer[search_start..].find(TAG) {
        let absolute_pos = search_start + pos;

        let has_quote_before = absolute_pos > 0 && is_quote_char(buffer, absolute_pos - 1);
        let after_pos = absolute_pos + TAG.len();
        let has_quote_after = is_quote_char(buffer, after_pos);

        if !has_quote_before && !has_quote_after {
            return Some(absolute_pos);
        }

        search_start = absolute_pos + 1;
    }

    None
}

/// 从完整文本中提取 thinking 块（用于非流式响应）
///
/// 使用与流式处理相同的标签检测逻辑（引用字符过滤），确保一致性。
/// 非流式场景下文本已完整，无需处理跨 chunk 分割问题。
///
/// # 返回值
/// - `(Some(thinking_content), remaining_text)` — 检测到有效 thinking 块
/// - `(None, original_text)` — 未检测到，原样返回
pub(crate) fn extract_thinking_from_complete_text(text: &str) -> (Option<String>, String) {
    let start_pos = match find_real_thinking_start_tag(text) {
        Some(pos) => pos,
        None => return (None, text.to_string()),
    };

    let before = &text[..start_pos];
    let after_open = &text[start_pos + "<thinking>".len()..];

    // 查找结束标签：优先匹配带 \n\n 后缀的，退而使用末尾匹配
    let (thinking_raw, text_after) =
        if let Some(end_pos) = find_real_thinking_end_tag(after_open) {
            (
                &after_open[..end_pos],
                &after_open[end_pos + "</thinking>\n\n".len()..],
            )
        } else if let Some(end_pos) = find_real_thinking_end_tag_at_buffer_end(after_open) {
            let after_tag = end_pos + "</thinking>".len();
            (
                &after_open[..end_pos],
                after_open[after_tag..].trim_start(),
            )
        } else {
            return (None, text.to_string());
        };

    // 剥离开头的换行符（与流式处理一致：模型输出 <thinking>\n）
    let thinking_content = thinking_raw
        .strip_prefix('\n')
        .unwrap_or(thinking_raw);

    let mut remaining = String::new();
    if !before.trim().is_empty() {
        remaining.push_str(before);
    }
    remaining.push_str(text_after);

    if thinking_content.is_empty() {
        (None, remaining)
    } else {
        (Some(thinking_content.to_string()), remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_simple_thinking_block_with_double_newline() {
        // extract_thinking_from_complete_text 要求 </thinking> 后有 \n\n
        let text = "<thinking>plan</thinking>\n\nanswer";
        let (thinking, rest) = extract_thinking_from_complete_text(text);
        assert_eq!(thinking.as_deref(), Some("plan"));
        assert_eq!(rest, "answer");
    }

    #[test]
    fn extract_thinking_at_buffer_end_without_double_newline() {
        // 退而使用末尾匹配（buffer_end）：</thinking> 后只有空白
        let text = "<thinking>plan</thinking>";
        let (thinking, rest) = extract_thinking_from_complete_text(text);
        assert_eq!(thinking.as_deref(), Some("plan"));
        assert_eq!(rest, "");
    }

    #[test]
    fn extract_no_thinking_returns_full_text() {
        let text = "just an answer";
        let (thinking, rest) = extract_thinking_from_complete_text(text);
        assert!(thinking.is_none());
        assert_eq!(rest, "just an answer");
    }

    #[test]
    fn extract_thinking_strips_leading_newline_in_content() {
        // <thinking>\n 的换行应被剥离
        let text = "<thinking>\nplan</thinking>\n\nanswer";
        let (thinking, rest) = extract_thinking_from_complete_text(text);
        assert_eq!(thinking.as_deref(), Some("plan"));
        assert_eq!(rest, "answer");
    }

    #[test]
    fn extract_thinking_skips_quoted_start_tag() {
        // 反引号包裹的 <thinking> 不应被识别
        let text = "use `<thinking>` tag for thinking";
        let (thinking, rest) = extract_thinking_from_complete_text(text);
        assert!(thinking.is_none());
        assert_eq!(rest, text);
    }

    #[test]
    fn extract_thinking_preserves_nonempty_before_content() {
        // <thinking> 之前有非空白前缀内容时，前缀应保留在 remaining 中
        let text = "preamble<thinking>plan</thinking>\n\nanswer";
        let (thinking, rest) = extract_thinking_from_complete_text(text);
        assert_eq!(thinking.as_deref(), Some("plan"));
        assert_eq!(rest, "preambleanswer");
    }

    // --- 标签搜索函数级单测（从 stream.rs 迁移） ---

    #[test]
    fn test_find_real_thinking_start_tag_basic() {
        assert_eq!(find_real_thinking_start_tag("<thinking>"), Some(0));
        assert_eq!(find_real_thinking_start_tag("prefix<thinking>"), Some(6));
    }

    #[test]
    fn test_find_real_thinking_start_tag_with_backticks() {
        assert_eq!(find_real_thinking_start_tag("`<thinking>`"), None);
        assert_eq!(find_real_thinking_start_tag("use `<thinking>` tag"), None);

        assert_eq!(
            find_real_thinking_start_tag("about `<thinking>` tag<thinking>content"),
            Some(22)
        );
    }

    #[test]
    fn test_find_real_thinking_start_tag_with_quotes() {
        assert_eq!(find_real_thinking_start_tag("\"<thinking>\""), None);
        assert_eq!(find_real_thinking_start_tag("the \"<thinking>\" tag"), None);

        assert_eq!(find_real_thinking_start_tag("'<thinking>'"), None);

        assert_eq!(
            find_real_thinking_start_tag("about \"<thinking>\" and '<thinking>' then<thinking>"),
            Some(40)
        );
    }

    #[test]
    fn test_find_real_thinking_start_tag_at_buffer_boundary() {
        // 标签跨 chunk 时（buffer 不含完整 <thinking>）应返回 None
        assert_eq!(find_real_thinking_start_tag("prefix<thin"), None);
        assert_eq!(find_real_thinking_start_tag("prefix<thinking"), None);
    }

    #[test]
    fn test_find_real_thinking_end_tag_basic() {
        assert_eq!(find_real_thinking_end_tag("</thinking>\n\n"), Some(0));
        assert_eq!(
            find_real_thinking_end_tag("content</thinking>\n\n"),
            Some(7)
        );
        assert_eq!(
            find_real_thinking_end_tag("some text</thinking>\n\nmore text"),
            Some(9)
        );

        assert_eq!(find_real_thinking_end_tag("</thinking>"), None);
        assert_eq!(find_real_thinking_end_tag("</thinking>\n"), None);
        assert_eq!(find_real_thinking_end_tag("</thinking> more"), None);
    }

    #[test]
    fn test_find_real_thinking_end_tag_with_backticks() {
        assert_eq!(find_real_thinking_end_tag("`</thinking>`\n\n"), None);
        assert_eq!(
            find_real_thinking_end_tag("mention `</thinking>` in code\n\n"),
            None
        );

        assert_eq!(find_real_thinking_end_tag("`</thinking>\n\n"), None);
        assert_eq!(find_real_thinking_end_tag("</thinking>`\n\n"), None);
    }

    #[test]
    fn test_find_real_thinking_end_tag_with_quotes() {
        assert_eq!(find_real_thinking_end_tag("\"</thinking>\"\n\n"), None);
        assert_eq!(
            find_real_thinking_end_tag("the string \"</thinking>\" is a tag\n\n"),
            None
        );

        assert_eq!(find_real_thinking_end_tag("'</thinking>'\n\n"), None);
        assert_eq!(
            find_real_thinking_end_tag("use '</thinking>' as marker\n\n"),
            None
        );

        assert_eq!(
            find_real_thinking_end_tag("about \"</thinking>\" tag</thinking>\n\n"),
            Some(23)
        );

        assert_eq!(
            find_real_thinking_end_tag("about '</thinking>' tag</thinking>\n\n"),
            Some(23)
        );
    }

    #[test]
    fn test_find_real_thinking_end_tag_mixed() {
        assert_eq!(
            find_real_thinking_end_tag("discussing `</thinking>` tag</thinking>\n\n"),
            Some(28)
        );

        assert_eq!(
            find_real_thinking_end_tag("`</thinking>` and `</thinking>` done</thinking>\n\n"),
            Some(36)
        );

        assert_eq!(
            find_real_thinking_end_tag(
                "`</thinking>` and \"</thinking>\" and '</thinking>' done</thinking>\n\n"
            ),
            Some(54)
        );
    }

    #[test]
    fn test_find_real_thinking_end_tag_at_buffer_end_basic() {
        // </thinking> 后只有空白才认定为结束标签
        assert_eq!(
            find_real_thinking_end_tag_at_buffer_end("abc</thinking>"),
            Some(3)
        );
        assert_eq!(
            find_real_thinking_end_tag_at_buffer_end("abc</thinking>\n"),
            Some(3)
        );
        assert_eq!(
            find_real_thinking_end_tag_at_buffer_end("abc</thinking>  \t"),
            Some(3)
        );

        // 后面有非空白内容则不识别
        assert_eq!(
            find_real_thinking_end_tag_at_buffer_end("abc</thinking>more"),
            None
        );
    }

    #[test]
    fn test_find_char_boundary_handles_multibyte() {
        let s = "你好world";
        // "你" 占 3 字节，"好" 占 3 字节
        assert_eq!(find_char_boundary(s, 0), 0);
        assert_eq!(find_char_boundary(s, 3), 3); // "你" 边界
        assert_eq!(find_char_boundary(s, 6), 6); // "好" 边界
        // 切在多字节中间应回退到边界
        assert_eq!(find_char_boundary(s, 1), 0);
        assert_eq!(find_char_boundary(s, 4), 3);
        // 越界应返回长度
        assert_eq!(find_char_boundary(s, 100), s.len());
    }
}
