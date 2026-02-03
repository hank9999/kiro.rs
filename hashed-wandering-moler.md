# Anthropic Messages 请求在 Kiro 端的行为分析

## 概述

当前端收到一个 Anthropic `/v1/messages` 请求时，kiro-rs 会将其转换为 Kiro API 格式并发送到 AWS Kiro 服务。

---

## 完整请求流程

### 1. 入口点
**文件:** `src/anthropic/router.rs:53`
```
POST /v1/messages -> post_messages()
```

### 2. 请求转换 (Anthropic → Kiro)

**文件:** `src/anthropic/converter.rs`

#### 模型映射
| Anthropic 模型 | Kiro 模型 |
|---------------|-----------|
| `*sonnet*` | `claude-sonnet-4.5` |
| `*opus*` | `claude-opus-4.5` |
| `*haiku*` | `claude-haiku-4.5` |

#### 请求结构转换

**Anthropic 请求格式:**
```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 1024,
  "messages": [
    {"role": "user", "content": "Hello"},
    {"role": "assistant", "content": "Hi!"},
    {"role": "user", "content": "How are you?"}
  ],
  "system": "You are helpful",
  "tools": [...],
  "stream": true
}
```

**转换后的 Kiro 请求格式:**
```json
{
  "conversationState": {
    "conversationId": "uuid-from-metadata-or-generated",
    "agentTaskType": "vibe",
    "chatTriggerType": "MANUAL",
    "currentMessage": {
      "userInputMessage": {
        "content": "How are you?",
        "modelId": "claude-sonnet-4.5",
        "origin": "AI_EDITOR",
        "userInputMessageContext": {
          "tools": [...],
          "toolResults": [...]
        }
      }
    },
    "history": [
      {
        "userInputMessage": {
          "content": "[System: You are helpful]\n\nHello",
          "modelId": "claude-sonnet-4.5"
        }
      },
      {
        "assistantResponseMessage": {
          "content": "Hi!"
        }
      }
    ]
  }
}
```

### 3. 发送到 Kiro API

**文件:** `src/kiro/provider.rs`

**API 端点:**
```
POST https://q.{region}.amazonaws.com/generateAssistantResponse
```

**请求头:**
```
Content-Type: application/json
Authorization: Bearer {oauth_token}
Host: q.{region}.amazonaws.com
x-amzn-kiro-agent-mode: vibe
x-amzn-codewhisperer-optout: true
x-amz-user-agent: aws-sdk-js/1.0.27 KiroIDE-{version}-{machine_id}
User-Agent: aws-sdk-js/1.0.27 ua/2.1 os/{os} lang/js md/nodejs#{node_version} ...
amz-sdk-invocation-id: {uuid}
amz-sdk-request: attempt=1; max=3
Connection: close
```

### 4. 重试与故障转移机制

**文件:** `src/kiro/provider.rs:351-535`

- 每个凭据最多重试 3 次
- 总重试上限 9 次
- 支持多凭据按优先级故障转移
- 错误处理:
  - 400/401/402/403: 切换凭据
  - 429: 指数退避重试
  - 5xx: 重试

### 5. 响应流处理

**文件:** `src/kiro/parser/decoder.rs`

Kiro 返回 **AWS Event Stream** 格式，包含以下事件类型:
- `assistantResponseEvent` - 文本内容
- `toolUseEvent` - 工具调用
- `contextUsageEvent` - Token 使用统计
- `meteringEvent` - 计费信息

### 6. 响应转换 (Kiro → Anthropic SSE)

**文件:** `src/anthropic/stream.rs`

| Kiro 事件 | Anthropic SSE 事件 |
|----------|-------------------|
| `assistantResponseEvent` | `content_block_delta` (text_delta) |
| `toolUseEvent` | `content_block_start` + `content_block_delta` |
| `contextUsageEvent` | 内部 token 计数 |
| Exception | `stop_reason: max_tokens` |

---

## 关键转换细节

### System Prompt 处理
System prompt 被注入到第一条用户消息的前面:
```
[System: {system_content}]

{first_user_message}
```

