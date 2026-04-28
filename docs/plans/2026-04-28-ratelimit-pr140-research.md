# kiro.rs ratelimit 调研报告（基于 PR #140）

> 生成日期：2026-04-28
> 分支：`refactor/v2`（Hexagonal 重构 Phase A-F 已完成）
> 本文是三份并行研究产物的融合报告，读者无需阅读原始产物即可理解全貌。

---

## 1. 背景与目标

kiro.rs 是面向多账号 Kiro/Anthropic API 的反向代理网关，当前处于 `refactor/v2` 分支（Phase F 已完成）。用户希望参考上游 master 分支的 PR #140（`feat: 增加凭据限流功能`）在 refactor/v2 中实现类似的限流能力。

**本文目标**：梳理 PR #140 的设计、refactor/v2 的现状缺口、外部最佳实践，输出决策所需的对比信息，不涉及具体实现计划。

### 限流需求的两个层次

三份研究均一致指出限流需求实际是两个相互独立的问题 [PR #140][现状][外部]：

| 层次 | 描述 | 优先级 |
|------|------|--------|
| **P0：账号冷却**（被动响应） | 上游返回 429 后将该账号临时从可用池摘除，按 Retry-After 冷却 | 核心缺口 |
| **P1：下游防滥用**（主动保护） | 防止单一下游客户端过量调用耗尽账号池，返回 429 给下游 | 可选功能 |

这两个需求的技术方案、集成层、工具选型完全不同，不能混为一谈。

---

## 2. PR #140 设计速览

PR #140 由两个 commit 组成（`7b6c2d1`、`8f33544`），净增约 1058 行，改动 25 个文件。

### 2.1 核心算法

PR #140 在 `MultiTokenManager`（旧 master 的"账号管理器"）内以**纯内存滑动窗口**实现限流：

- 数据结构：每条凭据维护一个 `VecDeque<Instant>` 存储历史请求时间戳
- 检查时机：`acquire_context`（获取 API 上下文）阶段，即在发出上游 HTTP 请求**之前**
- 触发源：**本地计数**，非上游 429 响应信号（预测式，非响应式）
- 超限行为：自动轮转到其他未超限凭据；所有凭据均超限时 `sleep_until` 等待最早恢复点后重试

### 2.2 规则模型

```
凭据级规则（rateLimits）若存在 → 完全覆盖全局默认（effective_rate_limit_rules 不合并）
全局默认（defaultRateLimits）   → 凭据无自定义规则时生效
```

- 规则格式：`{ window: "5m", maxRequests: 100 }`，支持 s/m/h/d 单位，最大 30 天
- 支持多条规则同时配置（"且"逻辑，任一超限即触发）
- Admin API 支持运行时读写，持久化到 config.json / credentials.json
- **运行态不持久化**：`VecDeque<Instant>` 仅存内存，重启计数清零

### 2.3 Admin 可观测性

`CredentialStatusItem` 新增字段：`rate_limited`（布尔）、`next_available_at`（RFC3339）、`rate_limit_summaries`（含 `remaining_requests` 的窗口摘要）。Admin UI 凭据卡片直接展示。PR 无新增结构化日志（tracing/log），可观测性依赖 Admin API 轮询。

### 2.4 关键可疑点 [PR #140]

1. **计入限流的请求范围**：PR 描述声称"仅 `/generateAssistantResponse` 和 `/mcp` 计入"，但 patch 内未见明确过滤代码，可能通过 `reserve_rate_limit_slot` 的调用位置隐式控制，需验证。
2. **全部超限时的等待策略**：`WaitUntil` 分支使用 `sleep_until` 阻塞当前 Tokio task，若等待时间较长（数小时）且并发高，可能大量 task 积压，PR 中无超时上限或队列限制。
3. **并发安全**：`check_rate_limit` 与 `reserve_rate_limit_slot` 之间存在 check-then-act 窗口，高并发下可能超限。PR 提到单锁，但具体锁范围未明。
4. **覆盖语义副作用**：凭据配置了任意 `rateLimits` 后，全局 `defaultRateLimits` 完全被忽略（非合并）。若全局配置了日限额，凭据级只配置分钟限额则日限额静默失效，PR 未讨论。

---

## 3. refactor/v2 现状与缺口

### 3.1 架构层级（由外到内）

```
interface/http/anthropic/handlers.rs  ← HTTP 入口
  └─ service/conversation/            ← 编排层
       └─ service/kiro_client.rs      ← KiroClient 门面
            └─ infra/http/executor.rs ← RequestExecutor（发请求 + 退避）
                 ├─ infra/http/retry.rs       ← DefaultRetryPolicy
                 └─ service/credential_pool/  ← CredentialPool（账号管理）
                      ├─ pool.rs     ← 门面（acquire / report_*）
                      ├─ state.rs    ← 运行时状态（失败计数 + 禁用 + 自愈）
                      └─ stats.rs    ← 统计（success_count）
```

### 3.2 当前 429 处理行为 [现状]

- `src/infra/http/retry.rs:67-71`：`408 | 429` 及所有 5xx → `RetryDecision::Retry { backoff }`，即**退避后原地重试，不切换凭据，不禁用凭据**
- `src/infra/http/retry.rs:25-33`：指数退避，基准 200ms × 2^attempt，上限 2000ms，加 ±25% jitter
- 常量：`MAX_RETRIES_PER_CREDENTIAL = 3`，`MAX_TOTAL_RETRIES = 9`

当前的 429 处理**不读取 `Retry-After` 头**，退避时长完全基于本地计数，忽略了上游的精确恢复时间信息。

### 3.3 已存在的状态机 [现状]

`src/service/credential_pool/state.rs` 中 `EntryState` 的禁用原因：

| 原因 | 触发条件 | 自愈 |
|------|---------|------|
| `TooManyFailures` | API 调用失败累积 3 次 | 是 |
| `QuotaExceeded` | `report_quota_exhausted()` 调用 | 否 |
| `TooManyRefreshFailures` | refresh 失败累积 3 次 | 否 |

**缺口**：`EntryState` 无 `cooldown_until: Option<Instant>` 字段；`CredentialView`（`src/domain/selector.rs:5`）只携带 `disabled` 和 `success_count`，无法在 `select_one()` 阶段跳过"被限流但未 disabled"的账号。

### 3.4 一个潜在 regression [现状]

`RefreshError::RateLimited`（`src/domain/error.rs:13-14`）在 `pool.rs:259` 走 `report_refresh_failure()` 分支，累积 3 次后**禁用账号为 `TooManyRefreshFailures`（不自愈）**。这与 API 层 429 的"不禁用、仅退避"策略不一致——refresh 429 属于暂时性限流，不应等同于确定性失败而被永久禁用。

### 3.5 PR #140 无法直接 cherry-pick 的原因 [现状][PR #140]

refactor/v2 经历了 Phase D-F 的 Hexagonal 架构重构，旧 master 的文件树已大幅变化：

| PR #140 改动的文件 | refactor/v2 现状 |
|-------------------|-----------------|
| `src/kiro/token_manager.rs` | 不存在，职责分散到 `credential_pool/` 各子模块 |
| `src/kiro/provider.rs` | 不存在，对应 `service/kiro_client.rs` + `executor.rs` |
| `src/model/rate_limit.rs`（新增） | 不存在，可整体引入但需适配 |
| `src/admin/types.rs` | 存在但已重构，字段兼容性未知 |
| `src/admin/router.rs` | 存在，路由结构需验证是否兼容 |

---

## 4. 外部最佳实践速览

### 4.1 Anthropic 429 响应格式 [外部]

| 头字段 | 格式 | 说明 |
|--------|------|------|
| `retry-after` | 整数秒 | 最短等待秒数，**优先读取** |
| `anthropic-ratelimit-requests-reset` | RFC 3339 | 请求限额完全恢复时刻 |
| `anthropic-ratelimit-tokens-reset` | RFC 3339 | token 限额恢复时刻 |

Anthropic 使用令牌桶算法（持续补充，非固定窗口重置）。`retry-after` 为整数秒（无 HTTP-date 格式变体）。

**重要区分**：Anthropic 429 可能来自不同限额层——RPM（恢复快，几秒到 1 分钟）、ITPM（取决于 token 消耗）、月度配额耗尽（reset 在下月，需长期摘除账号而非短暂冷却）。

### 4.2 Crate 选型结论 [外部]

| crate | 场景适用性 |
|-------|---------|
| `governor` / `tower-governor` | 主动限流（保护自己），不适合被动响应上游 429 |
| 自研 `cooldown_until` 标记 | 账号冷却场景最适合，语义清晰，成本最低 |
| `backoff` | 重试退避，可辅助解析 Retry-After 头 |

外部研究明确：governor 的 GCRA 设计需要事先知道上游的确切速率上限，而 kiro.rs 的多账号、多 Tier、多维度限制场景（RPM + ITPM + OTPM）使得本地估算不准确，governor 在此场景属于过度工程。

### 4.3 业界案例 [外部]

**LiteLLM Router**：收到 429 立即触发冷却（默认 5s），deployment 从路由候选摘除，其他健康账号继续服务。已知缺陷：不读取 `retry-after` 调整时长（Issue #7669），存在已知改进空间。

**OpenClaw 指数冷却模式**：1 分钟 → 5 分钟 → 25 分钟 → 1 小时，无 Retry-After 头时的保守策略。

---

## 5. 关键决策点对比

### 决策 1：响应式 vs 预测式拦截

| | PR #140 | refactor/v2 可选方向 | 外部共识 |
|--|---------|---------------------|---------|
| 触发源 | 本地计数（预测式，前置拦截） | 上游 429 响应（响应式，被动冷却） | 响应式（多数网关案例） |
| 准确性 | 估算，可能提前或延迟触发 | 精确，基于真实上游信号 | 真实信号更可靠 |
| 实现复杂度 | 高（需维护滑动窗口计数） | 低（读响应头 + 打标记） | — |
| 适用场景 | 已知精确速率上限 | 速率上限未知/因账号 Tier 不同 | — |

**冲突点**：PR #140 的预测式方案在多 Tier 账号混合场景下，限流阈值需手工配置且可能失准；外部研究和现状代码分析均指向响应式方案更符合 kiro.rs 场景。

### 决策 2：限流粒度

| 粒度 | PR #140 | 现状已有 | 外部实践 |
|------|---------|---------|---------|
| 凭据（credential）级 | 是（主要粒度） | `EntryState` 已有凭据级状态 | LiteLLM deployment 级 |
| 下游客户端（IP/Key）级 | 否 | 无 | tower-governor 可实现 |
| 全局 | 是（`defaultRateLimits`） | 无 | — |

PR #140 的凭据级粒度与 refactor/v2 的 `CredentialPool` 模型天然对齐，是优先考虑的粒度。下游客户端级限流目前两侧均无实现，属于独立功能。

### 决策 3：算法选型

| 算法 | 描述 | PR #140 | 现状 | 外部评价 |
|------|------|---------|------|---------|
| 滑动窗口（`VecDeque<Instant>`） | 精确，内存随并发线性增长 | 是 | 否 | 适合账号级计数 |
| `cooldown_until`（简单时间戳） | 极简，语义精确，O(1) | 否 | 无 | 外部首选（被动场景） |
| 令牌桶（governor/GCRA） | 平滑流量，需预知速率 | 否 | 否 | 不适合被动 429 处理 |
| 指数退避（现有） | 仅退避不摘出 | 否 | 已有（无 Retry-After 解析） | 改进空间：读 Retry-After 头 |

**对比要点**：`cooldown_until` 时间戳方案与 PR #140 的滑动窗口方案根本差异在于触发源——前者从上游响应获取恢复时间，后者自行计数预测。在 kiro.rs 场景，外部研究和现状分析均倾向前者。

### 决策 4：集成层

| 集成层 | 描述 | PR #140 | refactor/v2 候选 | 影响范围 |
|--------|------|---------|-----------------|---------|
| `acquire_context` 阶段（账号选择前） | 前置拦截 | 是（`MultiTokenManager`） | `credential_pool/pool.rs::acquire()` | 改动账号选择逻辑 |
| `retry.rs`（响应处理层） | 解析 Retry-After，决策退避时长 | 否（现有） | `DefaultRetryPolicy::decide()` | 改动最小，低风险 |
| `state.rs`（凭据状态层） | 新增 `cooldown_until` 字段，过滤冷却账号 | 否 | `EntryState` + `CredentialView` | 中等改动 |
| axum middleware 层 | 下游防滥用 | 否 | `interface/http/anthropic/middleware.rs` | 独立，不影响账号逻辑 |

**现状代码推荐分层** [现状]：
1. **低风险**：`retry.rs` 解析 `Retry-After` 头，精准退避
2. **中等**：`state.rs` 加 `cooldown_until`，`select_one()` 过滤冷却账号
3. **按需**：middleware 层加 `tower-governor`，下游防滥用

### 决策 5：是否引入 `governor` crate

| 方向 | 适用场景 | 代价 |
|------|---------|------|
| 引入 `governor` | 主动限流（下游防滥用） | 需理解 GCRA 模型，增加依赖 |
| 引入 `tower-governor` | Axum 中间件按 IP/Key 限制入站 | 中等集成成本 |
| 不引入（自研 cooldown） | 账号冷却（被动响应） | 实现简单，语义清晰，无新依赖 |

外部研究结论：对于 P0 账号冷却需求，不需要引入任何专用限流 crate，用 `tokio::time::Instant` + 现有状态字段即可。`governor` 仅在 P1 下游防滥用场景有价值。

---

## 6. 与 refactor/v2 集成的具体冲突

| 冲突点 | 描述 | 风险等级 |
|--------|------|---------|
| `MultiTokenManager` 不存在 | PR #140 核心改动文件在 refactor/v2 中不存在 | 高，无法直接套用 |
| `RefreshError::RateLimited` 处理不一致 | refresh 429 被当普通 refresh 失败处理，可能导致账号被永久禁用 | 中，已有 regression |
| `CredentialView` 缺少 `cooldown_until` | 选择器无法在 `select_one()` 阶段过滤冷却账号 | 中，是功能缺口 |
| 锁顺序约束 | 新增冷却状态必须遵守 `store → state → stats` 锁顺序，若放入 `state` 层则无需新锁 | 低（有清晰约束可遵循） |
| Admin `types.rs` 兼容性 | PR #140 新增的 `CredentialStatusItem` 字段是否与 refactor/v2 版本兼容未知 | 中，需验证 |
| `Retry-After` 头未读取 | 现有退避使用本地指数退避，忽略上游精确恢复时间 | 低（不 regression，但有改进空间） |

---

## 7. 推荐方案概要（方向，非实现计划）

综合三份研究，建议采取**响应式冷却**而非预测式滑动窗口：

1. **P0a（最小侵入）**：在 `DefaultRetryPolicy::decide()` 中解析 `Retry-After` 响应头，将其传递给 `RetryDecision::Retry { backoff }`。当前使用固定指数退避，改为读取上游精确时间，改动范围仅 `retry.rs`，风险最低。

2. **P0b（核心功能）**：在 `EntryState` 加 `cooldown_until: Option<Instant>` 字段；在 `CredentialView` 加同名字段；在 `executor.rs` 收到 429 后调 `pool.mark_cooldown(id, duration)` 而非 `report_failure`（现在 429 不调 `report_failure`，新增语义安全）；在 `select_one()` 的 snapshot 后过滤 `cooldown_until > now()` 的账号。此方案与 PR #140 的"凭据轮换"语义等效，但基于真实上游信号，更符合多 Tier 场景。

3. **P0c（修复 regression）**：区分 `RefreshError::RateLimited` 与其他 refresh 失败，前者应触发短暂冷却而非累积禁用计数。

4. **P1（按需，不耦合 P0）**：若需下游防滥用，在 axum middleware 层引入 `tower-governor`，与账号逻辑完全解耦。

**不推荐照搬 PR #140 的预测式滑动窗口方案**，理由：①触发源为本地估算，在多 Tier 账号场景需手工维护阈值；②在 refactor/v2 中无对应架构（`MultiTokenManager` 不存在），移植成本高；③外部实践（LiteLLM/OpenClaw）均倾向响应式冷却。

---

## 8. 待澄清问题（用户决策点）

1. **优先级**：P0a/P0b/P0c 是否并行推进，还是分批？P0a（retry.rs 改进）改动最小，可以先行。
2. **下游防滥用**：是否需要按客户端 IP 或 API key 限制下游请求数（P1）？当前两侧均无实现，属于新功能。
3. **月度配额耗尽处理**：Anthropic 月度配额耗尽的 429 与 RPM 429 行为应该相同（短暂冷却）还是触发长期禁用（类似现有 `QuotaExceeded`）？
4. **冷却持久化**：重启后是否需要恢复 cooldown 状态？PR #140 选择不持久化（运行态仅内存）；如需持久化，则 `Instant` 需改为 `SystemTime` 或 `chrono::DateTime`。
5. **可观测性要求**：是否需要暴露 Admin API 中凭据的冷却状态（类似 PR #140 的 `rate_limited`、`next_available_at` 字段）？还是仅内部处理？
6. **预测式限流保留价值**：是否有"主动防止超限"的需求（即不等上游 429，自己计数提前拦截）？若有，则 PR #140 的滑动窗口设计在 P0b 之外作为 P2 单独考虑。
7. **Codex 429 格式**：Codex 端点的 429 响应头格式未知（低置信度，需实测），是否需要提前覆盖？

---

## 9. 引用

| 来源 | 文档路径 | 关注点 |
|------|---------|--------|
| PR #140 分析 | `docs/plans/2026-04-28-ratelimit-pr140-pr-analysis.md` | 算法设计、Admin API、配置结构 |
| 代码现状 | `docs/plans/2026-04-28-ratelimit-pr140-codebase.md` | refactor/v2 架构、缺口、锁顺序约束 |
| 外部调研 | `docs/plans/2026-04-28-ratelimit-pr140-external.md` | Anthropic/OpenAI 429 格式、Crate 选型、LiteLLM 案例 |

**关键外部链接（来自外部调研）**：
- [Anthropic Rate Limits 官方文档](https://platform.claude.com/docs/en/api/rate-limits) — `retry-after` 整数秒格式
- [Anthropic Errors 官方文档](https://platform.claude.com/docs/en/api/errors) — 429 JSON body 结构
- [LiteLLM Routing 文档](https://docs.litellm.ai/docs/routing) — 业界冷却策略参考
- [LiteLLM Issue #7669](https://github.com/BerriAI/litellm/issues/7669) — 不尊重 retry_after 的已知缺陷

**关键代码位置（来自现状分析）**：
- `src/infra/http/retry.rs:67-71` — 429 当前处理（退避不切换不禁用）
- `src/service/credential_pool/state.rs:85-91` — `report_quota_exhausted`（立即禁用）
- `src/service/credential_pool/pool.rs:259` — `RefreshError::RateLimited` 走 `report_refresh_failure`（潜在 regression）
- `src/domain/selector.rs:5` — `CredentialView` 缺少 `cooldown_until` 字段
- `src/infra/http/executor.rs:258-267` — `RetryAfter(d)` 分支（sleep 但不报告凭据状态）
