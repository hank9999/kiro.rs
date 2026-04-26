# Plan: refactor/v2 修复计划（2026-04-26）

> 配套研究：[`@docs/plans/2026-04-26-refactor-v2-fixes-research.md`](2026-04-26-refactor-v2-fixes-research.md)

## Summary

研究报告对 `refactor/v2` 提出 REQUEST CHANGES：6 项 P0 阻塞、8 项 P1 重要、8 项 P2 次要，共 22 项偏离/回归。本计划把这 22 项归并到 6 个阶段（Phase A–F）依次推进；每阶段独立可验证、可单独合入，整体落地后让分支达到原 Plan 2026-04-25 的成功标准。修复重点：消灭并发与数据完整性回归（A），打通结构化错误体系（B），消灭 handler 双胞胎与模型/CLI 内联（C），落实 Phase 4 的 stream/converter 真实拆分（D），收敛运行时质量（E），同步文档与清理（F）。

## Stakes Classification

**Level**: High

**Rationale**:
- Phase A 涉及凭据池并发刷新与启动期数据校验，错误实现会造成生产事故（refresh token 误丢失、凭据被错误标记禁用）。
- Phase B/C/D 改动触及核心请求路径（handler / stream / 错误映射），对外 HTTP 协议契约不能变更，回归代价大。
- Phase E1 的 30s debounce 改写涉及异步任务生命周期，错误实现会丢失最后一次 stats。
- 单文件改动行数从 50（A1）到 ~1200（D 全套）不等，整体改动量大，影响面广。

## Context

**研究文档**：`docs/plans/2026-04-26-refactor-v2-fixes-research.md`（已生效）

**主要受影响文件（按改动量降序）**：

| 文件 | 当前行数 | 改动 | 阶段 |
|------|---------|------|------|
| `src/service/conversation/stream.rs` | 1977 | 拆出到 reducer.rs / delivery.rs / thinking.rs | D |
| `src/service/conversation/converter.rs` | 1767 | 拆出到 tools.rs | D |
| `src/service/credential_pool/pool.rs` | 1197 | single-flight + apply_refresh + debounce + stable order | A/E |
| `src/interface/http/anthropic/handlers.rs` | 955 | 错误映射重写 + 双胞胎合并 + 模型列表拆出 | B/C |
| `src/service/conversation/websearch.rs` | 761 | 移除 anyhow | B |
| `src/domain/credential.rs` | 676 | 新增 apply_refresh 方法 | A |
| `src/service/credential_pool/store.rs` | 360 | 重复 id 启动检测 | A |
| `src/domain/retry.rs` | 31 | DisabledReason 增加 Manual / TooManyRefreshFailures | A |
| `src/infra/http/retry.rs` | 198 | DefaultRetryPolicy 识别 ContextWindowFull/InputTooLong | B |
| `src/service/kiro_client.rs` | 96 | call_api/call_api_stream 合并 | E |
| `src/main.rs` | 276 | Args 拆出 + API key 日志收敛 + proxy 集中调用 | F |
| `src/config/proxy.rs` | - | 新增 to_proxy_config() 方法 | F |
| `src/interface/cli.rs` | 0（新建） | clap Args 定义 | F |
| `src/interface/http/error.rs` | 0（新建） | KiroError → axum::Response 映射 | B |
| `src/interface/http/anthropic/models.rs` | 0（新建） | 模型列表常量 | C |
| `README.md` | - | 删除 countTokens* 三行 + 完整示例 3 行 | F |
| `CHANGELOG.md` | 0（新建） | 标注 Breaking Change | F |

**关键约束**（继承自 CLAUDE.md / 锁顺序文档）：
- 锁顺序固定 `store → state → stats`，禁止反向获取。
- 凭据池禁止跨 `.await` 持锁。
- 对外 HTTP 协议（请求/响应字段名、SSE 事件序列、admin JSON shape）严禁 breaking change。
- credential.json schema 仅允许新增字段（向前兼容 `serde(default)`）。

## Success Criteria

完成时全部满足，且 `cargo test` 测试用例数显著增长（至少 +25 项，覆盖新增并发/错误映射场景）。

- [ ] 全部 22 个 issue 修复或显式标注"有意 breaking"
- [ ] `cargo clippy --all-targets -- -D warnings` 通过，且 `src/` 下不再有 `#![allow(dead_code)]`（仅 `parser/decoder.rs` 因复杂度可保留）
- [ ] `cargo test` 全绿；新增覆盖：`map_provider_error → Response`、`prepare_token` 并发、重复 id 启动拒绝、stats debounce、priority 稳定排序
- [ ] `grep -rE 'msg\.contains|err_str\.contains' src/` 仅命中允许位置（`infra/refresher/mod.rs::classify_refresh_http_error` 中 `body.contains("\"invalid_grant\"")` 为有意保留）
- [ ] `grep -rE 'anyhow' src/` 仅命中 `Cargo.toml` 与必要的旁路（websearch 移除后理想为零）
- [ ] 所有单文件 ≤ 600 行（特例：`infra/parser/decoder.rs`、`domain/credential.rs` 含 30+ 测试可超）
- [ ] 与 master 启一次冒烟：`/v1/messages` stream/non-stream + `/cc/v1/messages` + `/v1/models` + `/api/admin/credentials` 全部 200，事件序列与 master 字节级对齐
- [ ] README + CHANGELOG 同步生效

---

## Implementation Steps

### Phase A：数据完整性 + 并发安全（P0）

**目标**：消灭研究报告 §3.5 / §3.6 / §4.5 / §4.7 五个运行时回归。Phase A 独立于其它 phase，可率先合入降低风险。

#### Step A1.1: 重复 credential id 启动检测（RED）

- **Files**: `src/service/credential_pool/store.rs`（新增测试）
- **Action**: 添加失败测试 `load_rejects_duplicate_ids`：构造 `[{"id":1,...},{"id":1,...}]` 文件，调 `CredentialStore::load`，断言返回 `Err(ConfigError::Validation(_))`，错误信息包含 "duplicate id"。
- **Test cases**:
  - `[{"id":1,...},{"id":1,...}]` → `Err`，message 含 "duplicate id 1"
  - `[{"id":1,...},{"id":2,...}]` → `Ok`（控制组）
  - `[{...},{...}]`（均无 id，自动分配） → `Ok`（不应误报）
- **Verify**: `cargo test load_rejects_duplicate_ids` 失败（实现尚未添加）
- **Complexity**: Small

#### Step A1.2: 重复 credential id 启动检测（GREEN）

- **Files**: `src/service/credential_pool/store.rs:78-102`
- **Action**: 在 `CredentialStore::load` 第 4 步末尾、构造 HashMap 之前，遍历 creds 检查 `id` 是否重复。检测到重复返回 `ConfigError::Validation(format!("credentials.json 含重复 id {dup}：拒绝启动以避免凭据被静默覆盖"))`。
- **Verify**: A1.1 三个测试全部 pass。
- **Complexity**: Small

#### Step A2.1: 启动时初始化 current_id（RED）

