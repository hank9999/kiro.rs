# Plan: Kiro 端点分层重构 (2026-04-18)

[CC] 本计划基于 `/home/hank9999/kiro.rs/docs/plans/2026-04-18-kiro-endpoint-layered-refactor-research.md` 编写，严格遵循用户裁决后的硬约束。

---

## Summary

[CC] 将 Kiro 端点抽象的职责从散落在 provider / token_manager / admin_service / main.rs 的胶水代码中抽离，折叠为三层：
- **协议层**：`KiroEndpoint` trait 引入 `build_request()` + `classify_error()` + `KiroRequest` + `EndpointErrorKind`，折叠现有 10 个分散方法。
- **路由层**：新增 `EndpointRegistry`，替代 main.rs / provider / admin_service 的三份副本。
- **调度层**：`CallContext` 注入 `endpoint: Arc<dyn KiroEndpoint>` 与 `machine_id`；`MultiTokenManager.acquire_context*()` 预解析这两个字段；`KiroProvider` 的两个重试循环通过泛型 helper 简化；`get_usage_limits_for` 走 trait 消除 IDE 硬编码。

本轮**不实现** CLI endpoint（决策 1），但接口与注册表留有扩展空间。重构采用渐进迁移策略——新旧 trait 方法并存，切换所有调用点后再清理旧方法，保证每阶段均可编译、测试。

---

## Stakes Classification

[CC] **Level**: High

**Rationale**：
1. 改动位于主请求链路（所有 API/MCP 调用都经过 provider 重试循环），任何回归都会导致线上凭据轮转、错误分类、token 刷新行为错乱。
2. 涉及跨模块签名变更（CallContext、RequestContext、KiroEndpoint trait），多个调用点需要同步迁移。
3. 回归面覆盖 §5 全部 6 类行为（重试策略、凭据轮转、Token 刷新、负载均衡、端点路由、额度管理）。
4. admin_service 的凭据校验是对外 HTTP API 的一部分，错误消息格式变化可能破坏调用方。

---

## Context

**Research**：`/home/hank9999/kiro.rs/docs/plans/2026-04-18-kiro-endpoint-layered-refactor-research.md`（唯一事实来源）

**Affected Areas**（精确路径）：
- `src/kiro/endpoint/mod.rs` — trait 与 RequestContext 定义
- `src/kiro/endpoint/ide.rs` — IDE 端点实现
- `src/kiro/provider.rs` — KiroProvider、两个重试循环、client_cache
- `src/kiro/token_manager.rs` — MultiTokenManager、CallContext、`get_usage_limits_for`、`get_usage_limits`
- `src/kiro/admin_service.rs` — AdminService、`add_credential` 端点校验
- `src/main.rs` — 启动时端点注册、校验
- `src/kiro/machine_id.rs` — 仅调用点迁移，不改签名

---

## 必须保持不变的行为（契约，照抄研究 §5）

[CC] 以下行为在重构前后必须字节级一致；任何偏离都视为回归。

### 重试策略
- [ ] 429 / 408 / 5xx 触发 sleep + 重试，不禁用凭据
- [ ] 重试次数上限 = `min(凭据数 × MAX_RETRIES_PER_CREDENTIAL, MAX_TOTAL_RETRIES)` = `min(n × 3, 9)`
- [ ] 重试延迟沿用 `Self::retry_delay(attempt)`（指数退避）

### 凭据轮转与禁用
- [ ] 402 + 月度配额 → 禁用凭据 + 切换下一可用凭据（按优先级）
- [ ] 401/403 + Bearer token 失效 → 强制刷新（每凭据一次机会）；刷新失败后 `report_failure` 禁用
- [ ] 400 Bad Request → 直接 bail，不重试、不计入失败
- [ ] 连续失败 3 次 → 凭据被禁用

### Token 刷新
- [ ] API Key 凭据无刷新流程
- [ ] OAuth 凭据过期/即将过期时自动刷新（`try_ensure_token`）
- [ ] `force_refresh_token_for` 无条件刷新；API Key 凭据返回错误；每凭据仅一次机会

### 负载均衡
- [ ] priority 模式：优先 current_id，不可用切最高优先级
- [ ] balanced 模式：每次请求重新 `select_next_credential`

### 端点路由
- [ ] 凭据指定 endpoint → 优先使用；未指定 → 使用默认端点
- [ ] 启动时校验 default_endpoint 和所有凭据声明的 endpoint；缺失则进程退出
- [ ] 运行时 endpoint 解析失败不中断流程（provider 继续轮转凭据）

### 月度额度
- [ ] `get_usage_limits_for` 继续按凭据返回额度
- [ ] 重构后 IDE 额度请求的 Host/URL/User-Agent/查询参数与现有 `get_usage_limits`（第 323-393 行）**完全一致**

---

## 设计调整（覆盖研究文档原描述）

[CC] 以下设计根据用户裁决**覆盖或细化**研究文档 §1 的初稿：

### RequestContext 保留 machine_id（覆盖研究 §1.1 的简化版）

研究文档 §1.1 建议删除 `machine_id` 字段、由 endpoint 内部调用 `machine_id::generate_from_credentials`。**该建议作废**。最终设计：

```rust
pub struct RequestContext<'a> {
    pub credentials: &'a KiroCredentials,
    pub token: &'a str,
    pub machine_id: &'a str,    // 保留
    pub config: &'a Config,
}
```

machine_id 由 `MultiTokenManager.acquire_context*()` 在构造 `CallContext` 时**预先计算一次**并存入 `CallContext`，provider 构造 `RequestContext` 时直接借用。endpoint.build_request 内部**不再调用** `generate_from_credentials`。

### EndpointErrorKind 的精确语义（决策 2）

```rust
pub enum EndpointErrorKind {
    MonthlyQuotaExhausted,   // 402 + body 含月度配额标记
    BearerTokenInvalid,       // 401/403 + body 含 bearer 失效标记
    BadRequest,               // 仅对应 HTTP 400，直接 bail
    ClientError,              // 其他未分类 4xx（经 401/402/403 特殊 arm 处理后的剩余 4xx）
    Transient,                // 408/429/5xx，sleep + 重试
    Unknown,                  // 兜底：当作可重试瞬态错误
}
```

签名：`fn classify_error(&self, status: u16, body: &str) -> EndpointErrorKind`

endpoint 实现可**同时**使用 status 和 body 判断（两者都传入）。

### CallContext 增强

```rust
pub struct CallContext {
    pub id: u64,
    pub credentials: KiroCredentials,
    pub token: String,
    pub endpoint: Arc<dyn KiroEndpoint>,  // 新增
    pub machine_id: String,                // 新增（研究裁决版）
}
```

