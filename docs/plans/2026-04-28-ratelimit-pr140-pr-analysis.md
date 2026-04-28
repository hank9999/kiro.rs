# PR #140 Ratelimit 补丁分析

## Commits / 文件清单

| Commit | 日期 | 标题 |
|--------|------|------|
| 7b6c2d1 | 2026-04-16 | feat: 增加凭据限流功能 |
| 8f33544 | 2026-04-18 | Refine rate limit status display and runtime handling |

### 文件改动一览（25 个）

| 文件 | +行 | -行 | 说明 |
|------|-----|-----|------|
| `src/model/rate_limit.rs` | 207 | 0 | **新增**：限流规则解析与验证核心模块 |
| `src/kiro/token_manager.rs` | 607 | 140 | 限流运行时状态管理、凭据选择逻辑重构 |
| `src/kiro/model/credentials.rs` | 32 | 0 | `KiroCredentials` 添加 `rate_limits` 字段 |
| `src/model/config.rs` | 14 | 1 | `Config` 添加 `default_rate_limits` 字段 |
| `src/model/mod.rs` | 1 | 0 | 新增 `rate_limit` 模块 pub 导出 |
| `src/admin/types.rs` | 58 | 0 | Admin API 响应类型：`RateLimitSummary`、`CredentialStatusItem` 扩展 |
| `src/admin/service.rs` | 50 | 2 | Admin 服务层：读写全局/凭据级限流配置 |
| `src/admin/handlers.rs` | 39 | 2 | 新增 3 个 HTTP handler |
| `src/admin/router.rs` | 14 | 2 | 注册新路由 |
| `src/kiro/provider.rs` | 32 | 11 | `acquire_context` 接入限流选择逻辑 |
| `src/anthropic/handlers.rs` | 46 | 26 | 适配新 `acquire_context` 返回值 |
| `src/anthropic/converter.rs` | 71 | 27 | 类型转换调整 |
| `src/anthropic/stream.rs` | 67 | 47 | SSE 流处理适配 |
| `src/http_client.rs` | 4 | 0 | 无显式代理时强制 `no_proxy()` |
| `src/main.rs` | 3 | 0 | 启动时传递全局配置到 token manager |
| `admin-ui/src/types/api.ts` | 22 | 0 | 前端类型定义 |
| `admin-ui/src/api/credentials.ts` | 30 | 0 | 前端 API 调用封装 |
| `admin-ui/src/components/rate-limit-dialog.tsx` | 156 | 0 | **新增**：限流规则编辑弹窗组件 |
| `admin-ui/src/components/credential-card.tsx` | 87 | 1 | 凭据卡片展示限流运行态 |
| `admin-ui/src/components/dashboard.tsx` | 42 | 1 | 全局默认限流配置入口 |
| `admin-ui/src/hooks/use-credentials.ts` | 36 | 1 | 前端状态 hook 扩展 |
| `README.md` | 71 | 1 | 补充限流配置与行为文档 |
| `.codex/skills/build-kiro-rs-image/SKILL.md` | 68 | 0 | 新增 Docker 镜像构建技能 |
| `.codex/skills/build-kiro-rs-image/scripts/build.sh` | 61 | 0 | 构建脚本 |

净增约 1058 行（+1185 / -127）。

---

## 设计概览

作者在 `MultiTokenManager` 内以纯内存滑动窗口实现凭据级限流：每条凭据维护一个 `VecDeque<Instant>` 存储历史请求时间戳，每次 `acquire_context` 时先对队列做过期清理，再遍历所有规则判断窗口内计数是否超限。若单条凭据超限则自动轮转至其他可用凭据；若所有凭据全部超限则 sleep 等待最早恢复时间点后重试，而非直接返回错误。限流规则分两级：全局 `defaultRateLimits` 和凭据级 `rateLimits`（后者完全覆盖前者），均通过 Admin API 可读写并持久化到配置文件，运行态（时间戳队列）仅存内存。

---

## 关键代码片段

### 1. 滑动窗口核心算法 — `src/model/rate_limit.rs`