- **Files**: `src/service/credential_pool/pool.rs`（新增测试）
- **Action**: 测试 `current_id_initialized_to_lowest_priority_after_install_initial_states`：调 `pool_with_n_credentials(3, MODE_PRIORITY)` 后立即（不调用 acquire）读取 `pool.admin_snapshot().current_id`，断言不是 `0` 而是优先级最低（priority=0）的凭据 id。
- **Test cases**:
  - 3 凭据 priority=0/1/2 → `current_id == id_of(priority=0)`
  - 全部 disabled → `current_id == 0`（保持当前行为，无可用凭据时为 0）
  - balanced 模式 → `current_id == 0`（balanced 不维护 current_id）
- **Verify**: 测试失败（当前 `Mutex::new(None)` 未在 install_initial_states 触发）
- **Complexity**: Small

#### Step A2.2: 启动时初始化 current_id（GREEN）

- **Files**: `src/service/credential_pool/pool.rs:411-430`（`install_initial_states` 末尾）
- **Action**: `install_initial_states` 末尾调 `self.select_highest_priority()`（已有方法，priority 模式才生效）。
- **Verify**: A2.1 测试 pass；既有所有测试不回归。
- **Complexity**: Small

#### Step A3.1: per-credential refresh single-flight 锁（RED）

- **Files**: `src/service/credential_pool/pool.rs`（新增测试）
- **Action**: 写并发测试 `prepare_token_serializes_concurrent_refresh_for_same_credential`，使用 mock refresher（计数自增），用 `tokio::join!` 同时触发两个 acquire 触发 refresh 同一凭据（凭据 expires_at 设为已过期），断言 mock refresher 仅被调用 1 次。
- **Test cases**:
  - 2 个 task 并发 acquire 同一 priority=0 凭据，过期 → refresh count == 1，两次都拿到相同 access_token
  - 不同凭据并发 acquire → 各自独立 refresh，互不阻塞（计数 = 凭据数）
  - 一次 acquire refresh 失败但已写回 token，第二次 acquire 应直接复用新 token（不重复 refresh）
- **Verify**: 测试失败（当前两个 task 都各自调 refresh）
- **Complexity**: Medium（mock refresher trait 需新建或借用现有 abstraction）

#### Step A3.2: per-credential refresh single-flight 锁（GREEN）

- **Files**: `src/service/credential_pool/pool.rs:54-97, 311-350, 386-408, 768-801`
- **Action**:
  1. 在 `CredentialPool` 增加 `refresh_locks: parking_lot::Mutex<HashMap<u64, Arc<tokio::sync::Mutex<()>>>>` 字段（按 id 维护刷新锁）；新建 helper `refresh_guard_for(id)` 返回 `Arc<tokio::sync::Mutex<()>>`。
  2. `prepare_token` 在确认需要 refresh 后：
     - `let lock = self.refresh_guard_for(id);`
     - `let _g = lock.lock().await;`
     - **二次检查**：拿锁后 `self.store.get(id)` 重读凭据，若未过期 → 直接返回新值（避免重复刷新）。
     - 仍过期 → 走 refresher.refresh + apply_refresh + store.replace。
  3. 同样模式应用到 `force_refresh`、`prepare_token_for_admin`、`force_refresh_token_for`、`add_credential`（add_credential 由于 id 尚未分配可不加锁，唯一性由 store.add 内部保证）。
  4. delete_credential 时移除 `refresh_locks` 中对应 id 的项（避免内存泄漏）。
- **Verify**: A3.1 三个测试 pass；既有所有测试不回归；并发场景下 refresh count 严格 == 1。
- **Complexity**: Medium

#### Step A4.1: Credential::apply_refresh 抽取（RED）

- **Files**: `src/domain/credential.rs`（新增测试）
- **Action**: 新增测试 `apply_refresh_overwrites_access_token_and_optional_fields`：`Credential::default()` 起点，调 `apply_refresh(&outcome)`（含全部字段），断言四字段全部更新；再用 `RefreshOutcome { access_token: "x", refresh_token: None, profile_arn: None, expires_at: None }` 调用，断言仅 access_token 更新，其他保留。
- **Test cases**:
  - 完整 outcome → 4 字段全更新
  - 只有 access_token 的 outcome → 仅 access_token 更新，其他字段保持原值
  - profile_arn 原本有值，新 outcome 不带 profile_arn → 保留原值（不清空）
- **Verify**: 测试失败（方法不存在）
- **Complexity**: Small

#### Step A4.2: Credential::apply_refresh 抽取 + 调用点替换（GREEN）

- **Files**:
  - `src/domain/credential.rs`（新方法）
  - `src/service/credential_pool/pool.rs:336-347, 394-405, 666-675, 731-741, 788-798`（5 处替换）
- **Action**:
  1. 在 `impl Credential` 增加 `pub fn apply_refresh(&mut self, outcome: &RefreshOutcome)` 方法，移植研究报告 §4.4 的逻辑。
  2. 把 5 个调用点改为 `updated.apply_refresh(&outcome);`。
  3. 同步处理 `store.replace` 失败：当前用 `let _ = self.store.replace(...)` 静默忽略；改为 `if let Err(e) = self.store.replace(...) { tracing::error!(?e, id, "刷新成功但持久化失败，凭据已更新到内存"); }`，仍返回 Ok（避免单次磁盘失败导致请求失败）。
- **Verify**: A4.1 测试 pass；既有 pool 测试全绿；`grep -c 'updated\.access_token = Some(outcome' src/service/credential_pool/pool.rs` == 0。
- **Complexity**: Small

#### Step A5.1: DisabledReason 恢复 Manual + TooManyRefreshFailures（RED）

- **Files**: `src/domain/retry.rs`、`src/service/credential_pool/state.rs`、`src/service/credential_pool/admin.rs`（测试）
- **Action**: 写以下测试：
  - `domain::retry::tests::disabled_reason_includes_manual_and_too_many_refresh_failures`：断言 `DisabledReason` 含 `Manual` 与 `TooManyRefreshFailures` 变体（编译期 pattern match）。
  - `state::tests::set_disabled_true_assigns_manual_reason`：调 `set_disabled(id, true)`，读 `state.get(id).disabled_reason`，断言 `== Some(Manual)`。
  - `state::tests::report_refresh_failure_after_threshold_uses_too_many_refresh_failures`：连续 3 次 `report_refresh_failure`，断言 `disabled_reason == Some(TooManyRefreshFailures)`。
  - `state::tests::heal_too_many_failures_does_not_heal_too_many_refresh_failures`：refresh 失败禁用的凭据不应被自愈（与现行 `heal` 行为一致，但 reason 区分）。
  - `pool::tests::admin_snapshot_serializes_manual_reason`：手动 `set_disabled(id, true)` 后 `admin_snapshot().entries[i].disabled_reason == Some("Manual".into())`。
- **Test cases**: 5 项（见 Action）。
- **Verify**: 5 个测试均失败（变体未引入）。
- **Complexity**: Small

#### Step A5.2: DisabledReason 恢复 Manual + TooManyRefreshFailures（GREEN）