### EndpointRegistry 公开 API（决策 4）

```rust
impl EndpointRegistry {
    pub fn new(endpoints: HashMap<String, Arc<dyn KiroEndpoint>>, default: String) -> anyhow::Result<Self>;
    pub fn resolve(&self, credentials: &KiroCredentials) -> Arc<dyn KiroEndpoint>;
    pub fn contains(&self, name: &str) -> bool;   // 公开
    pub fn names(&self) -> Vec<&str>;              // 公开
}
```

**AdminService** 不再持有 `HashSet<String>`，改为持有 `Arc<EndpointRegistry>`，调用 `registry.contains(name)` + `registry.names()` 校验；错误消息格式保留在 admin_service 组装（业务关注点分离）。

### CLI endpoint 本轮不实现（决策 1）

- `src/kiro/endpoint/` 保持 `mod.rs` + `ide.rs`
- 不新增 `cli.rs`，不新增第二个 `impl KiroEndpoint`
- main.rs 注册表启动时**仅**注册 `IdeEndpoint`
- trait / registry 的设计需保证未来添加 cli.rs 时**无需修改** 已有代码（开闭原则）

---

## 设计讨论：API/MCP 重试循环合并（决策 5）

[CC] 基于研究 §2.3.3 的行号做 diff 级别分析：

### 两个循环的差异点

| 维度 | `call_mcp_with_retry`（129-271） | `call_api_with_retry`（279-491） |
|------|-------------------------------|-------------------------------|
| acquire_context 参数 | `None` | `model.as_deref()`（从请求体提取 model） |
| URL 来源 | `endpoint.mcp_url(&rctx)` | `endpoint.api_url(&rctx)` |
| body 变换 | `endpoint.transform_mcp_body(request_body, &rctx)` | `endpoint.transform_api_body(request_body, &rctx)` |
| header 装饰 | `endpoint.decorate_mcp(base, &rctx)` | `endpoint.decorate_api(base, &rctx)` |
| 错误消息前缀 | `"MCP 请求失败"` | `"{api_type} API 请求失败"`（流式/非流式） |
| is_stream 参数 | 无 | 有（仅用于错误消息前缀） |

### 可合并项

1. **重试次数计算**：完全一致（`min(total × 3, 9)`）
2. **force_refreshed Set 语义**：完全一致
3. **成功/失败上报**：`report_success` / `report_failure` / `report_quota_exhausted` 完全一致
4. **错误分支顺序（重构前）**：402 → 400 → 401/403 → 瞬态 → 其他 4xx → 兜底，两循环顺序一致。
   重构后枚举化为：MonthlyQuotaExhausted → BadRequest → BearerTokenInvalid → Unauthorized → Transient → ClientError → Unknown。
   注意：原"401/403"分支在枚举化时曾被拆分导致无标记 401/403 落入 ClientError 回归（见研究报告 2026-04-23），已通过新增 Unauthorized 变体修复。
5. **sleep 时机**：完全一致
6. **client_for 调用**：两者均通过 `self.client_for(&ctx.credentials)?` 取 client

### 不可直接合并的差异

1. **acquire_context 传 model**：通过 `KiroRequest` 枚举携带 `model: Option<&str>` 上下文，helper 从枚举取值后传入 acquire_context
2. **endpoint 方法名**：重构后统一为 `build_request(ctx, KiroRequest::Xxx)`，自然消除
3. **错误消息前缀差异**：由调用方传入 `request_kind_label: &str`（如 `"MCP 请求"` / `"流式 API 请求"` / `"非流式 API 请求"`）

### 推荐：基于阶段 3 完成后再合并（作为阶段 5 必做项，非可选优化）

[CC] 合并后 ~200 行代码 → ~130 行单一 helper，收益显著；且合并依赖于 `build_request` 已就位（阶段 1/2），风险可控。因此**合并列为阶段 5 的必做任务**，不标注为可选优化。

helper 签名建议（伪代码）：

```rust
async fn call_with_retry(
    &self,
    req: KiroRequest<'_>,
    model_for_acquire: Option<&str>,
    kind_label: &str,
) -> anyhow::Result<reqwest::Response>;
```

---

## Success Criteria

- [ ] `cargo build` 在主干分支无新增 warning
- [ ] `cargo clippy --all-targets -- -D warnings` 通过
- [ ] `cargo test` 所有已有测试通过（含 endpoint/mod.rs 的 4 个单元测试）
- [ ] 新增 `EndpointErrorKind` / `KiroRequest` 枚举对应的单元测试覆盖所有变体
- [ ] 手工回归：IDE 凭据的 generateAssistantResponse / MCP / usageLimits 三类请求在正常/402/401/400/429 场景行为与重构前一致
- [ ] `admin_service.add_credential` 使用未注册端点名时错误消息格式保持不变
- [ ] `src/kiro/endpoint/` 目录保持仅 `mod.rs` + `ide.rs`（决策 1）
- [ ] `git grep 'endpoint_for'` 无残留；`git grep 'known_endpoints'` 无残留；`HashMap<String, Arc<dyn KiroEndpoint>>` 仅在 EndpointRegistry 内部出现

---

## Implementation Steps

[CC] 共 **5 个阶段**，严格 before/after 顺序。每阶段最后一步描述该阶段的回滚方式。

---

### 阶段 1：协议层扩展（新增枚举与新 trait 方法，旧方法保留）

**目标**：在不破坏现有代码的前提下，引入 `KiroRequest`、`EndpointErrorKind`、`build_request`、`classify_error`，并在 `IdeEndpoint` 中提供实现。

**涉及文件**：
- `src/kiro/endpoint/mod.rs`
- `src/kiro/endpoint/ide.rs`

#### Step 1.1：定义新枚举（RED：先写测试）

- **Files**: `src/kiro/endpoint/mod.rs`
- **Action**:
  - 新增 `pub enum KiroRequest<'a> { GenerateAssistant { body: &'a str, stream: bool, model: Option<&'a str> }, Mcp { body: &'a str }, UsageLimits }`
  - 新增 `pub enum EndpointErrorKind { MonthlyQuotaExhausted, BearerTokenInvalid, BadRequest, ClientError, Transient, Unknown }`
  - 为 `EndpointErrorKind` 派生 `Debug, Clone, Copy, PartialEq, Eq`
- **Test cases**（在 `mod tests` 中新增）：
  - `EndpointErrorKind` 可 Debug 打印
  - `KiroRequest::GenerateAssistant` / `Mcp` / `UsageLimits` 三变体可构造
- **Verify**: `cargo build` 通过；新增测试通过
- **Complexity**: Small

#### Step 1.2：在 KiroEndpoint trait 新增默认方法（不删除旧方法）