```rust
// 解析时间窗口字符串（如 "5m" → 300 秒）
pub fn parse_window_seconds(window: &str) -> anyhow::Result<u64> {
    // 验证格式：正整数 + 单位(s/m/h/d)，最大 30 天
}

// 批量解析、去重、排序规则
pub fn resolve_rate_limit_rules(
    rules: &[RateLimitRule],
    source: &str,
) -> anyhow::Result<Vec<ResolvedRateLimitRule>> {
    // 验证无重复窗口、maxRequests > 0
    // 返回已按窗口长度升序排列的规范化规则
}

// 合并全局默认与凭据级覆盖
pub fn effective_rate_limit_rules(
    defaults: Option<&[RateLimitRule]>,
    overrides: Option<&[RateLimitRule]>,
) -> anyhow::Result<Vec<RateLimitRule>> {
    // 凭据级规则存在时完全忽略全局默认值（完整覆盖语义，无合并）
}
```

### 2. 限流状态与检查 — `src/kiro/token_manager.rs`

```rust
struct CredentialRateLimitState {
    request_timestamps: VecDeque<Instant>,
}

enum RateLimitAvailability {
    Ready,
    LimitedUntil(Instant),
}

fn check_rate_limit(
    state: &mut CredentialRateLimitState,
    rules: &[ResolvedRateLimitRule],
    now: Instant,
) -> RateLimitAvailability {
    // 1. 清理最旧窗口之外的所有时间戳（减少内存占用）
    // 2. 遍历每条规则：统计时间窗口内请求数
    // 3. 若任一规则超限，返回 LimitedUntil(最早的恢复时刻)
    // 4. 全部规则未超限，返回 Ready
}

fn reserve_rate_limit_slot(&self, id: u64, now: Instant) {
    // 请求成功后将 now 记录到凭据的时间戳队列
}
```

### 3. 凭据选择与等待 — `src/kiro/provider.rs` + `token_manager.rs`

```rust
enum CredentialSelection {
    Ready(u64, Credentials),
    WaitUntil(Instant),
    NoneAvailable,
}

pub async fn acquire_context(&self, model: Option<&str>) -> Result<ApiContext> {
    loop {
        match self.select_next_credential(model) {
            CredentialSelection::Ready(id, credentials) => {
                self.reserve_rate_limit_slot(id, Instant::now());
                return Ok(ctx);
            }
            CredentialSelection::WaitUntil(next) => {
                sleep_until(TokioInstant::from_std(next)).await;
                continue; // 重试，不直接失败
            }
            CredentialSelection::NoneAvailable => {
                // 尝试自动恢复所有禁用凭据，仍无则返回错误
            }
        }
    }
}
```

### 4. Admin API 响应类型 — `src/admin/types.rs`

```rust
#[derive(Debug, Serialize)]
pub struct RateLimitSummary {
    pub window: String,           // 如 "5m"
    pub max_requests: u32,        // 窗口限额
    pub remaining_requests: u32,  // 当前剩余额度
}

// CredentialStatusItem 新增字段：
pub rate_limits: Option<Vec<RateLimitRule>>,     // 凭据自身配置
pub effective_rate_limits: Vec<RateLimitRule>,   // 生效规则
pub rate_limited: bool,                           // 当前是否限流中
pub next_available_at: Option<String>,            // RFC3339 格式恢复时间
pub rate_limit_summary: Option<RateLimitSummary>, // 最紧张的单条摘要
pub rate_limit_summaries: Vec<RateLimitSummary>,  // 前两条最紧张的摘要
```

### 5. Admin 路由 — `src/admin/router.rs`

```rust
.route("/credentials/{id}/rate-limits", post(set_credential_rate_limits))
.route("/config/rate-limits", get(get_default_rate_limits).put(set_default_rate_limits))
```

### 6. 配置示例（来自 README.md 更新）

```json
// config.json 全局默认
{
  "defaultRateLimits": [
    { "window": "5m", "maxRequests": 100 },
    { "window": "24h", "maxRequests": 3000 }
  ]
}

// credentials.json 凭据级覆盖
[
  {
    "refreshToken": "...",
    "rateLimits": [
      { "window": "2m", "maxRequests": 20 },
      { "window": "1d", "maxRequests": 2000 }
    ]
  }
]
```

---

## 设计要素

- **触发条件**：限流在 `acquire_context`（获取 API 上下文）阶段触发，即在发出任何上游 HTTP 请求之前就基于内存计数拦截。patch 中未见对上游 HTTP 429 响应头或 SSE 事件的响应式限流处理——触发源完全是本地计数，非上游信号。

