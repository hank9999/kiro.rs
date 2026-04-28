# kiro.rs (refactor/v2) 限流相关代码现状

## 当前分支与架构概览

**分支**：`refactor/v2`（Hexagonal 重构 Phase D-F 已完成，Phase A-F 对应 Plan Phase 1-6）

**架构层级（由外到内）**：
```
interface/http/anthropic/handlers.rs   ← HTTP 入口（axum handler）
  └─ service/conversation/             ← Conversation 编排层（delivery / converter / reducer）
       └─ service/kiro_client.rs       ← KiroClient 门面
            └─ infra/http/executor.rs  ← RequestExecutor（发请求 + 退避 + 故障转移）
                 ├─ infra/http/retry.rs        ← DefaultRetryPolicy（状态码 → RetryDecision）
                 └─ service/credential_pool/   ← CredentialPool（账号管理）
                      ├─ pool.rs        ← 门面（acquire / report_* / force_refresh）
                      ├─ state.rs       ← 运行时状态（失败计数 + 禁用 + 自愈）
                      ├─ store.rs       ← 凭据存储
                      └─ stats.rs       ← 统计（success_count）
```

---

## 已存在的相关代码

### 1. 上游错误处理

**DefaultRetryPolicy（核心策略）**

- `src/infra/http/retry.rs:67-71` — 关键逻辑：`408 | 429` 以及所有 5xx 均映射为 `RetryDecision::Retry { backoff }`，即**退避后原地重试，不切换凭据，不禁用凭据**。
- `src/infra/http/retry.rs:25-33` — `next_backoff(attempt)` 实现指数退避：基准 200ms × 2^attempt，上限 2000ms，加 ±25% jitter。
- `src/infra/http/retry.rs:11-13` — 常量：`MAX_RETRIES_PER_CREDENTIAL = 3`，`MAX_TOTAL_RETRIES = 9`。

**RefreshError::RateLimited（刷新时的 429）**

- `src/domain/error.rs:13-14` — `RefreshError` 枚举包含 `RateLimited` 变体。
- `src/infra/refresher/mod.rs:27-39` — `classify_refresh_http_error()` 纯函数，把刷新端点的 HTTP 429 映射到 `RefreshError::RateLimited`。
- **gap**：`RefreshError::RateLimited` 在 `CredentialPool::acquire()` 中走 `_ => state.report_refresh_failure(id)` 分支（`src/service/credential_pool/pool.rs:259`），与其他 refresh 失败一视同仁，累积到 `MAX_FAILURES_PER_CREDENTIAL(=3)` 后禁用凭据。这与 API 层的 429 处理（退避不禁用）不一致——refresh 429 可能是暂时性限流，不应等同于确定性失败。

**RequestExecutor 中的 RetryAfter 分支**

- `src/infra/http/executor.rs:55` — `AttemptOutcome::RetryAfter(Duration)`
- `src/infra/http/executor.rs:258-267` — 收到 `RetryAfter(d)` 时：记录 `last_error`，`sleep(d)` 后继续循环，但**不报告到凭据状态**（既不 `report_failure`，也不 `report_success`）——凭据被"借而未还"，下次 acquire 仍可获取到该凭据。

**FatalKiroError（SSE 流中的上游 Error 事件）**

- `src/service/conversation/error.rs:29-33` — `FatalKiroError::UpstreamError { error_code, error_message }` 承载上游 SSE `event: error` 中的内容。
- `src/service/conversation/error.rs:99` — `is_fatal_exception()` 判断：`ThrottlingException`（上游限流异常）被认定为 fatal，会中止 SSE 流并向客户端发送 error 事件。
- `src/interface/http/anthropic/handlers.rs:938-961`（测试）— 验证 `RateLimited` error_code 可正确透传给客户端。

### 2. 账号选择 / 轮询