- **Files**: `src/kiro/endpoint/mod.rs`
- **Action**: 在 trait 中追加：
  ```rust
  fn build_request(
      &self,
      client: &reqwest::Client,
      ctx: &RequestContext<'_>,
      req: &KiroRequest<'_>,
  ) -> anyhow::Result<RequestBuilder> { unimplemented!("endpoint {} 未实现 build_request", self.name()) }

  fn classify_error(&self, status: u16, body: &str) -> EndpointErrorKind {
      // 默认实现：复用 is_monthly_request_limit / is_bearer_token_invalid + status
      if status == 402 && self.is_monthly_request_limit(body) { return EndpointErrorKind::MonthlyQuotaExhausted; }
      if matches!(status, 401 | 403) && self.is_bearer_token_invalid(body) { return EndpointErrorKind::BearerTokenInvalid; }
      if status == 400 { return EndpointErrorKind::BadRequest; }
      if matches!(status, 408 | 429) || (500..600).contains(&status) { return EndpointErrorKind::Transient; }
      if (400..500).contains(&status) { return EndpointErrorKind::ClientError; }
      EndpointErrorKind::Unknown
  }
  ```
- **Test cases**（补充测试）：
  - `default classify_error(402, "...MONTHLY_REQUEST_COUNT...")` → `MonthlyQuotaExhausted`
  - `default classify_error(401, "The bearer token included in the request is invalid")` → `BearerTokenInvalid`
  - `default classify_error(400, "{}")` → `BadRequest`
  - `default classify_error(429, "{}")` → `Transient`
  - `default classify_error(500, "{}")` → `Transient`
  - `default classify_error(402, "{}")` → `ClientError`（402 但 body 不含配额标记）
  - `default classify_error(404, "{}")` → `ClientError`
  - `default classify_error(200, "{}")` → `Unknown`
- **Verify**: `cargo build` + 新增测试通过
- **Complexity**: Small

#### Step 1.3：IdeEndpoint 实现 build_request（RED：先写测试）

- **Files**: `src/kiro/endpoint/ide.rs`
- **Action**:
  - 实现 `fn build_request(&self, client, ctx, req) -> anyhow::Result<RequestBuilder>`
  - 内部 match `req`：
    - `KiroRequest::GenerateAssistant { body, stream, .. }` → 等价现有 `api_url + transform_api_body + decorate_api` 三段式（stream 字段当前 IDE 不影响 URL/header，仅留作未来用）
    - `KiroRequest::Mcp { body }` → 等价 `mcp_url + transform_mcp_body + decorate_mcp`
    - `KiroRequest::UsageLimits` → 等价 `token_manager::get_usage_limits` 中硬编码的 IDE 格式（Host/URL/User-Agent/查询参数）
  - 公共 header 设置（content-type、Connection: close）也移入 build_request
- **Test cases**（`ide.rs` 中新增 `mod tests`）：
  - `build_request(GenerateAssistant)` 返回的 Builder 通过 `.build()?` 后 URL 含 `generateAssistantResponse`
  - `build_request(Mcp)` URL 含 `/mcp`
  - `build_request(UsageLimits)` URL 与 `get_usage_limits` 函数构造的 URL 字节级相等
  - 三者 Authorization header 均为 `Bearer {token}`
  - GenerateAssistant body 已注入 profile_arn（当 credentials 有 profile_arn 时）
- **Verify**: `cargo build` + `cargo test` 通过
- **Complexity**: Medium

#### Step 1.4：IdeEndpoint 实现 classify_error（可选覆盖默认）

- **Files**: `src/kiro/endpoint/ide.rs`
- **Action**: 本阶段**不覆盖**默认实现（默认已足够）。仅在 IDE 有特殊格式时才 override。当前研究未发现 IDE 特有错误格式差异，跳过。
- **Verify**: 确认无 override 也能通过所有测试
- **Complexity**: Small

#### Step 1.5：阶段 1 验收

- **Verify**:
  - `cargo build`、`cargo clippy --all-targets -- -D warnings`、`cargo test` 全绿
  - 旧 trait 方法（`api_url` / `mcp_url` / `decorate_api` / `decorate_mcp` / `transform_*_body` / `is_*`）**保留不动**，provider.rs 调用路径未改
- **回滚**：本阶段**仅新增** trait 方法与枚举，不修改任何旧路径；直接 `git revert` 阶段 1 的 commit 即可完全回滚。

---

### 阶段 2：路由层 — 引入 EndpointRegistry

**目标**：新增 `EndpointRegistry` 并替换三处调用点；admin_service 丢弃 `HashSet<String>` 副本。

**涉及文件**：
- `src/kiro/endpoint/mod.rs`（新增 registry 子模块或同文件内 struct）
- `src/main.rs`
- `src/kiro/provider.rs`（本阶段仍保留 `endpoints` 字段，但改为从 registry 取用；`endpoint_for` 改为委托）
- `src/kiro/admin_service.rs`

#### Step 2.1：定义 EndpointRegistry 与单元测试（RED）

- **Files**: `src/kiro/endpoint/mod.rs`（或新增 `src/kiro/endpoint/registry.rs`）
- **Action**:
  - 新增 struct `EndpointRegistry { endpoints: HashMap<String, Arc<dyn KiroEndpoint>>, default: String }`
  - 实现 `new()` 校验 default 存在于 endpoints；不存在则 `anyhow::bail!`
  - `resolve(&self, credentials)`：按 `credentials.endpoint.as_deref().unwrap_or(&self.default)` 查找；**未命中时 fallback 到 default** 并 `tracing::warn!`（保持运行时路由不中断）
  - `contains(&self, name) -> bool`
  - `names(&self) -> Vec<&str>`
- **Test cases**：
  - `new` 空 endpoints + default="ide" → Err
  - `new` endpoints 含 "ide" + default="ide" → Ok
  - `resolve` 凭据 endpoint=Some("ide") → 命中 ide
  - `resolve` 凭据 endpoint=None → 命中 default
  - `resolve` 凭据 endpoint=Some("unknown") → 返回 default 且触发 warn
  - `contains("ide")` → true；`contains("cli")` → false
  - `names()` 返回已注册名列表
- **Verify**: `cargo test endpoint::registry` 通过
- **Complexity**: Medium

#### Step 2.2：main.rs 切换到 EndpointRegistry 构造

- **Files**: `src/main.rs`（第 100-127 行区域）
- **Action**:
  - 保留 HashMap 的构造（仅注册 IdeEndpoint）
  - 用 `EndpointRegistry::new(endpoints, config.default_endpoint.clone())?` 替代原校验逻辑
  - 启动时额外遍历 credentials 调用 `registry.contains(cred.endpoint)` 做严格校验（沿用原错误消息与 `std::process::exit(1)` 行为）
  - `let endpoint_names: Vec<String> = registry.names().iter().map(|s| s.to_string()).collect()` 临时保留，供下一步 admin_service 迁移
  - 将 `Arc<EndpointRegistry>` 同时传给 KiroProvider 和 AdminService