### Tool 定义转换
Anthropic 的 `input_schema` 转换为 Kiro 的 `inputSchema`，结构基本一致。

### Tool Result 处理
当消息包含 `tool_result` 类型的 content block 时，会被提取到 `userInputMessageContext.toolResults` 中。

### 会话 ID
- 优先从 `metadata.user_id` 提取
- 否则生成新的 UUID

---

## 流程图

```
┌─────────────────────────────────────────────────────────────────┐
│                    Anthropic API Request                        │
│  POST /v1/messages                                              │
│  {model, messages, system, tools, stream}                       │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                    converter.rs                                  │
│  - 模型名映射 (sonnet→claude-sonnet-4.5)                         │
│  - System prompt 注入到首条消息                                   │
│  - 构建 history 数组                                             │
│  - 转换 tools 定义                                               │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                    provider.rs                                   │
│  POST https://q.{region}.amazonaws.com/generateAssistantResponse │
│  - OAuth Bearer Token 认证                                       │
│  - 伪装为 KiroIDE 客户端                                         │
│  - 多凭据故障转移                                                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                    AWS Event Stream Response                     │
│  - assistantResponseEvent (文本)                                 │
│  - toolUseEvent (工具调用)                                       │
│  - contextUsageEvent (token统计)                                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│                    stream.rs                                     │
│  转换为 Anthropic SSE 格式:                                      │
│  - message_start                                                 │
│  - content_block_start/delta/stop                                │
│  - message_delta (usage)                                         │
│  - message_stop                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

---

## History 构建逻辑

**文件:** `src/anthropic/converter.rs:427-519`

### 转换规则

Anthropic 的 `messages` 数组转换为 Kiro 的 `history` + `currentMessage`:

```
Anthropic messages:
[user1, assistant1, user2, assistant2, user3]
                                         ↑
                                    currentMessage

Kiro 结构:
history: [user1, assistant1, user2, assistant2]
currentMessage: user3
```

### 详细步骤

#### 1. System Prompt 处理
System prompt 被转换为 **user + assistant 配对**:

```json
// Anthropic
{ "system": [{"type": "text", "text": "You are helpful"}] }

// Kiro history (插入到最前面)
[
  { "userInputMessage": { "content": "You are helpful", ... } },
  { "assistantResponseMessage": { "content": "I will follow these instructions." } }
]
```

#### 2. Thinking 配置注入
如果启用了 thinking，会在 system prompt 前注入 thinking 标签:
```
<thinking_mode>extended</thinking_mode>
<thinking_budget>10000</thinking_budget>
You are helpful
```

#### 3. 消息配对规则

Kiro API 要求 **user 和 assistant 消息必须成对出现**:

| 场景 | 处理方式 |
|-----|---------|
| 连续多个 user 消息 | 合并为一个 user 消息 |
| 孤立的 user 消息 (末尾) | 自动配对 `"OK"` 的 assistant 响应 |
| assistant 只有 tool_use | content 设为 `" "` (空格占位符) |

#### 4. Assistant 消息转换

```json
// Anthropic assistant 消息
{
  "role": "assistant",
  "content": [
    {"type": "thinking", "thinking": "Let me think..."},
    {"type": "text", "text": "Hello!"},
    {"type": "tool_use", "id": "xxx", "name": "read_file", "input": {...}}
  ]
}

// Kiro assistant 消息
{
  "assistantResponseMessage": {
    "content": "<thinking>Let me think...</thinking>\n\nHello!",
    "toolUses": [
      {"toolUseId": "xxx", "name": "read_file", "input": {...}}
    ]
  }
}
```

#### 5. User 消息转换

```json
// Anthropic user 消息
{
  "role": "user",
  "content": [
    {"type": "text", "text": "Read this file"},
    {"type": "tool_result", "tool_use_id": "xxx", "content": "file content..."}
  ]
}