- **Files**:
  - `src/domain/retry.rs:11-17`（新增变体）
  - `src/service/credential_pool/state.rs:101-134, 149-164`（refresh 失败 reason 与 manual 区分）
  - `src/service/credential_pool/pool.rs:889-896`（disabled_reason_to_str 新增分支）
  - `src/service/admin/error.rs`（如有 reason 字符串映射，同步）
- **Action**:
  1. `DisabledReason` 增加 `Manual` 与 `TooManyRefreshFailures` 变体。
  2. `state::report_refresh_failure` 第 106 行 `Some(DisabledReason::TooManyFailures)` 改为 `Some(DisabledReason::TooManyRefreshFailures)`。
  3. `state::set_disabled(id, true)` 把 `disabled_reason = Some(Manual)`（false 路径仍 None）。
  4. `state::heal_too_many_failures` 仅自愈 `TooManyFailures`（保持当前行为，refresh 失败/manual 不参与自愈）。
  5. `pool::disabled_reason_to_str` 增加 `Manual => "Manual"`、`TooManyRefreshFailures => "TooManyRefreshFailures"` 分支。
- **Verify**: A5.1 5 个测试 pass；既有 state/pool 测试不回归；admin DTO 测试若依赖具体字符串需同步更新。
- **Complexity**: Small

#### Phase A 检查点

- [x] `cargo test` 全绿（基线 289 → 309，新增 20 个测试）
- [x] `cargo clippy --all-targets -- -D warnings` 通过
- [x] `grep -nE 'updated\.access_token = Some\(outcome' src/` 命中 0 次
- [ ] 手动并发冒烟：使用 `wrk -c 8 -t 4` 打 `/v1/messages`，监控日志中 refresh 调用次数 ≤ 凭据数（待人工执行）

---

### Phase B：错误体系真正打通（P0）

**目标**：消灭 §3.1 / §3.2 / §4.8。让 ProviderError → HTTP Response 走结构化 enum match，删除字符串扫描。

#### Step B1.1: DefaultRetryPolicy 识别 ContextWindowFull / InputTooLong（RED）

- **Files**: `src/infra/http/retry.rs`（新增测试）
- **Action**: 写测试：
  - `decide_400_with_content_length_exceeds_threshold_is_fail_context_window_full`：status=400，body 含 "CONTENT_LENGTH_EXCEEDS_THRESHOLD"，期望 `RetryDecision::Fail(ProviderError::ContextWindowFull)`。
  - `decide_400_with_input_is_too_long_is_fail_input_too_long`：status=400，body 含 "Input is too long"，期望 `Fail(InputTooLong)`。
  - `decide_400_other_keeps_upstream_http`：控制组，status=400 普通 body → `Fail(UpstreamHttp { status: 400, ... })`。
- **Test cases**: 3 项。
- **Verify**: 前 2 个测试失败。
- **Complexity**: Small

#### Step B1.2: DefaultRetryPolicy 识别 ContextWindowFull / InputTooLong（GREEN）

- **Files**: `src/infra/http/retry.rs:67-79`（client_error 分支前插入识别）
- **Action**: 在 "其他 4xx" 分支之前增加：
  ```rust
  if s == 400 && body.contains("CONTENT_LENGTH_EXCEEDS_THRESHOLD") {
      return RetryDecision::Fail(ProviderError::ContextWindowFull);
  }
  if s == 400 && body.contains("Input is too long") {
      return RetryDecision::Fail(ProviderError::InputTooLong);
  }
  ```
  这是 plan 允许的"识别在 infra 层、不在 interface 层"——retry policy 内部 `body.contains` 是输入判定而非错误识别字符串扫描。
- **Verify**: B1.1 3 个测试 pass。
- **Complexity**: Small

#### Step B2.1: 新增 interface/http/error.rs（RED）

- **Files**: `src/interface/http/error.rs`（新建）
- **Action**: 先以 `#[cfg(test)]` 块声明测试用例（实现暂留 todo!）：
  - `kiro_error_into_response_context_window_full_400_with_invalid_request_error_type`
  - `kiro_error_into_response_input_too_long_400`
  - `kiro_error_into_response_all_credentials_exhausted_502_with_api_error_type`
  - `kiro_error_into_response_endpoint_resolution_503`
  - `kiro_error_into_response_bad_request_400_with_message`
  - `kiro_error_into_response_upstream_http_502`
  - `kiro_error_into_response_refresh_token_invalid_502`
  - `kiro_error_into_response_default_502`
- **Test cases**: 8 项，每项构造 KiroError → 调 `into_response()` → 断言 `status_code` 与 body 中 `error.type` / `error.message` 字段。
- **Verify**: 测试编译失败（文件 / 函数尚未存在）。
- **Complexity**: Small

#### Step B2.2: 实现 interface/http/error.rs（GREEN）

- **Files**:
  - `src/interface/http/error.rs`（新建）
  - `src/interface/http/mod.rs`（添加 `pub mod error;`）
- **Action**:
  1. 公开 `pub fn kiro_error_response(err: &KiroError) -> Response`（或直接 `impl IntoResponse for KiroError` 包装类型 `ProviderHttpError`）。
  2. `match err` 实现：
     - `Provider(ContextWindowFull)` → 400, type=`invalid_request_error`, message="Context window is full. Reduce conversation history, system prompt, or tools."
     - `Provider(InputTooLong)` → 400, type=`invalid_request_error`, message="Input is too long. Reduce the size of your messages."
     - `Provider(BadRequest(msg))` → 400, type=`invalid_request_error`, message=msg
     - `Provider(EndpointResolution(msg))` → 503, type=`api_error`, message=msg
     - `Provider(AllCredentialsExhausted { available, total })` → 502, type=`api_error`, message=`format!("All {total} credentials exhausted ({available} available)")`
     - `Provider(UpstreamHttp { status, body })` → 502, type=`api_error`, message=body 截断 512 字节
     - `Refresh(TokenInvalid)` → 502, type=`api_error`, message="Refresh token invalid"
     - 其他 → 502, type=`api_error`, message=err.to_string()
  3. body schema 与现有 `ErrorResponse::new` 完全对齐（字段名 `type`/`message` 在 `error` 嵌套对象中，参考 dto.rs:ErrorResponse）。
- **Verify**: B2.1 8 个测试 pass。
- **Complexity**: Small

#### Step B2.3: handlers.rs 错误映射切换为结构化（RED）

- **Files**: `src/interface/http/anthropic/handlers.rs`（新增测试）
- **Action**: 把 handler 模块内现有错误映射相关的（隐式）测试覆盖改为：
  - `handler_post_messages_returns_400_for_context_window_full`：mock provider.call_api 返回 `Err(ProviderError::ContextWindowFull)`，断言响应 status 400 且 body type=`invalid_request_error`。（如果 mock 复杂可暂用集成测试；最少要在 axum 层走 `Response` 解析）
  - `handler_post_messages_returns_503_for_endpoint_resolution`
  - `handler_post_messages_returns_502_for_upstream_http`