- **Verify**: `cargo build` 通过；手工跑 main 启动无错（含配置错误端点时进程退出行为不变）
- **Complexity**: Medium

#### Step 2.3：AdminService 改为持有 Arc<EndpointRegistry>

- **Files**: `src/kiro/admin_service.rs`
- **Action**:
  - 删除 `known_endpoints: HashSet<String>` 字段
  - 新增 `endpoint_registry: Arc<EndpointRegistry>` 字段
  - `new` 签名改为 `pub fn new(token_manager: Arc<MultiTokenManager>, endpoint_registry: Arc<EndpointRegistry>, ...)`（其他参数不变）
  - `add_credential` 校验改为：
    ```rust
    if let Some(ref name) = req.endpoint {
        if !self.endpoint_registry.contains(name) {
            let mut known: Vec<&str> = self.endpoint_registry.names();
            known.sort();
            return Err(AdminServiceError::InvalidCredential(format!(
                "未知端点 \"{}\"，已注册端点: {:?}", name, known
            )));
        }
    }
    ```
  - **错误消息格式不变**（保持业务关注点分离）
- **Test cases**（如已有 admin_service 测试则扩展；否则新增）：
  - `add_credential` 传入 endpoint=Some("unknown") → InvalidCredential 且消息含 "未知端点"
  - `add_credential` 传入 endpoint=Some("ide") → 不因 endpoint 校验失败
  - `add_credential` 传入 endpoint=None → 不因 endpoint 校验失败
- **Verify**: `cargo build` + `cargo test` 通过
- **Complexity**: Small

#### Step 2.4：KiroProvider 的 endpoints 字段替换为 Arc<EndpointRegistry>

- **Files**: `src/kiro/provider.rs`（第 33-46 行、第 97-109 行）
- **Action**:
  - 删除 `endpoints: HashMap<String, Arc<dyn KiroEndpoint>>` 与 `default_endpoint: String` 字段
  - 新增 `endpoint_registry: Arc<EndpointRegistry>` 字段
  - `endpoint_for(&self, credentials)` 改为委托：`Ok(self.endpoint_registry.resolve(credentials))`（返回类型由 `anyhow::Result<Arc<dyn _>>` 改为 `Arc<dyn _>`，因为 resolve fallback 到 default 不会失败；所有调用点同步删除 `?` 与错误 arm）
- **Test cases**：无新增（重试循环行为通过阶段 5 的回归验证）
- **Verify**: `cargo build` + 所有已有测试通过
- **Complexity**: Medium

#### Step 2.5：阶段 2 验收

- **Verify**:
  - `git grep 'known_endpoints'` 无残留
  - `git grep -n 'HashMap<String, Arc<dyn KiroEndpoint>>'` 仅在 main.rs 构造 registry 和 registry 内部出现
  - `cargo clippy --all-targets -- -D warnings` 通过
  - 手工启动：正常配置、错误 default_endpoint、错误 credential.endpoint 三种场景行为与重构前一致
- **回滚**：本阶段改动跨 3-4 个文件，回滚步骤 = `git revert` 该阶段的 commit 范围。由于未删除 trait 旧方法，provider 重试循环仍使用 `api_url`/`mcp_url` 等旧路径，回滚只影响 registry 层。

---

### 阶段 3：调度层 — CallContext 增强 + registry 注入 TokenManager

**目标**：`MultiTokenManager` 持有 `Arc<EndpointRegistry>`，`acquire_context*()` 在构造 `CallContext` 时预解析 endpoint 与 machine_id。

**涉及文件**：
- `src/kiro/token_manager.rs`
- `src/main.rs`（构造 MultiTokenManager 时传入 registry）
- `src/kiro/provider.rs`（重试循环使用 `ctx.endpoint` 与 `ctx.machine_id`）

#### Step 3.1：CallContext 新增字段

- **Files**: `src/kiro/token_manager.rs`（第 540-548 行）
- **Action**:
  ```rust
  #[derive(Clone)]
  pub struct CallContext {
      pub id: u64,
      pub credentials: KiroCredentials,
      pub token: String,
      pub endpoint: Arc<dyn KiroEndpoint>,
      pub machine_id: String,
  }
  ```
  > **⚠️ 后续变更（阶段 7，2026-04-24）**：上述 `pub` 字段已在阶段 7 改为私有，通过 getter 暴露只读访问。详见阶段 7 说明。
- **Verify**: `cargo build` 会报调用点错误（下一步修复）
- **Complexity**: Small

#### Step 3.2：MultiTokenManager 持有 registry + 预解析逻辑

- **Files**: `src/kiro/token_manager.rs`（字段定义、new、acquire_context、acquire_context_for、try_ensure_token）
- **Action**:
  - 字段新增 `endpoint_registry: Arc<EndpointRegistry>`
  - `new()` 签名追加 registry 参数（更新所有调用点 main.rs）
  - `try_ensure_token`（第 884-969 行）：在构造 CallContext 时
    - `let endpoint = self.endpoint_registry.resolve(&credentials);`
    - `let machine_id = machine_id::generate_from_credentials(&credentials, &self.config);`
    - 填充两个新字段
  - `acquire_context` / `acquire_context_for` 通过 `try_ensure_token` 间接获取，无需单独处理
- **Test cases**（`token_manager` 如有 mock 测试则扩展）：
  - `acquire_context(None).await?` 返回的 CallContext.endpoint.name() == 凭据指定或默认 endpoint 名
  - CallContext.machine_id 非空且与 `generate_from_credentials(credentials, config)` 字节相等
- **Verify**: `cargo build` 成功；单元测试通过
- **Complexity**: Medium

#### Step 3.3：main.rs 将 registry 注入 MultiTokenManager

- **Files**: `src/main.rs`
- **Action**: 构造 MultiTokenManager 时传入 `Arc::clone(&registry)`；同时保持给 KiroProvider 和 AdminService 的 Arc clone
- **Verify**: `cargo build` 通过
- **Complexity**: Small

#### Step 3.4：provider 重试循环切换到使用 ctx.endpoint / ctx.machine_id

