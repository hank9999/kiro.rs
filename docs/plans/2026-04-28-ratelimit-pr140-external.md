# Rust LLM 网关限流外部调研

> 调研日期：2026-04-28
> 调研范围：Rust 异步生态中 LLM API 反向代理场景的上游 429 处理方案

---

## 场景定位

本项目（kiro.rs）是**被动响应型**限流场景，与主动保护型有本质区别：

| 维度 | 主动保护（自己服务） | 被动响应（上游 429） |
|------|---------------------|---------------------|
| 触发方 | 自己在下游客户端触发 | 上游 API 返回 429 |
| 决策时机 | 请求进入前预判 | 请求已发出后处理响应 |
| 状态来源 | 自己维护的计数器 | 上游响应头（Retry-After 等） |
| 典型工具 | governor / tower-governor | cooldown 标记 + backoff |

kiro.rs 的限流需求分两层：
1. **账号冷却**：上游返回 429 后，将该账号从可用池中短暂摘除（被动响应核心）
2. **下游防滥用**（可选）：防止单个客户端耗尽账号池，属于主动保护

---

## 上游 429 响应格式（事实，有来源）

### Anthropic

**HTTP 响应头**（来源：[Anthropic Rate Limits 官方文档](https://platform.claude.com/docs/en/api/rate-limits)）：

| 头字段 | 格式 | 说明 |
|--------|------|------|
| `retry-after` | 整数秒（如 `30`） | 服务端计算的最短等待秒数，优先级最高 |
| `anthropic-ratelimit-requests-limit` | 整数 | 当前周期最大请求数 |
| `anthropic-ratelimit-requests-remaining` | 整数 | 本周期剩余请求数 |
| `anthropic-ratelimit-requests-reset` | RFC 3339 时间戳 | 请求限额完全恢复的时刻 |
| `anthropic-ratelimit-tokens-limit` | 整数 | 最大 token 数（最严格限制） |
| `anthropic-ratelimit-tokens-remaining` | 整数（舍入到千） | 剩余 token 数 |
| `anthropic-ratelimit-tokens-reset` | RFC 3339 时间戳 | token 限额恢复时刻 |
| `anthropic-ratelimit-input-tokens-*` | 同上规律 | 输入 token 细分 |
| `anthropic-ratelimit-output-tokens-*` | 同上规律 | 输出 token 细分 |

**JSON body**（来源：[Anthropic Errors 官方文档](https://platform.claude.com/docs/en/api/errors)）：

```json
{
  "type": "error",
  "error": {
    "type": "rate_limit_error",
    "message": "Your account has hit a rate limit."
  },
  "request_id": "req_011CSHoEeqs5C35K2UUqR7Fy"
}
```

Anthropic 使用**令牌桶算法**（token bucket），速率持续补充而非固定窗口重置。`retry-after` 为整数秒，无 HTTP-date 格式变体（当前行为）。

### OpenAI

**HTTP 响应头**（来源：[OpenAI Rate Limits 文档](https://developers.openai.com/api/docs/guides/rate-limits)）：

| 头字段 | 格式 | 示例 |
|--------|------|------|
| `x-ratelimit-limit-requests` | 整数 | `60` |
| `x-ratelimit-limit-tokens` | 整数 | `150000` |
| `x-ratelimit-remaining-requests` | 整数 | `59` |
| `x-ratelimit-remaining-tokens` | 整数 | `149984` |
| `x-ratelimit-reset-requests` | **duration 字符串** | `1s`、`6m0s`、`4.253s` |
| `x-ratelimit-reset-tokens` | **duration 字符串** | `57.357s`、`8ms` |

注意：`x-ratelimit-reset-*` 使用 Go 风格 duration 字符串（非秒整数、非 RFC 3339），解析时需要专门处理。OpenAI 429 不一定携带标准 `Retry-After` 头。

**JSON body**（来源：社区实录与 [OpenAI Error Codes 文档](https://developers.openai.com/api/docs/guides/error-codes)）：

```json
{
  "error": {
    "message": "Rate limit exceeded. Retry in 6s.",
    "type": "rate_limit",
    "param": null,
    "code": "rate_limit_exceeded"
  }
}
```

配额耗尽时 type 为 `insufficient_quota`，需与 RPM/TPM 型 429 区分处理。

### Codex（GitHub Copilot / OpenAI Codex）

公开文档极少。推断（非事实）与 OpenAI 同格式，因其后端走 OpenAI 基础设施。实际行为需实测确认。

---

## Crate 选型对比

| crate | 适用场景 | 是否适合本项目 | 理由 |
|-------|---------|---------------|------|
| **governor** | 主动限流：输出速率控制、GCRA 令牌桶 | 部分适合 | 适合做下游客户端防滥用；不适合被动响应上游 429 |
| **tower-governor** | Axum/Tower 中间件：按 IP/Key 限制入站请求 | 部分适合 | 保护自己服务，非处理上游错误 |
| **backoff** | 指数退避重试，支持解析 Retry-After 头 | 适合 | 专门设计用于处理 429 等可重试错误，支持 async |
| **tokio-retry2** | Tokio 异步重试策略 | 适合 | 轻量，与 Tokio 深度集成 |
| **自研 cooldown 标记** | 账号池摘除/恢复 | 最适合 | 最精确地反映「账号不可用到指定时刻」语义 |

**结论**：

- **账号冷却**：不需要 governor 或 tower-governor。用 `tokio::time::Instant` + `DashMap<AccountId, Instant>` 实现 cooldown_until 标记即可，成本最低，语义最清晰。
- **下游防滥用**：tower-governor 适合，但属于可选功能（YAGNI 原则下暂不引入）。
- **重试逻辑**：backoff crate 可用，但 kiro.rs 的账号池场景是「换账号」而非「等待重试」，更倾向快速 failover 而非等待。

---

## 推荐策略

### 核心：精确 cooldown 而非盲目退避

被动响应 429 的最优策略是**精确按 Retry-After 冷却**，而非固定时长或指数退避：

```
收到上游 429 → 读取 retry-after 头
  ├─ 有 retry-after → cooldown_until = now() + retry_after_secs
  ├─ 无 retry-after，有 reset 时间戳 → cooldown_until = parse(reset_timestamp)
  └─ 都没有 → cooldown_until = now() + 指数退避（初始 60s，上限 1800s）
将该账号标记为 cooldown，从可用池摘除
下次请求到来时，过滤所有 cooldown_until > now() 的账号
cooldown 到期后，账号自动进入候选池（无需主动解禁）
```

### 指数退避的必要性

当所有账号都被限流（账号池耗尽）时才需要对外侧客户端做退避，防止雪崩：

- 初始等待：60 秒（与 Anthropic 常见 Retry-After 对齐）
- 倍增因子：2
- 最大等待：1800 秒（30 分钟）
- 加入 ±20% 随机抖动防止 thundering herd

### 区分 429 类型

Anthropic 的 429 可能来自不同限额层：
- RPM（请求/分钟）→ Retry-After 通常较短（几秒到 1 分钟）
- ITPM（输入 token/分钟）→ 等待时长取决于令牌消耗
- 月度配额耗尽 → 该账号应长期摘除（而非短暂冷却）

可通过 `anthropic-ratelimit-*-reset` 头的时间跨度来推断：reset 时间在今天之内 → RPM/TPM 类型；reset 在下月 → 月度配额耗尽，该账号需特殊标记。

### 不推荐令牌桶（governor）用于此场景

governor 的 GCRA 设计用于**预测并阻止**超限，需要事先知道上游的确切速率上限。在 kiro.rs 的多账号场景下：
1. 每个账号的速率上限因 Tier 不同而不同
2. 账号共享 token 池（上游有 RPM + ITPM + OTPM 三维限制）
3. 实时剩余额度才是真相，而非本地估算

因此 governor 在此场景下是**过度工程**，且会引入不准确的预测性限流。

---

## 业界案例

### LiteLLM Router

来源：[LiteLLM Routing 文档](https://docs.litellm.ai/docs/routing) + [Prometheus Metrics 文档](https://docs.litellm.ai/docs/proxy/prometheus)

核心策略：
- `cooldown_time`：默认 **5 秒**（`DEFAULT_COOLDOWN_TIME_SECONDS`），收到 429 后**立即**触发
- `allowed_fails`：默认 **3 次**失败后触发冷却（可配置为 1 次）
- 冷却期间该 deployment 从路由候选中摘除，其他健康 deployment 继续服务
- 支持按 deployment 粒度隔离故障，不影响同模型组其他账号
- **不支持动态 cooldown**（不读取 Retry-After 头调整时长）——这是已知缺陷

Prometheus 关键指标：
- `litellm_deployment_cooled_down`：deployment 被冷却次数（Counter）
- `litellm_deployment_state`：deployment 健康状态（0=健康、1=部分故障、2=完全故障）
- `litellm_remaining_requests_metric`：从上游响应头读取的剩余请求数
- `litellm_remaining_tokens_metric`：从上游响应头读取的剩余 token 数

LiteLLM 的一个已知问题（[Issue #7669](https://github.com/BerriAI/litellm/issues/7669)）：usage-based-routing-v2 路由模式下不尊重 `retry_after`，导致立即失败而非等待。说明业界实现也有此难点。

### OpenRouter

来源：[OpenRouter Rate Limits 文档](https://openrouter.ai/docs/api/reference/limits)

OpenRouter 对付费模型不设硬性速率限制，依赖上游提供商的限制。在账号池/模型切换策略上，OpenRouter 通过**模型级路由**实现类似效果：在 credit 不足或特定模型限流时，自动降级到备用模型。不直接公开账号池轮询的实现细节。

### 业界冷却退避模式（来自 OpenClaw 等开源网关）

搜索结果中提到的一个开源 LLM 网关（OpenClaw）采用的指数冷却模式：
- 1 分钟 → 5 分钟 → 25 分钟 → 1 小时
- 冷却期间 deployment 从可用池摘除，到期自动恢复
- 这是在没有可靠 Retry-After 头的情况下的保守策略

---

## 引用

- [Anthropic Rate Limits 官方文档](https://platform.claude.com/docs/en/api/rate-limits) — `retry-after` 整数秒格式、`anthropic-ratelimit-*-reset` RFC 3339 格式、令牌桶算法说明
- [Anthropic Errors 官方文档](https://platform.claude.com/docs/en/api/errors) — 429 JSON body 结构：`type: "rate_limit_error"`
- [OpenAI Rate Limits 文档](https://developers.openai.com/api/docs/guides/rate-limits) — `x-ratelimit-reset-*` duration 字符串格式（`6m0s`、`4.253s`）
- [LiteLLM Routing 文档](https://docs.litellm.ai/docs/routing) — `cooldown_time` 默认 5s、`allowed_fails` 默认 3、429 触发即时冷却
- [LiteLLM Prometheus 文档](https://docs.litellm.ai/docs/proxy/prometheus) — `litellm_deployment_cooled_down`、`litellm_deployment_state` 指标
- [LiteLLM Issue #7669](https://github.com/BerriAI/litellm/issues/7669) — usage-based-routing 不尊重 retry_after 的已知缺陷
- [governor crate 文档](https://docs.rs/governor) — GCRA 算法、适用于主动限流场景
- [tower-governor crate](https://docs.rs/tower_governor) — Tower 中间件，适用于下游防滥用
- [backoff crate](https://docs.rs/backoff) — 指数退避，支持解析 Retry-After 头
- [OpenRouter Rate Limits 文档](https://openrouter.ai/docs/api/reference/limits) — OpenRouter 模型级路由策略

---

## 置信度评估

- **Anthropic 响应格式**：高（来自官方文档，字段名称和格式均有明确记录）
- **OpenAI 响应格式**：中高（duration 字符串格式来自官方文档，JSON body 来自社区实录）
- **Codex 响应格式**：低（无公开文档，系推断）
- **LiteLLM 策略**：高（来自官方文档 + GitHub issue 交叉验证）
- **governor 不适合被动场景**：高（基于 crate 设计文档的逻辑推导）