- **Test cases**: 3 项最小集合（mock 难度大可适度简化为函数级测试 `map_provider_error(err) → status 校验`）。
- **Verify**: 测试失败（旧映射对 EndpointResolution 走 502 兜底而非 503）。
- **Complexity**: Medium

#### Step B2.4: handlers.rs 错误映射切换为结构化（GREEN）

- **Files**: `src/interface/http/anthropic/handlers.rs:1-85, 331, 467, 847`（map_provider_error* 全套）
- **Action**:
  1. 删除 `map_provider_error_anyhow / map_provider_error_inner / map_provider_error_legacy` 三个函数与 `use anyhow::Error;`、`#[allow(dead_code)]`。
  2. `fn map_provider_error(err: ProviderError) -> Response` 改为：先把 ProviderError 包成 `KiroError::Provider(err)`，再调 `crate::interface::http::error::kiro_error_response(&kiro)`。
  3. 调用点（`return map_provider_error(e);` × 3 处）保持不变。
- **Verify**: B2.3 3 个测试 pass；既有所有 handler 测试不回归；`grep -c 'msg\.contains\|err_str\.contains' src/interface/http/anthropic/handlers.rs` == 0。
- **Complexity**: Small

#### Step B3.1: websearch.rs 移除 anyhow（RED）

- **Files**: `src/service/conversation/websearch.rs`（新增测试）
- **Action**:
  - `call_mcp_api_returns_provider_error_when_provider_fails`：mock provider.call_mcp 返回 `Err(ProviderError::UpstreamHttp { ... })`，期望 `call_mcp_api` 返回 `Err(ProviderError::UpstreamHttp { ... })`。
  - `call_mcp_api_returns_provider_error_when_response_contains_mcp_error`：响应 body 含 `"error": {...}`，期望返回 `Err(ProviderError::UpstreamHttp { status: 200, body: <serialized> })`（因为业务上是 MCP 错误而非 HTTP 错误，code 与 message 转字符串作为 body）。
- **Test cases**: 2 项。
- **Verify**: 测试失败（当前签名是 `anyhow::Result<McpResponse>`，无法返回 ProviderError）。
- **Complexity**: Small

#### Step B3.2: websearch.rs 移除 anyhow（GREEN）

- **Files**: `src/service/conversation/websearch.rs:521-547`（call_mcp_api）+ 调用点 `let search_results = match call_mcp_api(...).await { ... }`
- **Action**:
  1. `call_mcp_api` 签名改为 `Result<McpResponse, ProviderError>`：
     - `serde_json::to_string(request)` 失败 → `ProviderError::BadRequest(format!("MCP serialize: {e}"))`
     - `provider.call_mcp` 失败 → 直接 `?`（已是 ProviderError）
     - `response.text` 失败 → `ProviderError::UpstreamHttp { status: 0, body: e.to_string() }`
     - `serde_json::from_str` 失败 → `ProviderError::UpstreamHttp { status: 200, body: format!("MCP malformed: {e}") }`
     - `mcp_response.error.is_some()` → `ProviderError::UpstreamHttp { status: 200, body: format!("MCP error code={code} msg={msg}") }`
  2. 调用点：当前是 `Ok(response) => parse_search_results(&response), Err(e) => { tracing::warn!(...); None }`，保持调用点不变（错误降级为 None，不影响输出）。但 `tracing::warn!("MCP API 调用失败: {}", e)` 中 `e` 类型变更不影响 Display。
- **Verify**: B3.1 测试 pass；`grep -c 'anyhow' src/service/conversation/websearch.rs` == 0。
- **Complexity**: Small

#### Phase B 检查点

- [x] `cargo test` 全绿（330 测试，Phase A 309 → 330，新增 21 个）
- [x] `grep -rE 'msg\.contains|err_str\.contains' src/` 剩余命中均为测试断言或文档注释，无错误识别字符串扫描
- [x] `grep -c '#\[allow(dead_code)\]' src/interface/http/anthropic/handlers.rs` == 0（map_provider_error_legacy 已删除）
- [x] `grep -rc anyhow src/` 仅 `domain/error.rs:3` + `service/credential_pool/admin.rs:11` 文档注释（websearch.rs 已清空）
- [ ] 手动冒烟：构造超长输入触发 ContextWindowFull → curl 返回 400 + invalid_request_error；强制 endpoint 不存在触发 EndpointResolution → 503（待人工执行）

---

### Phase C：handler 层去重（P0）

**目标**：消灭 §3.4 / §5.2 / §5.3。把 post_messages / post_messages_cc 收敛、CLI Args 拆出、模型列表拆出。

依赖：Phase B 完成（错误映射统一），否则双胞胎合并后错误路径会跟着重复。

#### Step C1.1: 模型列表拆到 anthropic/models.rs（RED+GREEN 合并，纯搬家）

- **Files**:
  - `src/interface/http/anthropic/models.rs`（新建）
  - `src/interface/http/anthropic/mod.rs`（pub mod 声明）
  - `src/interface/http/anthropic/handlers.rs:90-190`
- **Action**:
  1. 把 `get_models` 的模型列表常量提到新文件 `pub static MODEL_LIST: Lazy<Vec<Model>> = Lazy::new(|| vec![...]);` 或更简单的 `pub fn supported_models() -> Vec<Model>`（后者更 KISS，无需 once_cell）。
  2. handlers.rs `get_models` 改为 `Json(ModelsResponse { object: "list".into(), data: super::models::supported_models() })`，函数体从 100 行缩到 5 行。
- **Test cases**:
  - `supported_models_contains_expected_ids` 单测：断言列表包含 `claude-opus-4-6`、`claude-sonnet-4-6` 等关键 id（保护重构不漏拷）。
- **Verify**: 单测 pass；`curl /v1/models` 与重构前字节级一致（json 排序不变）。
- **Complexity**: Small

#### Step C2.1: SseDelivery 参数化 + 双 handler 合并（RED）

- **Files**: `src/interface/http/anthropic/handlers.rs`（新增测试）
- **Action**:
  - `delivery_mode_live_uses_immediate_pings`：表驱动测试，对 `post_messages_impl` 用 `DeliveryMode::Live` mock 流，断言事件序列含 `message_start` 在第一个上游 chunk 之前（无缓冲）。
  - `delivery_mode_buffered_emits_only_pings_until_stream_end`：对 `DeliveryMode::Buffered` mock 流，断言上游 chunk 期间仅有 `ping` SSE，最终一次性输出 `message_start` + 后续。
  - mock 难度大时可降级：单测 `select_handler_function_by_mode_picks_correct_create_stream_fn`，至少保证 mode 分发逻辑被覆盖。
- **Test cases**: 至少 2 项。
- **Verify**: 测试失败（实现尚未抽取）。
- **Complexity**: Medium

#### Step C2.2: SseDelivery 参数化 + 双 handler 合并（GREEN）