- **Files**: `src/kiro/provider.rs`（第 129-271、279-491 行）
- **Action**（两处循环同步修改）：
  - 删除 `let machine_id = machine_id::generate_from_credentials(...)`（第 146、304 行）
  - 删除 `let endpoint = self.endpoint_for(...)` 与其错误 arm（第 148-156、306-313 行）
  - `RequestContext` 构造改为 `machine_id: &ctx.machine_id`
  - 取代 `self.endpoint_for(...)` 的 `let endpoint = Arc::clone(&ctx.endpoint);`（或直接使用 `&ctx.endpoint`）
  - provider 保留 `endpoint_for` 方法本体暂不删，但标记 `#[allow(dead_code)]` 或立即删除（下一 step 若无引用则删）
- **Verify**: `cargo build` + `cargo test`；手工跑 API 请求正常；手工跑 MCP 请求正常
- **Complexity**: Medium

#### Step 3.5：清理 provider.endpoint_for 与相关字段

- **Files**: `src/kiro/provider.rs`
- **Action**:
  - 删除 `endpoint_for` 方法（第 97-109 行）
  - 如果字段 `endpoint_registry` 在 provider 本身已无读取点，保留该字段仅用于未来 resolve（可选）；若确认无用则删除。**决策**：保留字段，避免未来功能需要时再加回来；但不在本阶段新增读取点。
- **Verify**: `cargo clippy --all-targets -- -D warnings` 通过
- **Complexity**: Small

#### Step 3.6：阶段 3 验收

- **Verify**:
  - `cargo build` / `cargo clippy` / `cargo test` 全绿
  - 手工 API/MCP 请求：无 404 / 无 401 轮转异常 / 402 模拟下凭据禁用
  - `grep 'endpoint_for' src/` 在 provider 内无残留（注意 registry 内部的 resolve 不叫 endpoint_for）
  - `grep 'generate_from_credentials' src/` 仅在 token_manager 和 machine_id 模块内出现，provider 内无残留
- **回滚**：本阶段修改 CallContext 结构，回滚需确保 provider 的 `endpoint_for` + `machine_id` 代码恢复。通过 `git revert` 阶段 3 commits 即可，且阶段 2 的 registry 改动继续生效（registry 可独立存在）。

---

### 阶段 4：协议层收敛 — 重试循环切换到 build_request + classify_error

**目标**：provider 的两个重试循环从"三段式旧方法"切换到 `build_request` + `classify_error`。旧 trait 方法此时仍保留，供可能的降级。

**涉及文件**：
- `src/kiro/provider.rs`（两个重试循环内部）

#### Step 4.1：MCP 循环切换

- **Files**: `src/kiro/provider.rs`（第 129-271 行）
- **Action**：
  - 替换 `url / transform_mcp_body / decorate_mcp` 三段为：
    ```rust
    let req = KiroRequest::Mcp { body: request_body };
    let request = ctx.endpoint.build_request(self.client_for(&ctx.credentials)?, &rctx, &req)?;
    ```
  - 替换错误判断链为：
    ```rust
    match ctx.endpoint.classify_error(status.as_u16(), &body) {
        EndpointErrorKind::MonthlyQuotaExhausted => { /* 原 402 arm */ }
        EndpointErrorKind::BadRequest => { anyhow::bail!(...); }
        EndpointErrorKind::BearerTokenInvalid => { /* 原 401/403 + bearer 失效 arm */ }
        EndpointErrorKind::Transient => { /* 原 408/429/5xx arm */ }
        EndpointErrorKind::ClientError => { anyhow::bail!(...); }
        EndpointErrorKind::Unknown => { /* 兜底 */ }
    }
    ```
  - **保持错误消息格式、sleep 时机、force_refreshed 语义与原代码字节级一致**
- **Test cases**（手工回归，因集成路径无简单单测）：
  - 模拟 402 + MONTHLY_REQUEST_COUNT body → 凭据被禁用
  - 模拟 401 + bearer invalid body → 触发 force_refresh；第二次 401 不再 force_refresh
  - 模拟 400 → 直接 bail，不切换凭据
  - 模拟 429 → sleep 重试，不禁用
  - 模拟 404 → bail（ClientError arm）
- **Verify**: `cargo build`；手工跑 MCP 请求正常；打断点/日志确认分支路径
- **Complexity**: Medium

#### Step 4.2：API 循环切换（同结构，包含 stream 字段 + model 提取）

- **Files**: `src/kiro/provider.rs`（第 279-491 行）
- **Action**：
  - 构造 `KiroRequest::GenerateAssistant { body: request_body, stream: is_stream, model: model.as_deref() }`（注意 model 在循环外提取一次）
  - 同 4.1 结构切换
  - 错误消息前缀 `{api_type}` 保持
- **Test cases**（手工回归）：
  - 流式请求、非流式请求均正常
  - 月度配额 / bearer 失效 / 400 / 429 分支与 4.1 同
- **Verify**: `cargo build` + 手工回归
- **Complexity**: Medium

#### Step 4.3：删除旧 trait 方法（切换完成后收敛）

- **Files**: `src/kiro/endpoint/mod.rs`、`src/kiro/endpoint/ide.rs`
- **Action**:
  - 确认 provider 已无 `api_url / mcp_url / decorate_api / decorate_mcp / transform_api_body / transform_mcp_body / is_monthly_request_limit / is_bearer_token_invalid` 调用后
  - 从 trait 中删除这些方法
  - 从 `IdeEndpoint` 中删除对应 impl
  - `default_is_monthly_request_limit` / `default_is_bearer_token_invalid` 作为 mod.rs 内 pub 函数保留（供 classify_error 的默认实现复用）
  - 将 `inject_profile_arn` 作为 ide.rs 内私有函数保留（供 `build_request` 内部使用）
- **Test cases**：
  - 保留 endpoint/mod.rs 现有 4 个单元测试（测试 `default_is_*` 函数）
  - 新增 IDE endpoint `build_request` 对 GenerateAssistant 的 profile_arn 注入测试（若 1.3 已覆盖则无需重复）
- **Verify**: `cargo build` + `cargo clippy -D warnings` + `cargo test`
- **Complexity**: Small

> **⚠️ 已知遗漏（2026-04-23 code review 发现）**：本步骤删除了旧的多方法接口，但**未同步清理** Step 1.2 为过渡期引入的 `build_request` `unimplemented!()` 默认体。该默认体已完成过渡期使命（阶段 4.1/4.2 切换完成后 `IdeEndpoint` 已完整 override），继续保留会让必填契约伪装成可选覆盖——新端点漏实现时编译期沉默、生产路径 panic。此疏漏在**阶段 6**（follow-up）处理。

#### Step 4.4：阶段 4 验收

- **Verify**:
  - trait 公开方法精确为 `name` / `build_request` / `classify_error`
  - `IdeEndpoint` 仅实现这三个方法
  - 所有回归清单 §5 手工测试通过