- `src/service/credential_pool/pool.rs:209-267` — `CredentialPool::acquire()` 主循环：`select_one()` → `prepare_token()` → 返回 `CallContext`。失败时 `report_refresh_failure` 后继续循环，直到 `AllCredentialsExhausted`。
- `src/service/credential_pool/pool.rs:272-340` — `select_one()` 实现：priority 模式下优先复用 `current_id`（黏性，同一凭据连续服务）；balanced 模式走 `BalancedSelector`（least-used by success_count）。
- `src/infra/selector/priority.rs:14-28` — `PrioritySelector.select()`：按 `(priority, id)` 升序取最小，纯同步计算。
- `src/infra/selector/balanced.rs:14-31` — `BalancedSelector.select()`：按 `(success_count, priority)` 升序取最小，纯同步计算。
- **gap**：两个 Selector 的 `CredentialView` 只携带 `disabled` 和 `success_count` 两个状态字段（`src/domain/selector.rs:5`），**没有 `cooldown_until` 字段**，无法在选择阶段跳过"被限流但未 disabled 的"账号。

### 3. 凭据状态与禁用逻辑

- `src/service/credential_pool/state.rs:12` — `MAX_FAILURES_PER_CREDENTIAL = 3`（共用于 API 失败和 refresh 失败计数阈值）。
- `src/service/credential_pool/state.rs:73-82` — `report_failure(id)`：API 调用失败计数，累计 3 次后禁用为 `TooManyFailures`（会参与自愈）。
- `src/service/credential_pool/state.rs:85-91` — `report_quota_exhausted(id)`：立即禁用为 `QuotaExceeded`（不参与自愈）。
- `src/service/credential_pool/state.rs:97-106` — `report_refresh_failure(id)`：refresh 失败计数，累计 3 次后禁用为 `TooManyRefreshFailures`（不参与自愈）。
- `src/service/credential_pool/state.rs:138-151` — `heal_too_many_failures()`：仅自愈 `TooManyFailures`，不自愈 `QuotaExceeded / InvalidRefreshToken / TooManyRefreshFailures`。
- **gap**：`EntryState` 无 `cooldown_until: Option<Instant>` 字段；429 只会退避等待，不能把账号临时摘出选择池。

### 4. 重试逻辑

- `src/domain/retry.rs:22-31` — `RetryDecision` 枚举：`Retry { backoff }` / `FailoverCredential` / `ForceRefresh` / `DisableCredential(DisabledReason)` / `Fail(ProviderError)` / `Success`。**没有** `DisableReason::RateLimited` 变体或"临时冷却"分支。
- `src/infra/http/executor.rs:62-81` — `compute_attempt_outcome()`：`ForceRefresh` 对已刷新过的凭据降级为 `FailoverContinue`，避免无限 refresh。
- `src/infra/http/executor.rs:137` — `for attempt in 0..max_retries`：同一请求的所有重试（含跨凭据）共用一个计数器；`max_retries = (total_creds * 3).min(9)`。

---

## 已存在的依赖（Cargo.toml）

与限流/速率控制相关的 crate：**无**。

Cargo.toml 中不含 `governor`、`tower-governor`、`tower-limit`、`leaky_bucket`、`ratelimit` 等任何专用限流库。现有退避完全由 `fastrand`（jitter）+ `tokio::time::sleep` 手工实现。

---

## 限流应该放在哪一层？

根据代码现状，限流需求实际上是两个问题：
1. **对下游客户端限流**（按 IP / API key 控制入流量）
2. **对上游账号处理 429**（上游限流后的"账号冷却"）

### 候选 A：HTTP handler 入口（`interface/http/anthropic/`）

适合：按客户端 IP/key 做入口限流，拒绝过量请求（返回 429 给下游）。集成点在 axum middleware 层（`src/interface/http/anthropic/middleware.rs`），不改动 CredentialPool。实现需引入 `governor` / `tower-governor`。

### 候选 B：`infra/http/retry.rs`（DefaultRetryPolicy）