- **Files**: `src/interface/http/anthropic/handlers.rs`（重构 195/707 + 320/836 + 362/873）
- **Action**:
  1. 增加内部枚举 `enum DeliveryMode { Live, Buffered }`（已在 `service/conversation/delivery.rs` 中定义，导入复用）。
  2. 提取 `async fn post_messages_impl(state, mut payload, mode: DeliveryMode) -> Response`，承载 195-317 与 707-829 的共同逻辑：
     - kiro_client 取出
     - override_thinking_from_model_name
     - websearch 短路
     - convert_request
     - serialize kiro_request
     - count_all_tokens
     - thinking_enabled 判定
     - 流式分支：根据 mode 调 `handle_stream_request_unified(provider, body, model, input_tokens, thinking_enabled, tool_name_map, mode)`
     - 非流式分支保持不变（共用 `handle_non_stream_request`，与 mode 无关）
  3. 提取 `async fn handle_stream_request_unified(... mode: DeliveryMode) -> Response`：
     - `match mode { Live => create_sse_stream, Buffered => create_buffered_sse_stream }` 二选一
     - StreamContext / BufferedStreamContext 分别构造
  4. `post_messages` 改为 `post_messages_impl(state, payload, DeliveryMode::Live).await`；`post_messages_cc` 改为 `post_messages_impl(state, payload, DeliveryMode::Buffered).await`。
  5. **不**引入完整 SseDelivery trait（YAGNI；研究报告也允许此简化方案）。
- **Verify**: C2.1 测试 pass；既有 handler 测试不回归；`/v1/messages` 与 `/cc/v1/messages` 行为字节级保持。
- **Complexity**: Medium

#### Step C3.1: interface/cli.rs 拆出 Args（无新逻辑）

- **Files**:
  - `src/interface/cli.rs`（新建）
  - `src/interface/mod.rs`（pub mod cli）
  - `src/main.rs:34-45`（搬走）
- **Action**: 把 `Args` 结构体（含 derive Parser 与 doc）整体移到 `cli.rs`；`main.rs` 改 `use crate::interface::cli::Args;`。
- **Test cases**:
  - `cli::tests::default_args_parses_with_no_flags`：`Args::try_parse_from(&["binary"])` 成功，两个字段都是 None。
  - `cli::tests::custom_paths_parse_correctly`：`--config foo.json --credentials bar.json` 解析为对应值。
- **Verify**: 2 个测试 pass；`cargo run -- --help` 输出与重构前一致。
- **Complexity**: Small

#### Phase C 检查点

- [x] `cargo test` 全绿（基线 334 → 344，新增 10 个：models ×3、post_messages 503 ×2 + share_503 + dispatch_distinct、cli ×3）
- [ ] `wc -l src/interface/http/anthropic/handlers.rs` < 600（当前 802：模型抽出 -100、双 handler 合并 -150；剩余 `create_sse_stream` + `create_buffered_sse_stream` + `handle_non_stream_request` 总计 ~430 行，待 Phase D 把流处理迁移到 `delivery.rs` 后才能 < 600）
- [x] `cargo run -- --help` 与 master 一致
- [x] `cargo clippy --all-targets -- -D warnings` 通过
- [ ] curl 冒烟：`/v1/messages`（Live） + `/cc/v1/messages`（Buffered）SSE 帧顺序与 master 字节级对齐（待人工执行）

---

### Phase D：Conversation 层真实拆分（P0）

**目标**：消灭 §3.3 / §4.1。让 reducer.rs / delivery.rs / thinking.rs / tools.rs 不再是空壳别名，而是实际承载逻辑的模块。

依赖：与 Phase B/C 独立。可与 B/C 并行；但必须在 Phase F 之前（F 删除 #![allow(dead_code)] 时需要这些模块已被实际使用）。

#### Step D1.1: thinking.rs 真实化（搬家测试）

- **Files**: `src/service/conversation/thinking.rs`、`src/service/conversation/stream.rs`
- **Action**:
  1. 把 stream.rs 中以下纯函数原封移到 thinking.rs：`find_char_boundary`、`is_quote_char`、`find_real_thinking_start_tag`、`find_real_thinking_end_tag`、`find_real_thinking_end_tag_at_buffer_end`、常量 `QUOTE_CHARS`。
  2. stream.rs 改为 `use super::thinking::{find_real_thinking_start_tag, ...};` 引用。
  3. 保留 thinking.rs 中已有的 `extract_thinking_from_complete_text` + 测试。
  4. 把 stream.rs 中现有的 thinking 标签函数级单测（如有，搜 `test_find_real_thinking`）一并迁移。
- **Test cases**:
  - 既有 stream.rs 中 thinking 相关单测（约 5-10 项）全部迁移；额外补 `find_real_thinking_start_tag_at_buffer_boundary`：标签跨 chunk 时返回 None，等待更多内容。
- **Verify**: `cargo test` 全绿；`grep -c 'fn find_real_thinking' src/service/conversation/stream.rs` == 0；`grep -c 'fn find_real_thinking' src/service/conversation/thinking.rs` >= 3。
- **Complexity**: Medium（约 200 行函数 + 测试搬迁）

#### Step D2.1: reducer.rs 真实化（SseEvent + SseStateManager 迁移）

- **Files**: `src/service/conversation/reducer.rs`、`src/service/conversation/stream.rs`
- **Action**:
  1. 把 stream.rs:227-505 整段（`SseEvent` + `BlockState` + `SseStateManager` 全部 impl）迁移到 reducer.rs。
  2. reducer.rs 改为完整 impl，删除 `pub use super::stream::...` 别名。
  3. stream.rs 改 `use super::reducer::{SseEvent, SseStateManager, ...};`，移除原定义。
  4. 删除 reducer.rs 上的 `#![allow(dead_code, unused_imports)]`。
- **Test cases**: 既有 SseStateManager 测试（如有 `tests/conversation_tests.rs` 或单测）原样运行；额外补充：
  - `event_reducer::tests::message_start_only_emits_once`
  - `event_reducer::tests::content_block_stop_idempotent`
  - `event_reducer::tests::generate_final_events_closes_open_blocks_in_order`
- **Verify**: 测试 pass；`grep -c 'pub struct SseStateManager' src/service/conversation/stream.rs` == 0；`grep -c 'pub use.*SseStateManager' src/service/conversation/reducer.rs` == 0。
- **Complexity**: Medium

#### Step D3.1: delivery.rs 真实化（StreamContext + BufferedStreamContext 迁移）

- **Files**: `src/service/conversation/delivery.rs`、`src/service/conversation/stream.rs`
- **Action**:
  1. 把 stream.rs:510-1214 段（`StreamContext` + `BufferedStreamContext` + impl）迁移到 delivery.rs。
  2. delivery.rs 删除 `pub use ...` 别名，改为完整 impl。
  3. stream.rs 仅保留：`use` 语句、可能的辅助函数（如 `estimate_tokens`），其余为空——若 stream.rs 已无内容则删除文件并在 mod.rs 移除 `pub mod stream;`。
  4. handlers.rs 引用从 `use crate::service::conversation::stream::{BufferedStreamContext, StreamContext, SseEvent};` 改为 `use crate::service::conversation::{reducer::SseEvent, delivery::{BufferedStreamContext, StreamContext}};`。