- **回滚**：本阶段涉及 trait 定义变更，回滚较复杂。建议本阶段拆为 2 个 commit（4.1+4.2 切换调用 / 4.3 清理 trait），独立回滚。

---

### 阶段 5：调度层收敛 — 合并重试循环 + get_usage_limits 泛化

**目标**：合并 API/MCP 循环为单一 helper；`get_usage_limits_for` 走 `build_request(UsageLimits)`。

**涉及文件**：
- `src/kiro/provider.rs`
- `src/kiro/token_manager.rs`

#### Step 5.1：提取 call_with_retry helper

- **Files**: `src/kiro/provider.rs`
- **Action**:
  - 新增私有方法：
    ```rust
    async fn call_with_retry(
        &self,
        build_req: impl Fn() -> KiroRequest<'_>,  // 或按值传入
        model_for_acquire: Option<String>,
        kind_label: &str,
    ) -> anyhow::Result<reqwest::Response>
    ```
    **签名细节**：`build_req` 因需在循环内多次构造（每次重试需重建 Builder，但 KiroRequest 仅含引用，可直接按值传 copyable 的变体或提供工厂 closure）；实现时选择最简形式 —— 直接按值接收 `KiroRequest<'_>`，单次构造后多次引用。
  - 重写 `call_api_with_retry` 与 `call_mcp_with_retry` 为委托至 helper 的薄壳
- **Test cases**（手工回归）：
  - API/MCP 请求行为与阶段 4 末态完全一致
  - 错误消息中的前缀（`"MCP 请求"` / `"流式 API 请求"` / `"非流式 API 请求"`）正确
- **Verify**: `cargo build` + 手工回归
- **Complexity**: Large

#### Step 5.2：get_usage_limits_for 走 trait

- **Files**: `src/kiro/token_manager.rs`（第 1533-1639 行、第 323-393 行）
- **Action**:
  - `get_usage_limits_for(id)` 改为：
    1. `let ctx = self.acquire_context_for(id).await?;`（已含 endpoint + token + machine_id）
    2. 构造 `RequestContext`
    3. `ctx.endpoint.build_request(client, &rctx, &KiroRequest::UsageLimits)?.send().await?.json::<UsageLimitsResponse>().await?`
  - 删除或收缩 `get_usage_limits` 函数（第 323-393 行）；若仅 `get_usage_limits_for` 使用则整体移除
  - IDE endpoint 的 `build_request(UsageLimits)` 必须构造与原 `get_usage_limits` 字节级一致的请求（host / url / headers / 查询参数）
- **Test cases**：
  - 对 IDE credentials 调用 `get_usage_limits_for(id)`，比对响应与重构前一致
  - 用 mockito 或 wire-mock 构造 HTTP fixture，比对重构前后 request URL/headers 字节级一致（如果已有测试框架则接入；否则手工对比 wireshark / curl 抓包）
- **Verify**: `cargo build` + 手工比对
- **Complexity**: Medium

#### Step 5.3：阶段 5 验收

- **Verify**:
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test`
  - 手工回归 §5 全部 6 类
  - `call_api_with_retry` 与 `call_mcp_with_retry` 均为 ≤20 行的薄壳
- **回滚**：本阶段改动限于 provider.rs + token_manager.rs 的调度层，不影响 trait / registry / CallContext。可独立 revert。

---

### 阶段 6：trait 契约硬化（post-refactor follow-up，2026-04-23 追加）

**触发**：code review 发现 Step 4.3 遗漏了 `build_request` 过渡期 `unimplemented!()` 默认体的清理（见 Step 4.3 末尾遗漏注记）。

**目标**：把 `build_request` 从"可选覆盖 + panic 兜底"固化为"必填抽象方法"，让新端点漏实现在 `cargo build` 阶段报错，而非在生产流量到达 `provider.rs:179` / `token_manager.rs:350` 时 panic 崩 worker。

**涉及文件**：
- `src/kiro/endpoint/mod.rs`（trait 定义 + 测试专用两个 probe）

#### Step 6.1：trait 删除默认体

- **Files**: `src/kiro/endpoint/mod.rs:134-147`
- **Action**:
  - 删除 `build_request` 的默认体与 `unimplemented!()`
  - 方法签名以 `;` 结尾（与 `name()` 一致，成为真正的抽象方法）
  - 删除"默认实现为 `unimplemented!`，具体端点必须 override"的文档注释（契约已在类型签名上自文档化）
- **Verify**: `cargo build` — 预期 `IdeEndpoint` 因已完整实现而通过
- **Complexity**: Trivial

#### Step 6.2：测试 probe 补 stub

- **Files**: `src/kiro/endpoint/mod.rs:253-258`（`ProbeEndpoint`）与 `:376-380`（`NamedProbeEndpoint`）
- **Action**: 给两个测试 probe 各新增：
    ```rust
    fn build_request(
        &self,
        _client: &Client,
        _ctx: &RequestContext<'_>,
        _req: &KiroRequest<'_>,
    ) -> anyhow::Result<RequestBuilder> {
        unreachable!("probe endpoint should never build a request")
    }
    ```
  `unreachable!()` 比 `unimplemented!()` 更准确——表达的是"按测试设计永不应被调用"，而非"暂未实现"。同时删除 `ProbeEndpoint` 内"不 override build_request"的陈旧注释（L257）。
- **Test cases**：
  - 现有 15 个单元测试应全绿（probe 走的是 `classify_error` / `Registry` 路径，不触发 `build_request`）
- **Verify**: `cargo test kiro::endpoint`
- **Complexity**: Trivial

#### Step 6.3：负向验证（可选手工）

- **Action**: 在本地临时写一个仅实现 `name()` 的空 `impl KiroEndpoint`，确认 `cargo build` 报 `not all trait items implemented: missing`build_request\`\` 后删除临时代码——不提交。
- **Complexity**: Trivial

#### Step 6.4：阶段 6 验收

- **Verify**:
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test`
- **回滚**：本阶段为单文件改动，单个 commit，`git revert` 即可；不影响阶段 1-5 的已完成成果。

**Out of Scope（评估后延后）**：
- **合并 `ProbeEndpoint` / `NamedProbeEndpoint` 为单一 `TestEndpoint`**：独立的测试侧重构，避免本次 PR 范围扩散；若未来 trait 再加方法时再做。
- **拆分 `build_request` 为模板方法（`url_for` / `decorate_headers` 等）**：YAGNI，当前只有 1 个 endpoint 实现，抽象收益是假想的，复杂度代价是真实的。待第 2 个 endpoint 落地、差异模式清晰后再评估。

---

### 阶段 7：CallContext 不变量硬化 + get_usage_limits_for 一致性修复（2026-04-24 追加）