- **限流粒度**：按**凭据（credential）**粒度，即每个 refreshToken/API key 对应独立的时间戳队列。无按 IP 或按请求者限流。

- **算法**：**滑动窗口（Sliding Window）**。使用 `VecDeque<Instant>` 存储历史请求时刻，每次检查前先清理过期时间戳，再统计窗口内计数。支持同时配置多条不同时间窗口的规则（"且"逻辑，任一超限即触发）。

- **持久化**：**规则配置持久化**（写入 config.json / credentials.json），**运行态不持久化**（`VecDeque<Instant>` 仅存内存，重启后计数清零）。

- **集成层**：集成在 `MultiTokenManager::acquire_context` 层，即**账号选择器层**，属于 hexagonal 架构中的应用层/端口层，早于上游 HTTP client 层触发。

- **配置**：新增两个配置字段：
  - `config.defaultRateLimits: Vec<{window: string, maxRequests: u32}>` — 全局默认规则
  - `credentials[].rateLimits: Vec<{window: string, maxRequests: u32}>` — 凭据级覆盖规则
  - 时间窗口格式：`{正整数}{s|m|h|d}`，如 `30s`、`5m`、`2h`、`1d`，最大 30 天
  - 两者均无默认值（未配置则不限流）
  - Admin API 支持运行时读写并持久化，无需重启

- **可观测性**：Admin API 响应中新增：`rate_limited`（布尔）、`next_available_at`（RFC3339）、`rate_limit_summary`（最紧张单条）、`rate_limit_summaries`（前两条最紧张窗口 + `remaining_requests`）。Admin UI 凭据卡片直接展示这些字段。patch 中**未见**结构化日志（`tracing` / `log` 宏）的新增，可观测性主要依靠 Admin API 轮询，无主动告警机制。

- **测试**：PR 描述提到测试命令为 `cargo test test_snapshot_`（快照测试），但 patch 正文中**未提取到新增测试文件或 `#[test]` 函数**。覆盖程度未明示。

---

## 待澄清/可疑点

1. **计入限流的请求范围**：PR 描述称"仅 `/generateAssistantResponse` 和 `/mcp` 计入限流"，但 patch 内未提取到明确的过滤代码——可能通过 `reserve_rate_limit_slot` 的调用位置隐式控制，需进一步验证。

2. **全部凭据超限时的等待策略**：`WaitUntil` 分支使用 `sleep_until` 阻塞当前任务，若等待时间较长（如数小时）且并发请求多，可能导致大量 tokio task 积压——patch 中未见超时上限或队列限制。

3. **并发安全性**：`check_rate_limit` 和 `reserve_rate_limit_slot` 之间存在检查-然后-写入（check-then-act）的时间窗口，在高并发下可能导致超限。patch 提到使用"单锁"，但具体锁范围未从摘要中确认。

4. **凭据级规则完全覆盖全局默认**：当凭据配置了任意 `rateLimits` 时，全局 `defaultRateLimits` 完全被忽略（非合并）。这意味着若全局配置了日限额，凭据级若只配置分钟限额则日限额失效——是否符合预期未在 PR 中讨论。

5. **`Instant` 跨线程序列化**：`std::time::Instant` 不可跨进程序列化，与"持久化不持久化"的选择一致，但若将来要改为持久化需要改为 `SystemTime` 或 `chrono`。

---

## 不可信任的部分（需在 refactor/v2 现状中验证）

1. **`MultiTokenManager` 是否存在**：patch 基于 master 分支，refactor/v2 已对 token_manager 层进行重构（Phase D/E）。`MultiTokenManager` 的结构、`acquire_context` 签名、凭据选择逻辑可能已有较大差异，不能直接 cherry-pick。

2. **Admin API 路由前缀**：refactor/v2 是否保留了相同的 Admin router 结构（`/credentials/{id}/rate-limits`、`/config/rate-limits`）未知，需对比 `src/admin/router.rs` 现状。

3. **`CredentialStatusItem` 字段集合**：refactor/v2 的 `admin/types.rs` 已做过重构，patch 新增的字段是否与当前类型定义兼容未知。

4. **快照测试（`test_snapshot_`）**：patch 声称通过快照测试，但快照内容未在 patch 中展示。若 refactor/v2 改动了序列化格式，快照需要更新。

5. **`http_client.rs` 改动**：强制 `no_proxy()` 的改动可能与 refactor/v2 的代理处理方式冲突，需验证。