// Kiro user 消息
{
  "userInputMessage": {
    "content": "Read this file",
    "userInputMessageContext": {
      "toolResults": [
        {"toolUseId": "xxx", "output": {"message": "file content..."}}
      ]
    }
  }
}
```

### 完整转换示例

**Anthropic 请求:**
```json
{
  "model": "claude-sonnet-4-20250514",
  "system": [{"type": "text", "text": "You are a coding assistant"}],
  "messages": [
    {"role": "user", "content": "Hello"},
    {"role": "assistant", "content": "Hi! How can I help?"},
    {"role": "user", "content": "Write a function"}
  ]
}
```

**Kiro 请求:**
```json
{
  "conversationState": {
    "conversationId": "uuid",
    "agentTaskType": "vibe",
    "chatTriggerType": "MANUAL",
    "history": [
      {"userInputMessage": {"content": "You are a coding assistant", "modelId": "claude-sonnet-4.5"}},
      {"assistantResponseMessage": {"content": "I will follow these instructions."}},
      {"userInputMessage": {"content": "Hello", "modelId": "claude-sonnet-4.5"}},
      {"assistantResponseMessage": {"content": "Hi! How can I help?"}}
    ],
    "currentMessage": {
      "userInputMessage": {
        "content": "Write a function",
        "modelId": "claude-sonnet-4.5",
        "origin": "AI_EDITOR"
      }
    }
  }
}
```

---

## 会话 (Conversation) 机制

### 一个请求是否对应一个 Kiro 会话？

**不一定。** 会话 ID (`conversationId`) 的生成逻辑如下:

**文件:** `src/anthropic/converter.rs:133-138`

```rust
let conversation_id = req
    .metadata
    .as_ref()
    .and_then(|m| m.user_id.as_ref())
    .and_then(|user_id| extract_session_id(user_id))
    .unwrap_or_else(|| Uuid::new_v4().to_string());
```

### 会话 ID 来源优先级:

1. **从 `metadata.user_id` 提取** (如果存在)
   - 格式: `user_xxx_session_{uuid}`
   - 例如: `user_0dede55c...session_8bb5523b-ec7c-4540-a9ca-beb6d79f1552`
   - 提取出 `8bb5523b-ec7c-4540-a9ca-beb6d79f1552` 作为 `conversationId`

2. **生成新的 UUID** (如果没有 metadata 或无法提取)
   - 每次请求都会生成新的会话 ID

### 实际行为:

| 场景 | 会话行为 |
|-----|---------|
| 客户端传递相同的 `metadata.user_id` | **同一个 Kiro 会话** |
| 客户端不传 metadata 或每次不同 | **每次请求都是新会话** |
| Claude Code 等客户端 | 通常会传递 session ID，保持会话连续性 |

### Kiro 服务端对 conversationId 的处理

根据代码分析，Kiro 服务端会:

1. **在响应中回传 conversationId**
   - `assistantResponseEvent` 响应中包含 `conversationId`、`messageId`、`messageStatus` 等字段
   - 见 `src/kiro/model/events/assistant.rs:79-88` 的测试用例

2. **可能的用途** (基于代码推断):
   - **计费/配额追踪**: 按会话统计 token 使用量
   - **日志/审计**: 关联同一会话的多次请求
   - **服务端状态**: 可能缓存某些会话级别的状态

3. **kiro-rs 的处理**:
   - 响应中的 `conversationId` 被捕获到 `extra` 字段中，**未被使用**
   - 只提取 `content` 字段用于流式输出

### 注意事项:

- `agent_continuation_id` 每次请求都会生成新的 UUID
- 历史消息 (`history`) 是由客户端在请求中传递的，不是 Kiro 服务端维护的
- kiro-rs 是无状态代理，不维护会话上下文

---

## 总结

kiro-rs 作为代理服务，将 Anthropic API 请求转换为 Kiro (AWS CodeWhisperer/Q) API 格式。主要行为:

1. **协议转换**: Anthropic 消息格式 → Kiro ConversationState 格式
2. **身份伪装**: 使用 KiroIDE 的 User-Agent 和请求头
3. **OAuth 认证**: 使用 refresh token 获取 access token
4. **流式响应**: AWS Event Stream → Anthropic SSE
5. **故障转移**: 支持多凭据自动切换
6. **会话管理**: 从 metadata.user_id 提取 session UUID，或生成新 UUID