**触发**：code review 指出两个问题：
1. `CallContext` 声称"确保 token/credentials/endpoint/machine_id 的一致性"，但字段全 `pub`，任何模块都能用 struct literal 构造不一致的实例——类型系统未守住声称的不变量。
2. `get_usage_limits_for` 中 token 来自一次读取、credentials/endpoint/machine_id 来自另一次读取，存在并发刷新导致快照不一致的真实竞态窗口。

**设计决策**：
- `CallContext`：字段全私有 + `pub(crate)` getter，编译器保证只有 `token_manager.rs` 模块内的 `try_ensure_token` 能构造。
- `RequestContext`：字段保持 `pub`——它是引用视图包，无独立不变量，一致性由上游 `CallContext` 保证；私有化会引入大量 getter churn（`ide.rs` 约 15 处直接访问 + 4 处测试构造）。
- 新增 `CallContext::to_request_context(&self, config) -> RequestContext`，统一从 CallContext → RequestContext 的转换路径。
- 不加 `CallContext::new()` 构造器——字段私有后 struct literal 仅模块内可用，`new()` 只是同模块内的搬运，零额外安全收益。

**涉及文件**：
- `src/kiro/token_manager.rs`
- `src/kiro/provider.rs`

#### Step 7.1：CallContext 字段私有化 + getter

- **Files**: `src/kiro/token_manager.rs`
- **Action**:
  - struct 改为 `pub(crate)`，字段去掉 `pub`
  - 新增 getter：`id() -> u64`、`credentials() -> &KiroCredentials`、`token() -> &str`、`endpoint() -> Arc<dyn KiroEndpoint>`（返回 cloned Arc）、`machine_id() -> &str`
  - 新增 `to_request_context(&self, config) -> RequestContext`
  - `acquire_context` 可见性收紧为 `pub(crate)`
- **Verify**: `cargo build` 报 provider.rs 字段访问错误（下一步修复）
- **Complexity**: Small

#### Step 7.2：provider 消费侧迁移

- **Files**: `src/kiro/provider.rs`
- **Action**:
  - 所有 `ctx.field` → `ctx.field()` getter 调用
  - `RequestContext { ... }` 手写构造 → `ctx.to_request_context(config)`
  - 401/403 分支提取 `let id = ctx.id();` 避免重复调用
- **Verify**: `cargo build` + `cargo test` 通过
- **Complexity**: Small

#### Step 7.3：get_usage_limits_for 复用 try_ensure_token

- **Files**: `src/kiro/token_manager.rs`
- **Action**:
  - 删除 `get_usage_limits_for` 内自行编写的 ~60 行 token 刷新逻辑（token 与 credentials 分两次从 entries 读取）
  - 改为 `let ctx = self.try_ensure_token(id, &credentials).await?;`，后续所有参数从 `ctx` getter 提取
  - 行为差异：`try_ensure_token` 成功后会重置 `refresh_failure_count`，原 `get_usage_limits_for` 未做此操作——这是改进（成功获取 token 应清零计数，不区分请求类型）
- **Verify**: `cargo test token_manager` 通过
- **Complexity**: Small

#### Step 7.4：阶段 7 验收

- **Verify**:
  - `cargo check` 通过
  - `cargo test`（229 测试全过）
  - `CallContext` 字段仅在 `try_ensure_token` 的两个分支（API key / OAuth）中以 struct literal 构造
  - `provider.rs` 无直接字段访问残留
  - `get_usage_limits_for` 无二次 entries 读取残留
- **回滚**：两文件改动，单个 commit `8af1386`，`git revert` 即可。

---

## Test Strategy

### Automated Tests

| Test Case | Type | Input | Expected Output |
|-----------|------|-------|-----------------|
| `EndpointErrorKind` 枚举变体 Debug | Unit | 每个变体 | Debug 输出含变体名 |
| `classify_error` 默认实现 402+monthly | Unit | status=402, body=含 `MONTHLY_REQUEST_COUNT` | `MonthlyQuotaExhausted` |
| `classify_error` 默认实现 401+bearer invalid | Unit | status=401, body=含 `"The bearer token ... is invalid"` | `BearerTokenInvalid` |
| `classify_error` 默认实现 400 | Unit | status=400, body=任意 | `BadRequest` |
| `classify_error` 默认实现 429 | Unit | status=429 | `Transient` |
| `classify_error` 默认实现 500 | Unit | status=500 | `Transient` |
| `classify_error` 默认实现 402 不含标记 | Unit | status=402, body=空 JSON | `ClientError` |
| `classify_error` 默认实现 404 | Unit | status=404 | `ClientError` |
| `classify_error` 默认实现 200 | Unit | status=200 | `Unknown` |
| `EndpointRegistry::new` default 缺失 | Unit | endpoints={ide}, default="cli" | Err |
| `EndpointRegistry::new` OK | Unit | endpoints={ide}, default="ide" | Ok |
| `EndpointRegistry::resolve` 命中指定 | Unit | credentials.endpoint=Some("ide") | Arc(IdeEndpoint) |
| `EndpointRegistry::resolve` 命中默认 | Unit | credentials.endpoint=None | Arc(IdeEndpoint) |
| `EndpointRegistry::resolve` 未知降级 | Unit | credentials.endpoint=Some("xxx") | Arc(IdeEndpoint) + warn |
| `EndpointRegistry::contains` | Unit | "ide" / "cli" | true / false |
| `IdeEndpoint::build_request(GenerateAssistant)` URL | Unit | 典型 ctx + body | URL 含 `generateAssistantResponse` |
| `IdeEndpoint::build_request(Mcp)` URL | Unit | 典型 ctx + body | URL 含 `/mcp` |
| `IdeEndpoint::build_request(UsageLimits)` URL 一致性 | Unit | 典型 ctx | 与重构前 `get_usage_limits` 构造的 Request 字节级相等 |
| `IdeEndpoint::build_request(GenerateAssistant)` profile_arn 注入 | Unit | credentials.profile_arn=Some("arn:...") | body JSON 含 profileArn |
| `AdminService::add_credential` 未知 endpoint | Unit | endpoint=Some("unknown") | InvalidCredential("未知端点 ...") |
| `AdminService::add_credential` 默认 endpoint | Unit | endpoint=None | 不因 endpoint 校验失败 |
| `CallContext` 含 endpoint + machine_id | Unit | acquire_context 返回 | 字段非空且 endpoint.name() 匹配 |

### Manual Verification