- **Test cases**: 现有 StreamContext 行为测试（chunk → SSE 转换）保持运行；额外：
  - `delivery::tests::live_delivery_emits_message_start_first`
  - `delivery::tests::buffered_delivery_corrects_input_tokens_after_context_usage_event`
- **Verify**: 测试 pass；`wc -l src/service/conversation/stream.rs` 接近 0 或 stream.rs 已删除。
- **Complexity**: Large（迁移逻辑 ~700 行 + 跨模块引用调整）

#### Step D4.1: tools.rs 抽出（converter.rs 拆分）

- **Files**: `src/service/conversation/tools.rs`（新建）、`src/service/conversation/converter.rs`、`src/service/conversation/mod.rs`
- **Action**:
  1. 新建 tools.rs，迁入 converter.rs 中工具相关函数：
     - `normalize_json_schema`
     - `collect_history_tool_names`
     - `create_placeholder_tool`
     - `extract_tool_result_content`
     - `validate_tool_pairing`
     - `remove_orphaned_tool_uses`
     - `shorten_tool_name`
     - `map_tool_name`
     - `convert_tools`
     - 相关常量 `WRITE_TOOL_DESCRIPTION_SUFFIX` / `EDIT_TOOL_DESCRIPTION_SUFFIX`（若仅工具用则一并迁；否则保留 converter.rs）。
  2. converter.rs `use super::tools::{convert_tools, validate_tool_pairing, remove_orphaned_tool_uses, ...};`
  3. mod.rs 增加 `pub mod tools;`
- **Test cases**: 现有 converter.rs 工具相关单测（如 `test_convert_tools`、`test_normalize_json_schema_*`）全部迁移到 tools.rs。
- **Verify**: 测试 pass；`wc -l src/service/conversation/converter.rs` < 1000（剩下 ProtocolConverter 主干 + history/message 转换约 800 行；如仍 >600，延伸到 D4.2 拆 history.rs / message.rs）。
- **Complexity**: Medium

#### Step D4.2: converter.rs 进一步收敛（视 D4.1 结果可选）

- **Files**: `src/service/conversation/converter.rs`、新建 `history.rs` / `message.rs`（如需要）
- **Action**: 仅当 D4.1 后 converter.rs 仍 > 600 行时执行：把 `build_history`、`merge_user_messages`、`convert_assistant_message`、`merge_assistant_messages`、`process_message_content` 等历史/消息转换函数拆到独立子模块。否则跳过此步。
- **Test cases**: 同 D4.1 模式（搬家测试）。
- **Verify**: `wc -l src/service/conversation/converter.rs` < 600。
- **Complexity**: Medium（条件性）

#### Phase D 检查点

- [ ] `cargo test` 全绿（约 +20 个测试，主要是搬家迁移）
- [ ] `wc -l src/service/conversation/{stream,converter,delivery,reducer,thinking,tools,websearch}.rs`：除 websearch.rs（761 行 → 拆 history 到 phase F 决定）外，全部 < 600
- [ ] `grep -c 'pub use super::stream::' src/service/conversation/{reducer,delivery,thinking}.rs` 全部 == 0
- [ ] `cargo bench` 或冒烟：完整 SSE 一次请求耗时与 master 相比无显著回归（< 5%）
- [ ] curl `/v1/messages` Live + Buffered 的事件流字节级与 master 对齐

---

### Phase E：运行时质量（P1）

#### Step E1.1: stats 30s debounce（RED）

- **Files**: `src/service/credential_pool/pool.rs`（新增测试）
- **Action**:
  - `maybe_persist_stats_only_writes_once_within_debounce_window`：mock StatsFileStore（计 save 次数），在 100ms 内连调 5 次 `report_success`，等待 50ms 后断言 save count == 0；再睡 30s 后断言 save count == 1（用 tokio::time::pause + advance 加速）。
  - `maybe_persist_stats_persists_on_explicit_flush`：调用新增 `flush_stats()` 立即落盘（用于 admin 删除凭据等强一致场景）。
  - `maybe_persist_stats_persists_on_drop`：drop pool 触发剩余落盘（保证最后一次写不丢）。
- **Test cases**: 3 项。
- **Verify**: 测试失败。
- **Complexity**: Medium（涉及 tokio time + Drop）

#### Step E1.2: stats 30s debounce（GREEN）

- **Files**: `src/service/credential_pool/pool.rs:54-97, 375-381` + 新建 `src/service/credential_pool/stats_persister.rs`（或内联到 pool.rs 视体量决定）
- **Action**:
  1. 新增 `StatsPersister`：含 `Mutex<Option<Instant>> last_persist_at`、`Arc<StatsFileStore>`、`Arc<CredentialStats>`。
  2. `maybe_persist_stats` 改为：检查 `last_persist_at + 30s` 是否到，若到则同步 `store.save(...)` + 更新 `last_persist_at`；否则启动一个 `tokio::spawn` 定时器（每个池只允许 1 个未触发的定时任务，用 AtomicBool 守卫）。
  3. `flush_stats(&self)` 同步立即落盘（admin delete_credential / 主动 shutdown 用）。
  4. `delete_credential` 路径已有立即落盘（pool.rs:712-715），保持。
  5. `Drop for CredentialPool` 不实现（避免 async drop 复杂性）；用 main.rs 的 `tokio::signal` 钩子在退出前显式 `pool.flush_stats()`。
- **Verify**: E1.1 3 个测试 pass；高并发压测 100 req/s 持续 1 分钟，stats 文件 mtime 间隔 ≥ 30s。
- **Complexity**: Medium

#### Step E2.1: Priority 模式候选顺序稳定化（RED）

- **Files**: `src/service/credential_pool/pool.rs`（新增测试）
- **Action**:
  - `select_priority_with_tied_priorities_picks_lowest_id_first`：构造 3 个凭据 priority 全为 0，重复 100 次 `select_one(None)`，断言始终返回最低 id 的凭据（在 select_highest_priority 重置 current_id 之前）。
  - `select_priority_after_disable_picks_next_lowest_id`：禁用最低 id 后，下一次 select 返回次低 id（不是任意 id）。
- **Test cases**: 2 项。
- **Verify**: 测试可能偶发通过（HashMap 顺序非确定），需重复 100 次稳定失败。
- **Complexity**: Small

#### Step E2.2: Priority 模式候选顺序稳定化（GREEN）

- **Files**: `src/service/credential_pool/pool.rs:280-301, 558-583, 590-595`（select_one + switch_to_next + select_highest_priority）
- **Action**: 把所有 `min_by_key(|(_, c)| c.priority)` 改为 `min_by_key(|(id, c)| (c.priority, *id))`：当 priority 相同时按 id 升序排序。
- **Verify**: E2.1 测试 pass。
- **Complexity**: Small

#### Step E3.1: set_load_balancing_mode 单锁完成（RED + GREEN）