适合：在已有的 `decide()` 方法中为 429 返回一个"携带 Retry-After 时长"的 `RetryDecision` 变体。目前 429 已正确退避，但退避时长来自指数退避，**忽略了上游响应头中的 `Retry-After`**。增强点：解析 `Retry-After` header 并在 `RetryDecision::Retry { backoff }` 中使用它。

### 候选 C：`service/credential_pool/state.rs`（EntryState + DisabledReason）

适合：把被上游限流的账号临时冷却（`cooldown_until: Option<Instant>`），在 `select_one()` 的 view 过滤阶段跳过冷却中的账号，而不像现在一样继续把它拉入重试循环。需要在 `CredentialStateView` 和 `EntryState` 中新增 `cooldown_until` 字段，并在 `DisabledReason` 或单独路径中处理"临时不可用"逻辑。

### 推荐

**分层实施**，优先级从高到低：

1. **候选 B（立即可做，低风险）**：在 `DefaultRetryPolicy::decide()` 中解析 `Retry-After` 响应头，使退避时长更精准。不需要改 CredentialPool。
2. **候选 C（中等复杂度）**：在 `EntryState` 加 `cooldown_until: Option<Instant>`，在 `select_one()` 过滤时跳过冷却账号，并在 executor 收到 429 后调 `pool.mark_cooldown(id, duration)` 替代 `report_failure`（现在 429 不调 report_failure，不产生副作用，可安全新增）。这是与 PR #140（旧 master 的"429 不禁用凭据"修复）等效的正确架构表达。
3. **候选 A（按需）**：如需对下游客户端限流，在 middleware 层加 `tower-governor`，不影响账号选择逻辑。

---

## 与 PR #140 的关系

**未直接合并**。`refactor/v2` 是从 master 于 2026-04-25 分出的，PR #140 相关的 master commit（`eec4f69 fix: 429 不应该禁用或切换凭据`，2026-01-07）已在分叉点之前合入 master，但该 commit 修改的是旧 `src/kiro/provider.rs`（monolith 架构），**不在 refactor/v2 的文件树中**。

refactor/v2 对 429 的处理是在重构中**重新实现**的，对应逻辑在 `src/infra/http/retry.rs:67-71`（`DefaultRetryPolicy`），语义与旧 commit 一致（429 → `Retry`，不 disable，不 failover），但比旧 commit 更完整（覆盖了 408 + 5xx 退避，有指数退避 + jitter）。

**冲突点**：
- 旧 master commit 中没有实现"冷却摘出账号"，仅仅是"跳过 report_failure"；refactor/v2 中同样没有冷却机制。两者语义对齐，不存在实质冲突，但"账号级冷却"功能在两侧均缺失，属于共同 gap。
- `RefreshError::RateLimited` 在 refactor/v2 中被 `report_refresh_failure()` 统一处理，可能累积禁用账号——这与 API 层 429 的"不禁用"策略不一致，是 refactor/v2 引入的潜在 regression。

---

## 锁顺序约束（来自 backend-refactor-plan）

`src/service/credential_pool/pool.rs:1-14`（注释）记载了固定锁顺序：

```
store → state → stats
```

**禁止反向获取**（避免死锁）。

如果为限流引入新的共享状态（如 `cooldown_until`），约束如下：

1. 若 `cooldown_until` 存放于 `EntryState`（`state` 层），则直接复用 `state` 锁，**不需要新锁**，锁顺序不变。
2. 若单独引入 `RateLimitState`（第四把锁），必须排在 `stats` 之后（`store → state → stats → ratelimit`），或改为无锁（`DashMap` / `AtomicU64` 时间戳）。
3. **禁止**跨 `.await` 持锁——`pool.rs` 注释已明确，冷却时间的存取必须是同步操作，sleep 前放锁。
4. `select_one()` 是纯同步调用，持有三把锁的 `snapshot()` 副本，`cooldown_until` 过滤可在 snapshot 后做，**不需要额外加锁**。