- [ ] IDE 凭据正常 API 请求：请求成功返回 200
- [ ] IDE 凭据 MCP 请求：请求成功返回 200
- [ ] IDE 凭据 `get_usage_limits_for`：返回的 JSON schema 与重构前一致
- [ ] 模拟 402 + MONTHLY_REQUEST_COUNT：凭据被禁用、切换下一张
- [ ] 模拟 402 但 body 无标记：走 ClientError arm → 直接 bail
- [ ] 模拟 401 + bearer invalid：触发 force_refresh；第二次 401 不再 force_refresh 而 report_failure
- [ ] 模拟 400：直接 bail，不切换凭据
- [ ] 模拟 429：sleep + 重试，凭据不被禁用
- [ ] 模拟网络错误：重试，凭据不被禁用
- [ ] 启动时 default_endpoint 不存在：进程退出，错误消息格式与重构前一致
- [ ] 启动时 credentials.endpoint 不存在：进程退出，错误消息格式与重构前一致
- [ ] admin API `POST /admin/credentials` 传未知 endpoint：返回 InvalidCredential 错误，消息格式不变
- [ ] priority 模式下凭据轮转顺序：与重构前一致
- [ ] balanced 模式下凭据选择：与重构前一致

---

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|-----------|
| trait 方法切换漏改调用点 | provider 调用 unimplemented! panic | 阶段 1 `build_request` 默认实现是 `unimplemented!` 但 IDE 已 override；阶段 4.3 删除旧方法前 grep 确认无调用。**2026-04-23 补注**：该 mitigation 仅覆盖切换期风险，未覆盖"新 endpoint 漏实现"的长期风险——阶段 6 （follow-up）删除默认体引入编译期契约保护。 |
| CallContext 字段增加破坏 Clone 开销 | 请求延迟微增 | Arc<dyn KiroEndpoint> 与 String 均为廉价 Clone；machine_id 在 token_manager 预计算，provider 仅读取 |
| get_usage_limits_for 泛化后 URL/header 字节不一致 | 上游 400/401 | 阶段 5.2 使用 wire fixture 比对或手工对比；若无工具则以阶段 4 的请求日志为基线 |
| AdminService 错误消息格式变化 | 调用方解析失败 | 决策 4 明确消息格式由 admin_service 组装；测试用例验证字节级一致 |
| EndpointRegistry.resolve 未知降级 vs 启动校验冲突 | 若启动校验未捕获会路由到 default | 启动时严格校验（沿用原进程退出行为）；resolve 降级 + warn 仅作防御性兜底 |
| 阶段 5 helper 泛型复杂度 | 难以阅读 | helper 参数按值接受 KiroRequest，不使用 `impl Fn`；如方案超过 200 行则拆为两个具体函数 |
| force_refreshed Set 语义丢失 | 401/403 无限 refresh | 阶段 4.1/4.2 切换时确保 Set 仍在循环外；新增单测无法覆盖，依赖手工回归 |

---

## Rollback Strategy

[CC] 分阶段回滚：

- **阶段 1**：仅新增，无副作用 → `git revert <phase1-commit>`
- **阶段 2**：registry 替换 3 处副本 → `git revert <phase2-commits>`，恢复 HashMap/HashSet
- **阶段 3**：CallContext 结构变更 → `git revert <phase3-commits>`，provider 回到 endpoint_for + generate_from_credentials
- **阶段 4**：trait 旧方法已删除 → 需 revert 阶段 1/4 的 commits 成对；建议阶段 4.3 单独 commit，便于仅回滚 4.3
- **阶段 5**：仅 provider/token_manager 内部重构 → `git revert <phase5-commits>`

**高风险场景**：若阶段 4.3 删除旧方法后发现回归，且阶段 5 已合入，则需按 5 → 4.3 → 4.2 → 4.1 逆序回滚。**因此阶段 4.3 必须在完整手工回归通过后独立 commit 并单独验证 24h 稳定性，再推进阶段 5**。

---

## Status

- [x] Plan approved
- [x] Phase 1 complete — `d66370e`
- [x] Phase 2 complete — `edd3e21`
- [x] Phase 3 complete — `2d7cbfc`
- [x] Phase 4 complete — `6f13323`（4.1+4.2）/ `d0b77e2`（4.3 独立）
- [x] Phase 5 complete — `21bfe7a`
- [x] Implementation complete（阶段 1-5；隔离 worktree：`.claude/worktrees/agent-af695560`，分支 `worktree-agent-af695560`）
- [ ] Phase 6 (post-refactor follow-up): trait 契约硬化 — 2026-04-23 code review 追加，详见上方阶段 6

---

## 附录 A：阶段依赖图

```
[阶段 1: 协议层扩展（新增，不破坏）]
          │
          ▼
[阶段 2: 路由层 EndpointRegistry] ────┐
          │                              │
          ▼                              │
[阶段 3: CallContext + TokenManager 注入 registry]
          │
          ▼
[阶段 4: provider 切换 build_request + classify_error] 
          │                      (4.3 必须在 4.1/4.2 手工回归通过后单独 commit)
          ▼
[阶段 5: 合并重试循环 + get_usage_limits 泛化]
          │
          ▼
[阶段 6: trait 契约硬化（post-refactor follow-up）]
```

**关键约束**：
- 阶段 1 不依赖任何其他阶段
- 阶段 2 不依赖阶段 1（但推荐同 PR/同分支推进以便统一测试）
- 阶段 3 依赖阶段 2（CallContext 需要 registry 预解析）
- 阶段 4 依赖阶段 1（需要 build_request 实现）+ 阶段 3（需要 ctx.endpoint）
- 阶段 5 依赖阶段 4（需要 build_request 为唯一路径）
- **阶段 6 依赖阶段 4.3**（Step 1.2 引入的 `unimplemented!()` 默认体在 4.3 未清理；阶段 6 负责彻底删除）

## 附录 B：研究文档引用对照

本计划决策对研究文档的覆盖关系：
- 研究 §1.1 的 RequestContext 简化版本 → **本计划覆盖**，保留 machine_id（用户决策 3）
- 研究 §1.2 EndpointRegistry 设计 → **本计划采纳**（用户决策 4 扩展 contains/names）
- 研究 §1.3 CallContext 增强 → **本计划采纳并扩展** machine_id 字段
- 研究 §3 缺陷映射表 7 条 → 全部覆盖，分配至阶段 1-5
- 研究 §4.1 CLI 实现 → **本轮不实现**（用户决策 1）
- 研究 §4.2 EndpointErrorKind → **本计划固化** 6 变体语义（用户决策 2）
- 研究 §4.3 machine_id 去留 → **保留**（用户决策 3）
- 研究 §4.4 校验归属 → **admin_service 保留业务层**（用户决策 4）
- 研究 §4.5 合并重试循环 → **阶段 5 必做项**（用户决策 5）
- 研究 §5 回归清单 → **本计划"必须保持不变的行为"节照抄**