- **Files**: `src/service/credential_pool/pool.rs:136-161`
- **Action**:
  1. 测试 `set_load_balancing_mode_rollback_uses_same_lock_no_race`：mock persist 失败，断言回滚后 `get_load_balancing_mode()` 与初始值相同；重复 1000 次保证无 race。
  2. 实现：把 `let previous = { let mut guard = ...; replace }; ...; *self.load_balancing_mode.lock() = previous;` 改为单一 `let mut guard = self.load_balancing_mode.lock(); ...; if persist 失败 { *guard = previous; }`，全程持锁。注意 `persist_load_balancing_mode` 涉及磁盘 I/O，持锁等待磁盘**对该 hot path 影响可接受**——admin 路径，调用频率极低。
- **Test cases**: 1 项。
- **Verify**: 测试 pass。
- **Complexity**: Small

#### Step E4.1: kiro_client.rs call_api/call_api_stream 合并（RED + GREEN）

- **Files**: `src/service/kiro_client.rs:43-76`、调用点 `src/interface/http/anthropic/handlers.rs`、`src/service/conversation/websearch.rs`
- **Action**:
  1. 删除 `call_api_stream`，仅保留 `call_api`（语义已重合，区别仅在 caller 不缓冲）。
  2. handlers.rs 中 `provider.call_api_stream(...)` → `provider.call_api(...)`。
- **Test cases**: 不增测试（语义保持，搬家级改动）；既有 handler 集成测试覆盖。
- **Verify**: `cargo test` 全绿；`grep -c 'call_api_stream' src/` == 0。
- **Complexity**: Small

#### Phase E 检查点

- [ ] `cargo test` 全绿（含新增 ~6 个测试）
- [ ] 100 req/s 压测 60s，stats 文件 mtime 变化次数 ≤ 3（30s × 2 + 收尾 1）
- [ ] `grep -c 'min_by_key.*c\.priority\b' src/service/credential_pool/pool.rs` == 0（全部已替换为元组）

---

### Phase F：清理与文档（P2）

依赖：A–E 全部完成（删除 dead_code allow 之前需要这些模块已被实际使用）。

#### Step F1.1: 移除关键路径 #![allow(dead_code)]

- **Files**: 21 处 #![allow(dead_code)]：见研究报告 §4.2
- **Action**:
  1. 逐文件移除 `#![allow(dead_code)]`，运行 `cargo clippy --all-targets -- -D warnings`。
  2. 对真实 dead code：删除（如 `domain/usage.rs` 中未读字段，若能删则删；若由 serde 反序列化需要则改 `#[allow(dead_code)]` 单字段而非整模块；如 `domain/event/assistant.rs` 中字段是 JSON 解析靶子则保留单字段 allow 并写注释解释）。
  3. 重点处理：
     - `service/conversation/{reducer,delivery,thinking}.rs`：D 完成后应自然清空 dead，直接删 allow。
     - `domain/retry.rs`：A5 后所有变体已被使用，删 allow。
     - `infra/refresher/mod.rs`：内部纯函数已被 social/idc/api_key 使用，应可清理。
     - `service/credential_pool/mod.rs`：再确认是否还需要 allow。
- **Test cases**: 不新增（行为保持），但 `cargo clippy --all-targets -- -D warnings` 必须通过。
- **Verify**: `grep -rn '^#!\[allow(dead_code' src/` 命中 ≤ 2（研究中允许的特例）。
- **Complexity**: Medium

#### Step F2.1: GlobalProxyConfig::to_proxy_config() 集中

- **Files**:
  - `src/config/proxy.rs`（增加方法）
  - `src/main.rs:151-160`、`src/infra/refresher/mod.rs:113-122`、`src/service/credential_pool/pool.rs:826-835`（3 处替换）
- **Action**:
  1. 在 `impl GlobalProxyConfig` 增加 `pub fn to_proxy_config(&self) -> Option<ProxyConfig>`，复刻三处共同模式：取 `proxy_url`，可选 `with_auth`。
  2. 三个调用点全部改为 `config.proxy.to_proxy_config()`。
- **Test cases**:
  - `to_proxy_config_returns_none_when_url_missing`
  - `to_proxy_config_includes_auth_when_username_password_present`
  - `to_proxy_config_omits_auth_when_either_missing`
- **Verify**: 3 个测试 pass；`grep -c 'ProxyConfig::new(url)' src/` == 1（仅在 to_proxy_config 内）。
- **Complexity**: Small

#### Step F3.1: 10 分钟过期 warning

- **Files**: `src/service/credential_pool/pool.rs:964-973`（is_token_expired）+ `prepare_token` 等使用点
- **Action**:
  1. 增加 `fn is_token_expiring_soon(cred: &Credential) -> bool`（10 分钟阈值）。
  2. `prepare_token` 在 token 未过期但 expiring_soon 时 `tracing::warn!(id, "token 即将过期 (< 10min)，建议尽快刷新")`，但不强制 refresh（保持现行 5 分钟阈值的强制刷新）。
- **Test cases**:
  - `is_token_expiring_soon_returns_true_within_10_min`
  - `is_token_expiring_soon_returns_false_after_10_min`
- **Verify**: 测试 pass。
- **Complexity**: Small

#### Step F4.1: API key 日志收敛到前 4 字符

- **Files**: `src/main.rs:251`
- **Action**: `tracing::info!("API Key: {}***", &api_key[..(api_key.len() / 2)]);` 改为 `tracing::info!("API Key: {}***（长度 {}）", &api_key[..api_key.len().min(4)], api_key.len());`。
- **Test cases**: 不新增（main.rs 难单测）；改后 cargo run 启动日志 grep 验证。
- **Verify**: `cargo run` 输出 `API Key: sk-x***（长度 51）` 而非半截 key。
- **Complexity**: Small

#### Step F5.1: README 同步移除 countTokens* 字段

- **Files**: `README.md:185-187, 211-213`
- **Action**: 删除 `countTokensApiUrl/Key/AuthType` 三行（配置表）+ 完整示例三行。
- **Test cases**: 不适用。
- **Verify**: `grep -c countTokens README.md` == 0。
- **Complexity**: Small

#### Step F5.2: 新增 CHANGELOG.md

- **Files**: `CHANGELOG.md`（新建）
- **Action**:
  ```markdown
  # Changelog

  ## refactor/v2 (2026-04-26)

  ### Breaking Changes
  - 移除 `countTokensApiUrl` / `countTokensApiKey` / `countTokensAuthType` 配置项。
    `count_tokens` 端点改为内置 token 估算，不再支持外部代理。
    旧字段会被 serde 静默忽略（向前兼容），但下次保存配置时会从文件中消失。

  ### Features
  - Hexagonal 架构重构：domain / infra / service / interface 分层。
  - 错误体系：thiserror 分层取代 anyhow，HTTP 响应映射通过结构化 enum 完成。
  - 凭据池 single-flight 刷新：避免并发 refresh 浪费 refresh_token。
  - DisabledReason 区分 Manual / TooManyFailures / TooManyRefreshFailures / QuotaExceeded / InvalidRefreshToken / InvalidConfig。

  ### Improvements
  - stats 30s debounce，避免高并发下每请求落盘。
  - Priority 模式候选顺序稳定化（按 priority + id 二级排序）。
  - 启动时初始化 current_id 为最低 priority 启用凭据。
  - Token 即将过期（< 10min）时记录 warning。
  ```
- **Verify**: 文件存在；与 README 同步引用。
- **Complexity**: Small

#### Phase F 检查点

- [ ] `grep -rn '^#!\[allow(dead_code' src/` ≤ 2
- [ ] `cargo clippy --all-targets -- -D warnings` 通过
- [ ] README + CHANGELOG 均同步
- [ ] `cargo run` 启动日志中 API Key 仅显示前 4 字符

---

## Test Strategy

### Automated Tests（新增汇总）

| 测试名 | 类型 | 输入 | 预期 |
|--------|------|------|------|
| `load_rejects_duplicate_ids` | Unit | `[{id:1},{id:1}]` | `Err(ConfigError::Validation)` |
| `current_id_initialized_to_lowest_priority_after_install_initial_states` | Unit | 3 凭据 | `current_id == lowest priority id` |
| `prepare_token_serializes_concurrent_refresh_for_same_credential` | Async | 2 task 同时 acquire | mock refresher count == 1 |
| `apply_refresh_overwrites_access_token_and_optional_fields` | Unit | 完整 outcome | 4 字段全更新 |
| `disabled_reason_includes_manual_and_too_many_refresh_failures` | Unit | 编译期 pattern | 编译通过 |
| `set_disabled_true_assigns_manual_reason` | Unit | `set_disabled(id, true)` | reason == Manual |
| `report_refresh_failure_after_threshold_uses_too_many_refresh_failures` | Unit | 3 次 refresh 失败 | reason == TooManyRefreshFailures |
| `decide_400_with_content_length_exceeds_threshold_is_fail_context_window_full` | Unit | 400 + 关键字 | `Fail(ContextWindowFull)` |
| `decide_400_with_input_is_too_long_is_fail_input_too_long` | Unit | 400 + 关键字 | `Fail(InputTooLong)` |
| `kiro_error_into_response_*`（×8） | Unit | 各 KiroError 变体 | status + body 字段对齐 |
| `call_mcp_api_returns_provider_error_when_provider_fails` | Unit | mock 失败 | `Err(ProviderError)` |
| `supported_models_contains_expected_ids` | Unit | - | 列表包含关键 id |
| `delivery_mode_live_uses_immediate_pings` | Async | mock 流 | message_start 在 chunk 之前 |
| `delivery_mode_buffered_emits_only_pings_until_stream_end` | Async | mock 流 | chunk 期间仅 ping |
| `cli::tests::default_args_parses_with_no_flags` | Unit | `[binary]` | None,None |
| `find_real_thinking_*`（迁移） | Unit | 各种边界 | 维持原行为 |
| `event_reducer::tests::*`（迁移 + 新增） | Unit | SSE 事件序列 | 状态机正确 |
| `delivery::tests::*` | Unit | StreamContext 行为 | 转换正确 |
| `maybe_persist_stats_only_writes_once_within_debounce_window` | Async tokio time | 5×record | save count == 0/1 |
| `maybe_persist_stats_persists_on_explicit_flush` | Async | flush_stats | 立即落盘 |
| `select_priority_with_tied_priorities_picks_lowest_id_first` | Unit | 3 凭据同 priority | 稳定返回最低 id |
| `set_load_balancing_mode_rollback_uses_same_lock_no_race` | Stress | mock 失败 ×1000 | 无 race |
| `to_proxy_config_*`（×3） | Unit | - | 配置转换正确 |
| `is_token_expiring_soon_*`（×2） | Unit | 不同 expires_at | 边界正确 |

### Manual Verification

- [ ] `cargo run` 启动正常，日志 API Key 仅前 4 字符 + 长度
- [ ] `curl http://localhost:8080/v1/models` 返回 10 个模型（与 master 字节级一致）
- [ ] `curl POST /v1/messages` 流式：第一帧是 `event: message_start`（DeliveryMode::Live）
- [ ] `curl POST /cc/v1/messages` 流式：上游慢响应期间仅有 `event: ping` 帧
- [ ] `curl POST /v1/messages` 输入超长：响应 400 + `error.type=invalid_request_error`
- [ ] 强制 endpoint 不存在触发：响应 503 + `error.type=api_error`
- [ ] `curl POST /api/admin/credentials/:id/disabled` true → 该凭据 admin_snapshot 中 `disabled_reason == "Manual"`
- [ ] 100 req/s 压测 60s：`stat -c %Y kiro_stats.json` 变化 ≤ 3 次
- [ ] credentials.json 含重复 id 启动：进程退出，log 显示 "credentials.json 含重复 id"
- [ ] grep `cargo test 2>&1 | grep "test result"`：测试数 ≥ 314（289 + 至少 25 新增）

---

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| A3 single-flight 死锁 | 严重（请求挂起） | 用 `tokio::sync::Mutex` 而非 `parking_lot`；锁仅覆盖 refresh 调用与 store.replace；先在测试中跑 `tokio::time::timeout(5s, ...)` 兜底 |
| D 拆分引入循环依赖 | 中（编译失败） | 严格遵循 reducer→thinking→delivery 单向依赖；reducer.rs 不引用 delivery.rs；delivery.rs 引用 reducer + thinking |
| E1 debounce 漏写最后一次 | 中（统计丢失） | main.rs 增加 SIGTERM/SIGINT handler 调 `pool.flush_stats()`；测试覆盖 drop 路径（虽不实现 Drop，但 flush 需在 main 退出前调） |
| 错误响应字节级不一致导致客户端解析失败 | 中（兼容性） | B2.4 中保持 `error.type` / `error.message` 字段名与值；B2.1 测试断言 body 字符串包含关键 message 字符串 |
| Phase D 拆分行为偏移 | 中（功能回归） | 严格搬家不改逻辑；每步 D 单独验证 `cargo test`；最终冒烟 SSE 字节级比对 |
| Phase F1 误删非 dead 字段 | 低（编译失败） | 逐文件 clippy + 测试，不一次性 sed 全删 |
| 22 个 issue 一次性合入风险大 | 高（review 复杂度） | 严格按 phase 提交；每个 phase 独立 commit + PR-style 描述；可独立 revert |

## Rollback Strategy

- 每个 Phase 独立 git commit，commit message 标注 `Phase A.X / Phase B.X` 等。
- 任意 phase 失败：`git revert <phase commits>` 回滚单 phase 不影响其他。
- credential.json schema 仅新增字段（`#[serde(default)]`），向前兼容旧文件。
- 新增 `#[serde(default)]` 字段不会破坏旧版本读取该文件。
- HTTP 响应字段与值保持，rollback 后客户端无感知。
- CHANGELOG.md 与 README 同步 revert 即可。
- 完整 rollback：`git revert <plan-start-commit>..HEAD`，回到 `58dc47d` 状态。

## Status

- [x] Plan approved
- [x] Phase A 完成（2026-04-26）
- [x] Phase B 完成（2026-04-26）
- [x] Phase C 完成（2026-04-26，handlers.rs 实际行数受限于 Phase D 流处理迁移，已记录在检查点）
- [ ] Phase D 完成
- [ ] Phase E 完成
- [ ] Phase F 完成
- [ ] Implementation complete
