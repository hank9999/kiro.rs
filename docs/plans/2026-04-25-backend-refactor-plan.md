# Plan: kiro-rs 后端整体重构（2026-04-25）

> 与研究文档配套：[`2026-04-25-backend-refactor-research.md`](./2026-04-25-backend-refactor-research.md)

## Summary

把 `src/` 中除 `admin_ui/` 外的全部 Rust 代码（约 14 100 行 / 36 文件）按 Hexagonal-style 分层重写为
`error / config / domain / infra / service / interface` 六层结构。核心动作：

1. 把 `MultiTokenManager`（2 599 行）拆为 `Store / Selector / State / Refresher` 四件套，对外以 `CredentialPool` 门面整合。
2. 把 `KiroProvider`（519 行）改为 `RequestExecutor + RetryPolicy`，删除 `call_api / call_mcp` 双胞胎。
3. 把 `anthropic/handlers.rs`（938 行）+ `stream.rs`（1 989）+ `converter.rs`（1 777）拆为 `ProtocolConverter / EventReducer / ThinkingExtractor / SseDelivery (Live | Buffered)`，消除 v1 ↔ cc/v1 双胞胎 handler。
4. 引入分层 `thiserror` 错误体系，删掉 4 个 `classify_*_error` 字符串匹配。
5. 删除 count_tokens 远程 API 调用整段代码 + 三个相关 config 字段（**Breaking Change**，详见下文 "对外契约"）；本地 count_tokens 改 `async fn` 路径。
6. `Config` 改用 `serde flatten` 子结构（NetConfig / RegionConfig / ProxyConfig / KiroIdentity / AdminConfig / EndpointConfig / FeatureFlags），JSON 仍是扁平 camelCase。
7. 消除 `token::COUNT_TOKENS_CONFIG` 与 `machine_id::FALLBACK_MACHINE_IDS` 两处全局静态。
8. 保留并迁移以下亮点：`KiroEndpoint` trait、`EventStreamDecoder` 四态状态机、`KiroCredentials` 数据模型 + 全部单测、`http_client` 工厂、`common::auth`、`admin/error.rs` 的结构化错误模式。

**对外契约**：

- **HTTP 端点（5 Anthropic + 10 Admin）**：方法、路径、请求/响应 JSON 字段名与字段层级保持一致；HTTP 关键 header（Content-Type、CORS、鉴权相关）一致；SSE 事件类型与序列、关键字段（input_tokens、stop_reason、content delta、index）一致。**不要求字节级一致**：动态项（message id、时间戳、UUID、Date/Server header、字段序列化顺序中的非语义差异）允许差异。
- **上游 AWS / Kiro 协议**：保持一致。
- **`credentials.json`**：现有格式与字段保留；单对象 / 数组两种格式仍兼容。
- **`config.json`（Breaking Change）**：删除 `countTokensApiUrl` / `countTokensApiKey` / `countTokensAuthType` 三字段。旧字段在加载时被 serde 默认忽略（启动不失败）；首次 `Config::save()` 后这三个字段会从文件中消失。CHANGELOG 必须显式标注；README 中相关章节同步移除。

## Stakes Classification

**Level**: **High**

**Rationale**：

- 单次合入主干，触及全部业务模块（Anthropic 协议层、Kiro 客户端、凭据池、配置、Admin、错误体系）。
- 外部契约多（5 + 10 个 HTTP 端点 + 配置文件 + 凭据持久化 + 上游协议），任一处偏离都直接影响线上用户。
- 线程/并发模型大幅变动：5 把锁的 `MultiTokenManager` 拆解后，状态一致性需要重新论证。
- count_tokens 改为 async 后，所有上游 token 估算路径都会经过 `.await`，潜在阻塞行为可能变化。

## Context

- **Research**：[`docs/plans/2026-04-25-backend-refactor-research.md`](./2026-04-25-backend-refactor-research.md)
- **当前分支**：`refactor/v2`（刚从 master 分出，未做实质改动）
- **目标分支**：仍在 `refactor/v2` 上一次性完成；通过后整体合入 master
- **Affected Areas**：除 `admin-ui/`（前端 SPA 源码目录）外的所有 Rust 代码 + `Cargo.toml` 可能的依赖小调整。`src/admin_ui/router.rs` 迁移到 `src/interface/http/ui.rs`，逻辑零改动。**不引入** `tests/` 集成测试目录（验证一律通过 unit test 与手工冒烟覆盖，详见 Phase 8）。

## 目标目录结构

```
src/
├── main.rs                          薄启动器：解析 args + 初始化日志 + 装配 wiring → axum::serve
├── error.rs                         crate-level error re-export
├── config/                          配置（serde flatten 保 JSON 兼容）
│   ├── mod.rs                       Config 顶级 + load/save
│   ├── net.rs                       NetConfig (host, port, tlsBackend)
│   ├── region.rs                    RegionConfig (region, authRegion, apiRegion)
│   ├── proxy.rs                     GlobalProxyConfig (proxyUrl/Username/Password)
│   ├── kiro.rs                      KiroIdentity (kiroVersion, machineId, systemVersion, nodeVersion)
│   ├── admin.rs                     AdminConfig (adminApiKey)
│   ├── endpoint.rs                  EndpointConfig (defaultEndpoint, endpoints map)
│   └── feature.rs                   FeatureFlags (extractThinking, loadBalancingMode)
├── domain/                          抽象层（trait + 数据 + 错误）
│   ├── mod.rs
│   ├── error.rs                     KiroError / RefreshError / ProviderError / RetryDecision
│   ├── credential.rs                Credential 数据 + AuthMethod enum + 不变量校验
│   ├── token.rs                     TokenSource trait + RefreshOutcome
│   ├── endpoint.rs                  KiroEndpoint trait + RequestContext（迁移自 kiro/endpoint）
│   ├── retry.rs                     RetryPolicy trait + RetryDecision enum
│   ├── selector.rs                  CredentialSelector trait + CredentialView<'a>（id+credential+state+stats 的只读视图，select 必须同步纯计算）
│   └── event.rs                     KiroEvent enum（迁移自 kiro/model/events）
├── infra/                           实现层
│   ├── mod.rs
│   ├── http/
│   │   ├── mod.rs
│   │   ├── client.rs                build_client + ProxyConfig（保留并简化）
│   │   ├── executor.rs              RequestExecutor 通用 send + 退避 + 故障转移
│   │   └── retry.rs                 DefaultRetryPolicy（状态码 → RetryDecision）
│   ├── parser/                      AWS Event Stream（整体保留，仅 use 路径调整）
│   │   └── ...
│   ├── refresher/
│   │   ├── mod.rs
│   │   ├── social.rs                SocialRefresher : TokenSource
│   │   ├── idc.rs                   IdcRefresher : TokenSource
│   │   └── api_key.rs               ApiKeyRefresher : TokenSource (passthrough)
│   ├── selector/
│   │   ├── mod.rs
│   │   ├── priority.rs              PrioritySelector : CredentialSelector
│   │   └── balanced.rs              BalancedSelector : CredentialSelector (least-used)
│   ├── endpoint/
│   │   ├── mod.rs                   EndpointRegistry（持有 default_endpoint + endpoints map + resolve_for(&Credential) → Arc<dyn KiroEndpoint>）
│   │   └── ide.rs                   IdeEndpoint : KiroEndpoint
│   ├── machine_id.rs                设备指纹生成（移除全局静态，由 CredentialPool 持有 fallback 缓存）
│   └── storage/
│       ├── mod.rs
│       ├── credentials_file.rs      CredentialsFileStore（单/多格式回写）
│       ├── stats_file.rs            StatsFileStore（success_count + last_used_at）
│       └── balance_cache.rs         BalanceCacheStore（admin 5 分钟缓存）
├── service/                         用例编排层
│   ├── mod.rs
│   ├── credential_pool/
│   │   ├── mod.rs                   CredentialPool 门面（acquire / report_*）
│   │   ├── store.rs                 CredentialStore（增删改 + 持久化协调）
│   │   ├── state.rs                 CredentialState（失败计数 + 禁用 + 自愈）
│   │   └── pool.rs                  组合：store + state + selector + refresher
│   ├── kiro_client.rs               KiroClient = executor + EndpointRegistry + pool + policy（具体类型，不抽 trait）
│   ├── conversation/
│   │   ├── mod.rs
│   │   ├── converter.rs             ProtocolConverter (AnthropicRequest → KiroRequest)
│   │   ├── tools.rs                 工具名映射 / placeholder / convert_tools
│   │   ├── thinking.rs              ThinkingExtractor 状态机（非流式 + 流式共用）
│   │   ├── reducer.rs               EventReducer（KiroEvent → AnthropicSseEvent[]）
│   │   ├── delivery.rs              SseDelivery trait + LiveDelivery + BufferedDelivery
│   │   ├── tokens.rs                count_tokens 本地实现（async fn）
│   │   └── websearch.rs             WebSearch 旁路（保留特殊路径）
│   └── admin/
│       ├── mod.rs
│       ├── service.rs               AdminService（去掉 4 个 classify_*_error）
│       └── balance.rs               余额查询 + 5 分钟缓存编排
├── interface/                       接口层
│   ├── mod.rs
│   ├── http/
│   │   ├── mod.rs
│   │   ├── auth.rs                  API Key 提取 + 常量时间比较（迁移自 common/auth.rs）
│   │   ├── error.rs                 HTTP 错误 mapping（KiroError → status + JSON envelope）
│   │   ├── anthropic/
│   │   │   ├── mod.rs
│   │   │   ├── router.rs            /v1, /cc/v1
│   │   │   ├── handlers.rs          单个 messages handler，参数化 SseDelivery
│   │   │   ├── dto.rs               MessagesRequest / Response / Tool / SystemMessage
│   │   │   └── models.rs            GET /v1/models 模型列表
│   │   ├── admin/
│   │   │   ├── mod.rs
│   │   │   ├── router.rs            /api/admin/*
│   │   │   ├── handlers.rs
│   │   │   └── dto.rs
│   │   └── ui.rs                    admin_ui SPA fallback（迁移自 admin_ui/router.rs，逻辑不动）
│   └── cli.rs                       clap Args
```

> 不引入 `tests/` 目录与 `src/lib.rs`：所有验证一律通过模块内 `#[cfg(test)] mod tests` 的**纯函数 / 纯逻辑单元测试** + Phase 8 手工冒烟完成。**不使用任何 mock / fake**（不引入 wiremock、不手搓 TcpListener、不抽 KiroClient trait 注入假实现）：组合层（refresher、executor、handler）的端到端行为完全靠 Phase 8 真实启动冒烟覆盖；可单测的部分一律拆为纯函数（错误码 → 错误类型映射、URL/body 构造、retry 决策、协议转换、事件 reducer、token 估算等）。

## Success Criteria

### 功能等价

- [ ] `config.json` 旧格式（包括已废弃的 `countTokensApiUrl/Key/AuthType`）能成功加载并启动；旧字段被 serde 忽略，首次 `save()` 后字段从文件中消失（**Breaking Change，预期行为**）
- [ ] `credentials.json` 单对象格式与数组格式都能加载、排序、首次回写补齐 ID/machineId
- [ ] 5 个 Anthropic 端点：请求/响应 JSON 字段名一致；HTTP 关键 header（Content-Type、CORS、鉴权）一致；SSE 事件类型与序列、关键字段（input_tokens、stop_reason、content delta、index）一致。**不要求字节级一致**（动态 id、时间戳、Date/Server header、字段序列化顺序差异允许）
- [ ] 10 个 Admin 端点：请求/响应 JSON 字段名一致；错误 envelope 形态一致 `{"error": {"type", "message"}}`；HTTP header 关键字段一致
- [ ] Admin UI（`/admin`、`/admin/{*file}`）静态资源响应不变
- [ ] 凭据池行为等价：`MAX_RETRIES_PER_CREDENTIAL=3`、`MAX_TOTAL_RETRIES=9`、`MAX_FAILURES_PER_CREDENTIAL=3`、`STATS_SAVE_DEBOUNCE=30s`、token 提前 5 分钟刷新、即将过期 10 分钟提示、自动禁用全灭后自愈
- [ ] 余额缓存 5 分钟 TTL、文件位置不变
- [ ] 负载均衡 priority / balanced 模式语义不变（balanced = Least-Used + 平局看 priority）
- [ ] WebSearch 旁路、Thinking 模型名后缀覆写、profile_arn 注入逻辑不变
- [ ] 多凭据 token 刷新后回写 `credentials.json` 行为不变；单对象格式不回写

### 代码质量

- [ ] 单文件不超过 600 行（特例：`tests/`、`infra/parser/decoder.rs` 已是合理 337 行）
- [ ] 没有 `block_in_place` / `Handle::current().block_on(...)` 在 axum handler 路径
- [ ] 没有 `OnceLock<...>` 用于业务配置或可变缓存（仅允许日志、常量等无副作用场景）
- [ ] 没有 `msg.contains("...")` 形式的错误识别（除非匹配上游响应 body 的稳定字符串特征，例如 `MONTHLY_REQUEST_COUNT`，且封装在 endpoint 层）
- [ ] `cargo test` 全绿；现存所有 `#[cfg(test)] mod tests` 块的用例不丢失（binary crate 内的 unit test 通过 `cargo test` 运行，不再使用 `cargo test --lib`）
- [ ] `cargo build --release` 通过、`cargo build --no-default-features` 通过（musl 路径）；**Phase 7 完工后** `cargo clippy --all-targets -- -D warnings` 通过（中间 Phase 仅要求 `cargo clippy --all-targets` 无 error）

### 行为可观测

- [ ] 启动日志列出 5 个 Anthropic + 10 个 Admin 端点（与现状一致）
- [ ] `tracing` 日志级别 / 字段与现状对齐（`info!`, `warn!`, `debug!`, `trace!`）

## Implementation Steps

### Phase 0：准备工作

#### Step 0.1：内嵌 fixture（用于 unit test）

- **Files**：与对应 unit test 模块同目录（例如 `src/config/tests/fixtures/`）；可用 `include_str!()` 引入
- **Action**：从现有 `config.example.json` / `credentials.example.*.json` / `README.md` 抽样，构造下列 fixture（仅供 unit test `#[cfg(test)] mod tests` 使用）：
  - `config_legacy_with_count_tokens_api.json`（含将被删除的字段，验证 serde 忽略 + save 后字段消失）
  - `config_minimum.json`（仅必填）
  - `config_full.json`（README 完整示例）
  - `credentials_single_social.json`
  - `credentials_single_idc.json`
  - `credentials_array_mixed.json`（social + idc + api_key + direct proxy）
  - `credentials_with_machine_id.json`
- **Verify**：fixture 文件被 unit test `include_str!` 引入并断言通过
- **Complexity**：Small

#### Step 0.2：在 `Cargo.toml` 添加 dev-dependencies

- **Files**：`Cargo.toml`
- **Action**：添加 `[dev-dependencies]` 段：
  ```toml
  thiserror = "2"
  tokio-test = "0.4"
  ```
  并在 `[dependencies]` 添加 `thiserror = "2"`（生产代码也要）
- **测试策略**：**不引入任何 mock / fake**（不要 wiremock、tower、TcpListener mock、trait 注入假实现）。所有 unit test 限定为**纯函数 / 纯逻辑**（错误映射、URL/body 构造、retry 决策、协议转换、事件 reducer、token 估算、selector 排序、状态机迁移等）。组合层端到端行为放到 Phase 8 真实启动冒烟覆盖。dev-dep 仅 `thiserror` + `tokio-test`。
- **Verify**：`cargo check` 通过
- **Complexity**：Small

#### Step 0.3：建立计划任务清单

- **Files**：N/A（用 TaskCreate 在执行阶段拆解）
- **Action**：在执行阶段开始时，按本计划 Phase 1-8 创建 `TaskCreate` 任务并按 `TaskUpdate` 推进
- **Verify**：任务建好且状态可见
- **Complexity**：Small

---

### Phase 1：基建（错误体系 + Config 子结构）*(必须在 Phase 2 之前完成)*

#### Step 1.1：测试新错误层级（RED）

- **Files**：`src/domain/error.rs`（新建）
- **Action**：定义 `KiroError`, `RefreshError`, `ProviderError`, `ConfigError` 等 enum + 单元测试：
  - `RefreshError::TokenInvalid` 的 `Display` 包含 "invalid_grant"
  - `ProviderError::AllCredentialsExhausted { available, total }` `Display` 含具体数字
  - `KiroError::from(reqwest::Error)` 路径
  - `From<RefreshError> for KiroError` 自动转换
- **Test cases**：
  - `RefreshError::TokenInvalid` → display contains "invalid_grant"
  - `ProviderError::ContextWindowFull` → display contains "Context window is full"
  - `KiroError::Refresh(RefreshError::Network(_))` → kind() == "network"
  - `From<ProviderError>` for `KiroError`
- **Verify**：`cargo test domain::error` 失败（实现尚未存在）
- **Complexity**：Small

#### Step 1.2：实现错误层级（GREEN）

- **Files**：`src/domain/error.rs`（实现）+ `src/error.rs`（re-export）
- **Action**：用 `thiserror::Error` derive，按照 Step 1.1 测试编写实现。要点：
  - `RefreshError`：`TokenInvalid`, `RateLimited`, `Unauthorized`, `Forbidden`, `Network(reqwest::Error)`, `ServerError(StatusCode)`, `MalformedResponse(serde_json::Error)`
  - `ProviderError`：`AllCredentialsExhausted { available, total }`, `ContextWindowFull`, `InputTooLong`, `BadRequest(String)`, `UpstreamHttp { status: u16, body: String }`, `EndpointResolution(String)`
  - `KiroError`：`Refresh(RefreshError)`, `Provider(ProviderError)`, `Endpoint(...)`, `Network(reqwest::Error)`, `Config(ConfigError)`, `Storage(io::Error)`, `Decode(parser::ParseError)`
  - 提供 `KiroError::http_status_hint()` 给 interface/http 层用
- **Verify**：`cargo test domain::error` 全绿
- **Complexity**：Medium

#### Step 1.3：测试 Config 子结构反序列化（RED）

- **Files**：`src/config/mod.rs` 等子文件（新建）
- **Action**：写测试：
  - 加载 `config_legacy_with_count_tokens_api.json` → 成功，`count_tokens_*` 字段被忽略（serde 默认）
  - 加载 `config_minimum.json` → 成功，所有可选字段使用默认值
  - 加载 `config_full.json` → 成功，每个子结构字段值正确
  - `Config::default()` 产物序列化后与 `Default::default()` 等价（roundtrip）
  - `effective_auth_region()` / `effective_api_region()` 在 Config 上方法签名不变
- **Test cases**：
  - `config_legacy_with_count_tokens_api.json` → load OK；`countTokensApiUrl/Key/AuthType` 三字段被 serde 默认忽略
  - `config_legacy_with_count_tokens_api.json` → load → save → 重新读取文件文本 → 不再含 `countTokensApiUrl/Key/AuthType`（**Breaking Change 预期行为，必须断言**）
  - `config_minimum.json` → host=127.0.0.1, port=8080, region=us-east-1
  - `config_full.json` → admin_api_key.is_some(), proxy_url == "http://127.0.0.1:7890"
  - serialize-then-deserialize roundtrip → 等价（仅对新 schema 字段）
- **Verify**：测试存在并失败
- **Complexity**：Small

#### Step 1.4：实现 Config 子结构（GREEN）

- **Files**：`src/config/{mod,net,region,proxy,kiro,admin,endpoint,feature}.rs`（新建）
- **Action**：
  - 每个子结构 `#[derive(Deserialize, Serialize, Default)]` + `#[serde(rename_all = "camelCase")]`
  - 顶层 `Config { #[serde(flatten)] net: NetConfig, ... }`
  - 保留 `effective_auth_region()` / `effective_api_region()`、`load(path)`、`save()`、`config_path()` 公共 API
  - 不再有 `count_tokens_api_url / api_key / auth_type` 字段
- **Verify**：Step 1.3 测试全绿；`cargo build` 通过；老的 `model/config.rs` 还未删除（保留以便 Phase 2 平滑切换）
- **Complexity**：Medium

#### Step 1.5：domain 层基础 trait 占位

- **Files**：`src/domain/{mod,credential,token,endpoint,retry,selector,event}.rs`（新建，trait 定义 + 占位）
- **Action**：写出 trait / 数据类型的签名（实现留到 Phase 2-4）：
  - `Credential` 结构（用 `id: u64`、`auth: AuthMethod`、`region: Region`、`proxy: ProxyOverride`、…字段）
  - `AuthMethod::{Social, Idc, ApiKey}`
  - `TokenSource` trait（`async fn refresh(&self, cred: &Credential) -> Result<RefreshOutcome, RefreshError>`）
  - `KiroEndpoint` trait（与现 `kiro/endpoint/mod.rs` 一致，仅迁移）
  - `RetryPolicy` trait（`fn decide(&self, status: StatusCode, body: &str, attempt: usize) -> RetryDecision`）+ `RetryDecision::{Retry, FailoverCredential, DisableCredential, ForceRefresh, Fail}`
  - `CredentialView<'a>`：`{ id: u64, credential: &'a Credential, state: &'a CredentialState, stats: &'a CredentialStats }`，**`Send`/`Sync` 不强制**（只在锁内瞬态使用）
  - `CredentialSelector` trait：
    ```rust
    pub trait CredentialSelector: Send + Sync {
        /// 必须同步、纯计算：`candidates` 借用了 store/state/stats 锁内的数据，
        /// 实现禁止跨 `.await`、禁止再获取其他锁。
        fn select(&self, candidates: &[CredentialView<'_>], model: Option<&str>) -> Option<u64>;
    }
    ```
  - `KiroEvent` enum（迁移自 `kiro/model/events`）
- **Action 细节**：trait 方法暂可 `unimplemented!()`，仅保证签名完整 + 文档注释。新模块顶部加 `#![allow(dead_code)]` 直到 Phase 7 接入完成后逐个移除。
- **不引入 KiroClient trait**：handler 直接持具体类型 `Arc<KiroClient>`（YAGNI；不为 mock 抽象）。
- **Verify**：`cargo build` 通过；`cargo clippy --all-targets` 无 error（**不要求 `-D warnings`**，dead_code/unused 暂允许）
- **Complexity**：Medium

#### Step 1.6：Phase 1 检查点

- **Files**：N/A
- **Action**：运行 `cargo test`，确认新 domain/error 全绿、新 config 全绿；老 `model/config.rs` 保持不变也仍绿
- **Verify**：`cargo test` 全绿；`cargo clippy --all-targets` 无 error（中间 Phase 不要求 `-D warnings`，最终在 Phase 7/8 强制）
- **Complexity**：Small

---

### Phase 2：迁移基础设施（保留亮点 + 拆 token_manager）

> **重要**：本阶段产出新的 `infra/` 与 `service/credential_pool/`，但仍保留老 `kiro/` 不动，等 Phase 3 完成后再删除。

#### Step 2.1：迁移 parser（无逻辑变更）

- **Files**：`src/infra/parser/{mod,crc,decoder,error,frame,header}.rs`
- **Action**：将 `src/kiro/parser/*` 整体复制到 `src/infra/parser/`，仅修改文件中的 `use` 路径与文档注释中的模块引用。**不修改任何业务逻辑**。
- **Verify**：`cargo test infra::parser` 通过；老 `kiro::parser` 仍存在并通过
- **Complexity**：Small

#### Step 2.2：迁移 endpoint trait + ide 实现

- **Files**：`src/domain/endpoint.rs`、`src/infra/endpoint/{mod,ide}.rs`
- **Action**：
  - `src/domain/endpoint.rs` ← 迁移自 `kiro/endpoint/mod.rs` 的 trait + `RequestContext` + 默认 `is_monthly_request_limit` / `is_bearer_token_invalid` 实现
  - `src/infra/endpoint/ide.rs` ← 迁移自 `kiro/endpoint/ide.rs`
  - 保留全部现有单测
- **Verify**：`cargo test infra::endpoint` 通过
- **Complexity**：Small

#### Step 2.3：迁移 http_client + ProxyConfig

- **Files**：`src/infra/http/client.rs`
- **Action**：迁移 `src/http_client.rs` 内容（`ProxyConfig` + `build_client`）；保留单测
- **Verify**：`cargo test infra::http::client` 通过
- **Complexity**：Small

#### Step 2.4：迁移 Credential 数据模型

- **Files**：`src/domain/credential.rs`
- **Action**：以 `kiro/model/credentials.rs` 为基础迁移：
  - `KiroCredentials` 重命名为 `Credential`，结构体字段保持不变（serde 行为一致）
  - `CredentialsConfig::{Single, Multiple}` 迁移并改名为 `CredentialsFile::{Single, Multiple}`
  - 全部 30+ unit test 整体迁移并适配新模块路径
- **Verify**：`cargo test domain::credential` 通过
- **Complexity**：Medium

#### Step 2.5：infra/storage 三个文件存储

- **Files**：`src/infra/storage/{mod,credentials_file,stats_file,balance_cache}.rs`
- **Action**：从 `kiro/token_manager.rs` 抽取持久化逻辑：
  - `CredentialsFileStore`：负责凭据文件读 / 写（含单/多格式判定 + 仅多格式回写 + 优先级排序 + ID/machineId 补齐回写）
  - `StatsFileStore`：success_count + last_used_at 持久化（沿用 cache_dir 下 `kiro_stats.json`）
  - `BalanceCacheStore`：余额缓存读写（沿用 `kiro_balance_cache.json`，TTL 300s）
- **Test cases**：
  - 加载 `credentials_array_mixed.json` → 4 条凭据按 priority 排序
  - 加载后写回 → 文件内容字段顺序与原文件一致（pretty JSON）
  - 单对象格式加载后调用 `save()` → 不应回写（`is_multiple == false`）
  - stats roundtrip → success_count/last_used_at 保留
- **Verify**：`cargo test infra::storage` 全绿
- **Complexity**：Medium

#### Step 2.6：infra/machine_id 移除全局静态

- **Files**：`src/infra/machine_id.rs`
- **Action**：迁移 `kiro/machine_id.rs` 的 `normalize_machine_id` / `generate_from_credentials` / `sha256_hex` 函数；**删除** `FALLBACK_MACHINE_IDS` 静态。新增 `MachineIdResolver` 结构体持有 `Mutex<HashMap<Option<u64>, String>>`，由调用方（`CredentialPool`）持有实例。`generate_from_credentials` 改签为 `MachineIdResolver::resolve(&self, cred, config) -> String`。
- **Test cases**：保留全部现有测试，新增：
  - 不同 `MachineIdResolver` 实例之间 fallback 不互通
  - 同一 resolver 内同 credential.id 调用两次返回相同值
- **Verify**：`cargo test infra::machine_id` 通过、Step 2.6 测试全绿
- **Complexity**：Medium

#### Step 2.7：测试 Refresher 纯函数（RED）

- **Files**：`src/infra/refresher/{mod,social,idc,api_key}.rs`
- **Action**：**不发 HTTP、不 mock 上游**。把 refresher 内部能拆出来的纯函数单独测：
  - `classify_refresh_error(status: StatusCode, body: &str) -> RefreshError`：
    - 400 + body 含 `invalid_grant` → `RefreshError::TokenInvalid`
    - 401 → `RefreshError::Unauthorized`
    - 403 → `RefreshError::Forbidden`
    - 429 → `RefreshError::RateLimited`
    - 5xx → `RefreshError::ServerError`
  - `build_social_request_body(cred: &Credential) -> String`：构造的 JSON 字段一致（refresh_token、client_id、grant_type 等）
  - `build_idc_request_body(cred: &Credential) -> Result<String, RefreshError>`：缺 `clientId` 返回 `RefreshError::Unauthorized`
  - `parse_refresh_response(json: &str) -> Result<RefreshOutcome, RefreshError>`：成功解析 `{ access_token, refresh_token, profile_arn, expires_at }`；缺字段返回 `MalformedResponse`
  - `ApiKeyRefresher::refresh` 是 passthrough，可直接调（不发 HTTP）：返回入参 token
- **真实 HTTP 路径**：放到 Phase 8 手工冒烟（用真实凭据触发刷新一次）
- **Verify**：测试存在并失败
- **Complexity**：Small

#### Step 2.8：实现 Refresher（GREEN）

- **Files**：`src/infra/refresher/{social,idc,api_key}.rs`
- **Action**：基于 `kiro/token_manager.rs:142` 与 `:225` 的现有逻辑迁移；**HTTP 构造、错误判定、字段提取一律拆为纯函数**（`build_*_request_body`、`classify_refresh_error`、`parse_refresh_response`），`refresh()` 仅做"发请求 + 调 4 个纯函数 + 装配 RefreshOutcome"的薄壳。`api_key.rs` 实现 `TokenSource::refresh` 直接返回入参。
- **Verify**：Step 2.7 全部测试绿；`refresh()` 主体本身不写 unit test（无 mock，依赖 Phase 8 冒烟）
- **Complexity**：Medium

#### Step 2.9：测试 CredentialSelector 两种实现（RED）

- **Files**：`src/infra/selector/{mod,priority,balanced}.rs`
- **Action**：测试入参为 `&[CredentialView<'_>]`（同步纯计算）：
  - `PrioritySelector::select(&[]) → None`
  - `PrioritySelector::select(non-empty)` → 返回 priority 最小的 id（priority 来自 `view.credential.priority`）
  - `PrioritySelector` 在 model 含 "opus" 且 `view.credential.supports_opus() == false` 时跳过
  - `BalancedSelector::select` 返回 `view.stats.success_count` 最小者；平局看 `view.credential.priority`
  - 全部 disabled 的视图（`view.state.disabled == true`）传入前应已被 pool 过滤；selector 不重复过滤 disabled，但显式断言"selector 接收的 candidates 全部为 enabled" 通过 debug_assert
- **Verify**：测试失败
- **Complexity**：Small

#### Step 2.10：实现 Selectors（GREEN）

- **Files**：`src/infra/selector/{priority,balanced}.rs`
- **Action**：基于 `kiro/token_manager.rs:697` 现有 `select_next_credential` 逻辑拆分。selector **仅做 priority/balanced 排序与 opus 过滤**，disabled 过滤由 pool 在组装 view 时完成；selector 实现保持同步、纯计算（无 I/O、不 await）。
- **Verify**：Step 2.9 测试全绿
- **Complexity**：Small

#### Step 2.11：测试 CredentialState（失败计数 / 自愈）（RED）

- **Files**：`src/service/credential_pool/state.rs`
- **Action**：测试：
  - `report_failure` 累计 3 次后自动禁用，`disabled_reason == TooManyFailures`
  - `report_quota_exhausted` 立即禁用，`disabled_reason == QuotaExceeded`
  - `report_refresh_token_invalid` 立即禁用，`disabled_reason == InvalidRefreshToken`
  - `report_success` 重置 failure_count
  - 当全部凭据被 `TooManyFailures` 禁用时，`acquire` 触发自愈，重置失败计数并重新启用
  - `set_disabled(false)` 重置 disabled_reason
- **Verify**：测试存在失败
- **Complexity**：Medium

#### Step 2.12：实现 CredentialState（GREEN）

- **Files**：`src/service/credential_pool/state.rs`
- **Action**：抽取 `kiro/token_manager.rs` 中的 `report_*` 方法 + `disabled_reason` 枚举 + 自愈逻辑成独立结构。
  - 内部数据结构按 **id → 状态** 的 `HashMap<u64, EntryState>` 而不是 `Vec<EntryState>`，避免与 store 的 Vec 索引耦合（参见 Step 2.14 的 join 顺序约束）
  - 锁仅 1 把（`parking_lot::Mutex<HashMap<u64, EntryState>>`）
  - `EntryState` 字段：`failure_count`, `refresh_failure_count`, `disabled`, `disabled_reason`，**不含凭据数据本身**（数据由 store 持有）
  - 单独再开一份 `HashMap<u64, EntryStats>`（含 `success_count`, `last_used_at`），由 `StatsFileStore` 持久化；与 state 一起按 id 组装到 view 中
- **Verify**：Step 2.11 测试全绿
- **Complexity**：Medium

#### Step 2.13：CredentialStore（增删改 + 持久化协调）

- **Files**：`src/service/credential_pool/store.rs`
- **Action**：
  - 持有 `Mutex<HashMap<u64, Credential>>` + `MachineIdResolver` + `Arc<dyn CredentialsFileStore>`（key 为 id，回写时按 priority 排序后落盘以保留文件中字段顺序）
  - 提供 `add(Credential) -> id`, `remove(id)`, `get(id) -> Option<Credential>`, `update_token(id, RefreshOutcome)`, `set_priority(id, u32)`, `set_endpoint(id, String)` 等
  - ID 分配（沿用 `max_id + 1` 策略）
  - 加载时补齐 ID/machineId 后回写
  - **配置无效（`auth_method=api_key` 但缺 `kiroApiKey`）→ 加载时不报错；store 仍返回该条凭据，但通过返回值附带 `Vec<ValidationIssue { id, kind: InvalidConfig, message: String }>`，由 pool 在装载时调用 `state.set_disabled(id, DisabledReason::InvalidConfig)`，启动**继续**（与现行 token_manager.rs:606-622 行为完全一致）。
- **Test cases**：
  - 加载 `credentials_array_mixed.json` → 4 条
  - 加载 → 添加 1 条 → 写回 → 重新加载 → 5 条且新条有 ID
  - 加载单对象格式 → 1 条；写回时不修改文件内容
  - **api_key 凭据缺 kiroApiKey → 加载成功，store 返回 1 条凭据 + 1 条 `ValidationIssue::InvalidConfig`**；pool 装载后该条目 `disabled=true, reason=InvalidConfig`，启动不失败
- **Verify**：测试全绿
- **Complexity**：Medium

#### Step 2.14：CredentialPool 门面

- **Files**：`src/service/credential_pool/{mod,pool}.rs`
- **Action**：组装 store + state + selector + refresher + machine_id_resolver。对外 API：
  - `acquire(model: Option<&str>) -> Result<CallContext, ProviderError>`
  - `report_success(id)`, `report_failure(id) -> bool`, `report_quota_exhausted(id) -> bool`, `report_refresh_failure(id) -> bool`, `report_refresh_token_invalid(id) -> bool`
  - `force_refresh(id) -> Result<(), RefreshError>`
  - `snapshot() -> ManagerSnapshot`
  - `add_credential(req) -> Result<u64>`, `delete_credential(id) -> Result<()>`, `set_disabled(id, bool) -> Result<()>`, `set_priority(id, u32) -> Result<()>`, `reset_and_enable(id) -> Result<()>`
  - `get_usage_limits_for(id) -> Result<UsageLimitsResponse, KiroError>`
  - `get_load_balancing_mode() -> &str`, `set_load_balancing_mode(mode) -> Result<()>`
- **Action 细节**：
  - 锁顺序固定：`store -> state -> stats`（避免死锁）；**禁止反向获取**
  - **acquire 视图组装**（关键约束）：在持有 store / state / stats 三把锁期间，按 `store.iter().map(|(id, cred)| { let st = state.get(id)?; let stt = stats.get(id)?; CredentialView { id, credential: cred, state: st, stats: stt } })` 拼装，**按 id join，不假设 Vec index 对齐**；先在拼装期内过滤 `disabled == false` 与 opus 支持，再传入 `selector.select(&views, model)`
  - `selector.select(...)` 返回 `Option<u64>` 后释放三把锁，再在外层执行 token 刷新 / I/O（**禁止跨 .await 持锁**）
  - `acquire_context` 整体逻辑参照 `kiro/token_manager.rs:755`，但调度循环不再嵌入自愈，自愈下推到 selector/state 协作
  - `current_id` 仅在 priority 模式有效，balanced 模式每次重新选
  - 只暴露上述 API，**不再**直接暴露 `entries` 锁或内部结构
- **Test cases**（**纯函数 / 纯状态转移单测，无 HTTP，无 mock**）：
  - `acquire(model)` 在内存内（不发 HTTP）单凭据可用时返回该凭据 id
  - `acquire(model)` 全部凭据 disabled 时返回 `ProviderError::AllCredentialsExhausted { available: 0, total }`
  - `acquire(model)` balanced 模式下连续调用 6 次（不发 HTTP）：在 2 个凭据间均匀分布
  - `acquire(model)` priority 模式下 `current_id` 命中时返回该 id；不命中时 fallback selector
  - `report_quota_exhausted(id)` 后立即 `acquire` → 切到下一条；当所有都 quota 时 → exhausted
  - 自愈：全部 `TooManyFailures` 后 `acquire` 重置失败计数并重新启用，返回某个 id
- **真实 HTTP 路径（force_refresh、call_api 重试）**：放到 Phase 8 手工冒烟，**不在此处写 mock 测试**
- **Verify**：测试全绿
- **Complexity**：Medium

#### Step 2.15：Phase 2 检查点

- **Files**：N/A
- **Action**：运行 `cargo test`，所有现有测试 + 新建 Phase 1-2 测试都绿。**老 `kiro/` 模块仍然存在、未被使用**。
- **Verify**：`cargo test` 全绿；`cargo clippy --all-targets` 无 error（中间 Phase 不要求 `-D warnings`）
- **Complexity**：Small

---

### Phase 3：KiroClient（RequestExecutor + RetryPolicy）

#### Step 3.1：测试 DefaultRetryPolicy（RED）

- **Files**：`src/infra/http/retry.rs`
- **Action**：测试 `DefaultRetryPolicy::decide(status, body, attempt)`：
  - 200 → `RetryDecision::Success`（或上层不走 decide）
  - 400 → `Fail`
  - 401/403 + bearer invalid → `ForceRefresh`
  - 401/403 + 普通 → `FailoverCredential`
  - 402 + MONTHLY_REQUEST_COUNT → `DisableCredential(QuotaExceeded)`
  - 408/429/5xx → `Retry { backoff: Duration }`
  - 其他 4xx → `Fail`
  - attempt 越大退避越长（指数退避 + 抖动），上限 2 s
- **Verify**：测试失败
- **Complexity**：Small

#### Step 3.2：实现 DefaultRetryPolicy（GREEN）

- **Files**：`src/infra/http/retry.rs`
- **Action**：照 Step 3.1 测试实现；退避算法移植自 `kiro/provider.rs:509 retry_delay`（`200ms * 2^attempt`，max 2 s，jitter ±25%）。常量 `MAX_RETRIES_PER_CREDENTIAL=3`、`MAX_TOTAL_RETRIES=9` 提到 `policy` 上
- **Verify**：Step 3.1 测试全绿
- **Complexity**：Small

#### Step 3.3：测试 RequestExecutor 纯逻辑（RED）

- **Files**：`src/infra/http/executor.rs`
- **Action**：**不发 HTTP、不 mock 上游**。把 executor 内部能拆出来的纯函数 / 纯逻辑单独测：
  - `EndpointRegistry::resolve_for(&credential)`：凭据 endpoint 字段命中 → 返回对应实现；缺失 → fallback default；未注册的 endpoint 名 → `ProviderError::EndpointResolution`
  - `compute_attempt_outcome(decision: RetryDecision, ctx: &CallContext, attempt_state: &mut AttemptState) -> AttemptOutcome`（纯状态转移函数，把 `policy.decide` 的结果映射到下一步动作：retry / failover / refresh / fail / success）
  - 退避 sleep 时长选择：抽出 `next_backoff(attempt: usize) -> Duration`（已在 Step 3.1 测过，可复用）
- **真实 HTTP 路径（acquire → send → decide → 应用）**：放到 Phase 8 手工冒烟，**不在此处写 mock 测试**
- **Verify**：测试失败
- **Complexity**：Small

#### Step 3.4：实现 RequestExecutor（GREEN）

- **Files**：`src/infra/http/executor.rs`
- **Action**：参考 `kiro/provider.rs:279 call_api_with_retry` 与 `:129 call_mcp_with_retry`，**只写一份**：
  ```rust
  pub async fn execute(
      &self,
      kind: EndpointKind, // Api | Mcp
      body: &str,
      model: Option<&str>,
      pool: &CredentialPool,
      endpoints: &EndpointRegistry,
      policy: &dyn RetryPolicy,
  ) -> Result<reqwest::Response, ProviderError>
  ```
  - 内部循环顺序固定：
    ```
    let ctx = pool.acquire(model).await?;
    let endpoint = endpoints.resolve_for(&ctx.credentials)?;
    let rctx = RequestContext { credentials: &ctx.credentials, ... };
    let url = endpoint.url(kind, &rctx);
    let body_transformed = endpoint.transform_body(kind, body, &rctx);
    let request = endpoint.decorate(kind, base, &rctx);
    let response = client.send(request).await;
    let decision = policy.decide(status, &body_text, attempt);
    apply(decision, &mut ctx, pool); // report/refresh/retry/failover/fail
    ```
  - 每次 retry / failover **重新 acquire → 重新 resolve endpoint**（凭据级 endpoint 字段在每次循环内重新生效）
  - `client_for(credential)` 缓存按 effective_proxy 维度，由 executor 自身持有
- **Verify**：Step 3.3 测试全绿
- **Complexity**：Medium

#### Step 3.5：KiroClient 编排

- **Files**：`src/service/kiro_client.rs`
- **Action**：
  ```rust
  pub struct KiroClient {
      executor: Arc<RequestExecutor>,
      pool: Arc<CredentialPool>,
      endpoints: Arc<EndpointRegistry>,
      policy: Arc<dyn RetryPolicy>,
  }

  impl KiroClient {
      pub async fn call_api(&self, body: &str, model: Option<&str>) -> Result<reqwest::Response, ProviderError> {
          self.executor.execute(EndpointKind::Api, body, model, &self.pool, &self.endpoints, &*self.policy).await
      }
      pub async fn call_api_stream(&self, body: &str, model: Option<&str>) -> Result<reqwest::Response, ProviderError> { /* 同上，stream 不缓冲 */ }
      pub async fn call_mcp(&self, body: &str, model: Option<&str>) -> Result<reqwest::Response, ProviderError> { /* EndpointKind::Mcp */ }
      pub fn pool(&self) -> Arc<CredentialPool> { self.pool.clone() }
  }
  ```
  - **不再引入 `domain::client::KiroClient` trait**：handler 直接持 `Arc<KiroClient>` 具体类型（YAGNI）
  - **EndpointRegistry** 内部持有 `default_endpoint: String` + `endpoints: HashMap<String, Arc<dyn KiroEndpoint>>`，对外仅暴露 `resolve_for(&Credential) -> Result<Arc<dyn KiroEndpoint>, ProviderError>`，让"按凭据 endpoint 字段查注册表 / 否则用 default" 的语义集中在一个地方
  - `call_api_stream` 与 `call_api` 共享 executor，差异仅在不缓冲响应
- **Verify**：Step 3.1-3.3 纯函数测试全绿；KiroClient 端到端调用路径在 Phase 8 冒烟
- **Complexity**：Small

---

### Phase 4：Conversation 协议层

#### Step 4.1：测试 ProtocolConverter（RED）

- **Files**：`src/service/conversation/converter.rs` + `tools.rs` + `thinking.rs`
- **Action**：从 `anthropic/converter.rs` 现有逻辑提取测试用例：
  - `convert_request` 空 messages → `ConversionError::EmptyMessages`
  - `convert_request` 不支持的模型 → `ConversionError::UnsupportedModel`
  - `convert_request` 多轮 user/assistant + tools → 输出 KiroRequest 字段一一对应（手写 fixture）
  - `merge_user_messages` 连续 user → 合并
  - `merge_assistant_messages` 同上
  - `validate_tool_pairing` 缺失 tool_use_id 的 tool_result → 错误
  - `remove_orphaned_tool_uses` → 移除无配对 tool_use
  - `shorten_tool_name`、`map_tool_name` 长度 / 字符约束
  - `extract_session_id` 从 `metadata.user_id` 提取 UUID
  - `normalize_json_schema` 移除非标准字段
- **Verify**：测试失败
- **Complexity**：Medium

#### Step 4.2：实现 ProtocolConverter（GREEN）

- **Files**：`src/service/conversation/{converter,tools,thinking}.rs`
- **Action**：把 `anthropic/converter.rs` 的 24 个顶层 fn 拆到三个子模块：
  - `converter.rs`：`ProtocolConverter` 结构 + `convert_request` 主入口 + `build_history` + `process_message_content`
  - `tools.rs`：`convert_tools`, `shorten_tool_name`, `map_tool_name`, `validate_tool_pairing`, `remove_orphaned_tool_uses`, `extract_tool_result_content`, `create_placeholder_tool`, `collect_history_tool_names`
  - `thinking.rs`：`generate_thinking_prefix`, `has_thinking_tags`, 还有 `find_real_thinking_*` 5 个字符串扫描函数（迁移自 `anthropic/stream.rs`）
- **Verify**：Step 4.1 测试全绿；converter.rs 单文件不超过 600 行
- **Complexity**：Large

#### Step 4.3：测试本地 count_tokens（async fn）（RED）

- **Files**：`src/service/conversation/tokens.rs`
- **Action**：
  - 中文文本 token 估算 = 现 `count_tokens` 算法相同（保持等价）
  - 英文文本同上
  - 空文本 → 至少 1 token
  - 包含工具的请求估算包括 tool name + description + input_schema
  - **不再测试远程 API 路径**（已删除）
- **Verify**：测试失败
- **Complexity**：Small

#### Step 4.4：实现本地 count_tokens（GREEN）

- **Files**：`src/service/conversation/tokens.rs`
- **Action**：从 `src/token.rs` 迁移 `count_tokens` / `count_all_tokens_local` / `estimate_output_tokens` / `is_non_western_char` 这 4 个函数，**不要**全局静态、**不要**远程 API 调用、签名改 `async fn`（虽内部纯计算，签名 async 让 handler 能 `.await` 后无缝切换到 spawn_blocking 等优化）
- **Verify**：Step 4.3 测试全绿
- **Complexity**：Small

#### Step 4.5：测试 ThinkingExtractor（RED）

- **Files**：`src/service/conversation/thinking.rs`（追加测试）
- **Action**：
  - 流式 chunk 顺次喂入 `ThinkingExtractor`，正确产出 `(thinking, text)` 段
  - 完整文本 `<thinking>...</thinking>...` → 正确切分
  - 引号干扰：`<thinking>...</thinking with "quote">...</thinking>` 仅末尾真正闭合标签生效
  - `</thinking>` 在 chunk 边界断开时不丢失
- **Verify**：测试失败
- **Complexity**：Medium

#### Step 4.6：实现 ThinkingExtractor（GREEN）

- **Files**：`src/service/conversation/thinking.rs`
- **Action**：抽取 `anthropic/stream.rs` 顶部的 5 个字符串函数 + `extract_thinking_from_complete_text`，封装为 `ThinkingExtractor` 状态机；流式与非流式共用
- **Verify**：Step 4.5 测试全绿
- **Complexity**：Medium

#### Step 4.7：测试 EventReducer（RED）

- **Files**：`src/service/conversation/reducer.rs`
- **Action**：
  - 喂入序列 `assistantResponse → toolUse → contextUsage → assistantResponse → end` → 产出符合 Anthropic SSE 顺序的事件
  - `message_start` 含正确 input_tokens 估算值
  - tool_use 增量 JSON 累积 + stop=true 时聚合
  - thinking 块开闭事件
  - `Exception::ContentLengthExceededException` → stop_reason="max_tokens"
  - `contextUsage_percentage >= 100` → stop_reason="model_context_window_exceeded"
- **Verify**：测试失败
- **Complexity**：Medium

#### Step 4.8：实现 EventReducer（GREEN）

- **Files**：`src/service/conversation/reducer.rs`
- **Action**：合并 `anthropic/stream.rs` 的 `SseStateManager` + `StreamContext::process_kiro_event` 为单一 `EventReducer`：
  ```
  pub struct EventReducer { ... }
  impl EventReducer {
      pub fn new(model: &str, input_tokens: i32, thinking_enabled: bool, tool_name_map: HashMap<String, String>) -> Self;
      pub fn initial_events(&mut self) -> Vec<SseEvent>;
      pub fn on_kiro_event(&mut self, ev: &KiroEvent) -> Vec<SseEvent>;
      pub fn finalize(&mut self) -> Vec<SseEvent>;
      pub fn correct_input_tokens(&mut self, actual: i32);
  }
  ```
- **Verify**：Step 4.7 测试全绿；reducer.rs 单文件 ≤ 600 行
- **Complexity**：Large

#### Step 4.9：测试 SseDelivery（RED）

- **Files**：`src/service/conversation/delivery.rs`
- **Action**：
  - `LiveDelivery` 行为：每收到事件立即推 SSE，每 25 s 推 ping
  - `BufferedDelivery` 行为：等流结束后，调 `reducer.correct_input_tokens(...)` 修正再批量推；期间每 25 s ping
  - 上游错误（中途 chunk 错误）→ 两种模式都把已积累事件 finalize 后输出
- **Verify**：测试失败
- **Complexity**：Medium

#### Step 4.10：实现 SseDelivery（GREEN）

- **Files**：`src/service/conversation/delivery.rs`
- **Action**：定义 trait + 两个实现，把 `anthropic/handlers.rs:345 create_sse_stream` + `:856 create_buffered_sse_stream` 抽象到此处。Handler 后续直接 `delivery.run(response_stream).await`。
- **Verify**：Step 4.9 测试全绿
- **Complexity**：Large

#### Step 4.11：迁移 WebSearch 旁路

- **Files**：`src/service/conversation/websearch.rs`
- **Action**：把 `anthropic/websearch.rs` 整体迁移到此处；调整 `KiroClient::call_mcp` 调用位置；保留全部既有行为（一次性内置 WebSearch 工具检测、输入 tokens 估算、特殊 SSE 序列）
- **Verify**：`cargo build` 通过；后续 Phase 5 会走端到端验证
- **Complexity**：Medium

#### Step 4.12：迁移 KiroEvent + Kiro 请求模型

- **Files**：`src/domain/event.rs` + `src/domain/request.rs`（新建）
- **Action**：
  - `domain/event.rs` ← 迁移 `kiro/model/events/{base,assistant,tool_use,context_usage}.rs`
  - `domain/request.rs` ← 迁移 `kiro/model/requests/{conversation,kiro,tool}.rs`
  - 保留全部 `EventPayload` trait 与 enum 结构
- **Verify**：`cargo test domain::event` 通过；老 `kiro::model` 仍存在
- **Complexity**：Small

---

### Phase 5：interface/http – Anthropic 层

#### Step 5.1：迁移 auth.rs

- **Files**：`src/interface/http/auth.rs`
- **Action**：迁移 `common/auth.rs` 的 `extract_api_key` 与 `constant_time_eq`，单元测试保留
- **Verify**：`cargo test interface::http::auth` 通过
- **Complexity**：Small

#### Step 5.2：HTTP 错误映射

- **Files**：`src/interface/http/error.rs`
- **Action**：实现 `From<KiroError> for axum::Response`：
  - `ProviderError::ContextWindowFull` → 400 + `invalid_request_error` "Context window is full..."
  - `ProviderError::InputTooLong` → 400 + 类似消息
  - `ProviderError::AllCredentialsExhausted { .. }` → 502 + `api_error`
  - `ProviderError::EndpointResolution(_)` → 503 + `service_unavailable`
  - `RefreshError::TokenInvalid` → 502 + `api_error`
  - 默认 → 502 + `api_error`
- **Test cases**：每种 KiroError 变体 → 对应 status + JSON envelope
- **Verify**：测试全绿
- **Complexity**：Medium

#### Step 5.3：迁移 DTO + 自定义 deserializer

- **Files**：`src/interface/http/anthropic/dto.rs`
- **Action**：迁移 `anthropic/types.rs` 全部 struct + `system` 字段的 string|array 自定义 deserializer + Thinking budget 上限处理
- **Verify**：现有 dto roundtrip 测试通过
- **Complexity**：Small

#### Step 5.4：handler 拆纯函数（RED）

- **Files**：`src/interface/http/anthropic/handlers.rs`
- **Action**：**不为 handler 写 mock 测试**。把 handler 内部分支判断 / 数据预处理逻辑能拆为纯函数的全部拆出来单测：
  - `should_use_buffered_delivery(path: &str) -> bool`：`/cc/v1/messages` → true；`/v1/messages` → false
  - `apply_thinking_model_suffix(model: &str, thinking_enabled: bool) -> String`：thinking 启用时追加后缀
  - `is_web_search_request(req: &MessagesRequest) -> bool`：检测 WebSearch 工具
  - `map_provider_error_to_response(err: &ProviderError) -> (StatusCode, ErrorEnvelope)`：错误映射（与 `interface/http/error.rs` 复用，Step 5.2 已测）
- **handler 主体**（`post_messages_impl` 等）：**不写 unit test**，靠 Phase 8 真实启动冒烟覆盖
- **Verify**：纯函数测试失败
- **Complexity**：Small

#### Step 5.5：实现统一 messages handler（GREEN）

- **Files**：`src/interface/http/anthropic/handlers.rs`
- **Action**：
  ```
  async fn post_messages_impl<D: SseDelivery>(state, payload, delivery: D) -> Response
  pub async fn post_messages(State<AppState>, Json<MessagesRequest>) -> Response { post_messages_impl(state, payload, LiveDelivery::new()).await }
  pub async fn post_messages_cc(State<AppState>, Json<MessagesRequest>) -> Response { post_messages_impl(state, payload, BufferedDelivery::new()).await }
  ```
  - 共享逻辑：thinking 覆写、WebSearch 检测、convert_request、token 估算（`.await` count_tokens）、KiroClient.call_api_stream / call_api、错误映射
  - 单文件 ≤ 600 行（预期 ~250 行，比原 938 行大幅简化）
- **Verify**：Step 5.4 纯函数测试全绿；handler 主体在 Phase 8 冒烟
- **Complexity**：Medium

#### Step 5.6：models handler + count_tokens handler

- **Files**：`src/interface/http/anthropic/{models,handlers}.rs`
- **Action**：迁移 `GET /v1/models` 模型列表（保留全部 10 个模型 ID 与字段）；`POST /v1/messages/count_tokens` / `POST /cc/v1/messages/count_tokens` 直接调 `tokens::count_all_tokens(...).await`
- **Verify**：手动 curl 测试模型列表字段一致；count_tokens 测试通过
- **Complexity**：Small

#### Step 5.7：anthropic router

- **Files**：`src/interface/http/anthropic/router.rs`
- **Action**：迁移 `anthropic/router.rs`，组装 v1 + cc/v1 + auth middleware + cors + body limit
- **Verify**：`cargo test` 通过；端到端测试见 Phase 8
- **Complexity**：Small

---

### Phase 6：interface/http – Admin 层

#### Step 6.1：迁移 admin DTO + error

- **Files**：`src/interface/http/admin/dto.rs` + `src/service/admin/error.rs`
- **Action**：迁移 `admin/types.rs` + `admin/error.rs`，结构不变；`AdminServiceError` 改为接受 `From<ProviderError>` / `From<RefreshError>` / `From<ConfigError>` 自动转换
- **Verify**：现有 admin 测试 + 新 From 测试全绿
- **Complexity**：Small

#### Step 6.2：测试 AdminService 去字符串匹配（RED）

- **Files**：`src/service/admin/service.rs`
- **Action**：
  - `force_refresh` 上游返回 invalid_grant → AdminServiceError::InvalidCredential（不再用 `msg.contains("已被截断")`）
  - `add_credential` 重复 → InvalidCredential（结构化）
  - `delete_credential` 凭据存在但启用 → InvalidCredential
  - `delete_credential` 凭据不存在 → NotFound
  - `get_balance` 上游 502 → UpstreamError
- **Verify**：测试失败（因为旧 service.rs 仍用字符串匹配，但新 service.rs 应实现结构化）
- **Complexity**：Medium

#### Step 6.3：实现新 AdminService（GREEN）

- **Files**：`src/service/admin/service.rs`
- **Action**：
  - 调用 `CredentialPool` 的方法直接返回结构化错误（`Result<T, ProviderError>` / `Result<T, RefreshError>` 等）
  - **删除全部 4 个 `classify_*_error` 函数**
  - `From<ProviderError> for AdminServiceError` / `From<RefreshError>` 自动转换
  - 余额缓存逻辑迁移到 `service/admin/balance.rs`，仍用 `BalanceCacheStore`
- **Verify**：Step 6.2 测试全绿；service.rs 单文件 ≤ 400 行（预期 ~250 行，原 457 行）
- **Complexity**：Medium

#### Step 6.4：admin handlers + router

- **Files**：`src/interface/http/admin/{handlers,router}.rs`
- **Action**：迁移 `admin/handlers.rs` + `admin/router.rs`，10 个端点路径方法不变；handler 内部调新 AdminService
- **Verify**：`cargo build` 通过
- **Complexity**：Small

#### Step 6.5：admin_ui 迁移

- **Files**：`src/interface/http/ui.rs`
- **Action**：迁移 `admin_ui/router.rs` 内容（含 `Asset rust_embed`、`get_cache_control`、`is_asset_path`），逻辑零改动
- **Verify**：手动浏览器访问 `/admin` 静态页面正常
- **Complexity**：Small

---

### Phase 7：装配 + 删除老代码

#### Step 7.1：新 main.rs 装配

- **Files**：`src/main.rs`
- **Action**：薄入口：
  ```
  fn main() -> ExitCode {
      let args = Args::parse();
      init_tracing();
      let config = Config::load(...)?;
      let credentials = CredentialsFileStore::load(...)?;
      let pool = build_credential_pool(config, credentials);
      let endpoints = register_endpoints();
      let client = KiroClient::new(executor, pool, endpoints, default_endpoint, policy);
      let app = build_app(config.clone(), client);
      tokio::main { axum::serve(...).await }
  }
  ```
  - 装配工厂函数放 `src/main.rs` 内或 `src/wiring.rs`
  - 启动日志保留：列出 5 个 Anthropic 端点 + 10 个 Admin 端点（如启用）
- **Verify**：`cargo run` 启动成功；启动日志格式不变
- **Complexity**：Medium

#### Step 7.2：删除老模块

- **Files**：删除以下目录/文件：
  - `src/anthropic/` 整个
  - `src/admin/` 整个（不含 admin_ui）
  - `src/admin_ui/` 整个（已迁移到 interface/http/ui.rs）
  - `src/common/` 整个（已迁移到 interface/http/auth.rs）
  - `src/kiro/` 整个
  - `src/model/` 整个（已迁移到 config/ 和 interface/cli.rs）
  - `src/http_client.rs`、`src/token.rs`、`src/debug.rs`、`src/test.rs`
- **Action**：用 `git rm -r ...` 删除；`main.rs` 的 `mod xxx` 同步移除
- **Verify**：`cargo build` 通过；`cargo test` 全绿；`find src -name '*.rs' | wc -l` 显示新结构文件数（预期 ~50-60，比原 36 略多但单文件更小）
- **Complexity**：Small

#### Step 7.3：清理 Cargo.toml

- **Files**：`Cargo.toml`
- **Action**：核对依赖：
  - 确认 `thiserror` 在生产依赖
  - 确认 `crc`, `bytes`, `parking_lot`, `subtle`, `urlencoding`, `tokio::sync` 等仍被新代码使用
  - 移除任何不再使用的 crate
- **Verify**：`cargo build --release` 通过、`cargo build --no-default-features` 通过
- **Complexity**：Small

---

### Phase 8：手工冒烟 + 负载验证

> **不引入** `tests/` 集成测试目录、**不引入任何 mock**。原 Step 8.1-8.3 的契约保护：
> - 配置兼容：在 Phase 1 Step 1.3 的 `src/config/...` 纯函数单测中加载 fixture 并断言（含老 fixture 的 serde 忽略 + save 后字段消失）
> - Anthropic / Admin 端点契约：handler 主体行为**完全靠下面 Step 8.1 真实启动冒烟覆盖**；handler 内部可拆出的分支判断 / 数据预处理已在 Step 5.4 / 6.x 作为纯函数单测覆盖
>
> Phase 8 仅做"真实启动后冒烟 + 负载行为 + 基线对照"。

#### Step 8.1：手工端到端冒烟测试

- **Files**：N/A
- **Action**（按 README 顺序）：
  - `cargo build --release`
  - `pnpm -C admin-ui build`（确保前端产物存在）
  - 用本地真实 `config.json` + `credentials.json` 启动
  - `curl -i http://127.0.0.1:8990/v1/models` → 字段名一致、status 200、关键 header 一致
  - `curl -i -X POST http://127.0.0.1:8990/v1/messages -H 'x-api-key: ...' -d '...'` 流式 / 非流式各跑一次；与 master 二进制下抓取的响应做"语义比较"（字段名 + SSE 事件序列 + 关键字段值；忽略 message id / 时间戳 / Date header）
  - 浏览器访问 `http://127.0.0.1:8990/admin` → 页面正常加载、能看到凭据列表
  - 在 Admin UI 上：禁用 → 启用 → 修改优先级 → 强制刷新 → 加载余额（依赖真实凭据，可跳过）
  - `curl -i http://127.0.0.1:8990/api/admin/credentials -H 'x-api-key: <admin>'` → 200
- **Verify**：所有响应字段名 + HTTP 关键 header 与 master 分支一致；动态项允许差异
- **Complexity**：Medium

#### Step 8.2：负载行为验证

- **Files**：N/A（脚本式手工测试）
- **Action**：
  - `MAX_TOTAL_RETRIES=9` 上限：用故意失效的真实凭据（refreshToken 改坏）让上游全部 401 → 9 次重试后 502
  - 自愈：用 2 条故意失效的凭据，观察"两条凭据各失败 3 次 → 自愈一次再失败 → 502"的日志序列
  - 退避时间：日志中观察相邻重试间 200ms → 400ms → 800ms → 1.6s → 2s（含抖动）
- **Verify**：日志事件数量 + 时间间隔与现状一致
- **Complexity**：Medium

#### Step 8.3：基线对照（性能 / 内存）

- **Files**：N/A
- **Action**：在同一机器上对比 master 与 refactor/v2：
  - `cargo build --release` 后二进制大小（应在 ±5% 内）
  - `cargo run --release` 启动到接收 1 个请求的内存占用（`ps -o rss`）
  - 流式 100 个 chunk 的处理延迟（`curl -w '%{time_total}'`）
- **Verify**：偏差在 ±10% 内（大改动允许小幅波动）
- **Complexity**：Small

#### Step 8.4：最终静态检查

- **Files**：N/A
- **Action**：Phase 7 完工后运行：
  - `cargo build --release`
  - `cargo build --no-default-features`
  - `cargo test`（含全部 `#[cfg(test)] mod tests`）
  - `cargo clippy --all-targets -- -D warnings`（**最终关卡**：旧模块已删、新模块全部接入，dead_code/unused 必须为零）
- **Verify**：四条命令全绿
- **Complexity**：Small

---

## Test Strategy

### Automated Tests

#### Unit Tests（嵌入在源码 `#[cfg(test)] mod tests`）

| 模块 | 主要用例 | 类型 |
| - | - | - |
| `domain::error` | 4 类错误的 Display / From / kind | Unit |
| `config::*` | 加载老 / 新格式 + roundtrip | Unit |
| `domain::credential` | 30+ 现有凭据测试（迁移） | Unit |
| `infra::parser::*` | 现有 decoder/frame/header 测试（迁移） | Unit |
| `infra::endpoint::ide` | inject_profile_arn + monthly_request_limit | Unit |
| `infra::http::client` | ProxyConfig + build_client（迁移） | Unit |
| `infra::http::retry` | DefaultRetryPolicy 状态码分类 | Unit |
| `infra::http::executor` | EndpointRegistry::resolve_for + 状态转移纯函数 | Unit（纯函数） |
| `infra::refresher::*` | classify_refresh_error + build_*_request_body + parse_refresh_response | Unit（纯函数） |
| `infra::selector::*` | priority / balanced / opus 过滤 | Unit |
| `infra::machine_id` | uuid 归一 + fallback 隔离 | Unit |
| `infra::storage::*` | 凭据 / 统计 / 余额 cache 文件 | Unit |
| `service::credential_pool::state` | 失败计数 / 自愈 / 禁用原因 | Unit |
| `service::credential_pool::pool` | 内存内 acquire / 状态转移（无 HTTP） | Unit（纯逻辑） |
| `service::conversation::converter` | 24 个原 fn 拆分后的等价 | Unit |
| `service::conversation::thinking` | 标签状态机（流式 + 完整） | Unit |
| `service::conversation::reducer` | 事件序列 → SSE | Unit |
| `service::conversation::delivery` | Live + Buffered 行为 | Unit |
| `service::conversation::tokens` | 中文 / 英文 / 工具估算 | Unit |
| `service::admin::service` | 结构化错误映射（无字符串匹配） | Unit |
| `interface::http::auth` | API Key 提取（迁移） | Unit |
| `interface::http::error` | KiroError → status mapping | Unit |
| `interface::http::anthropic::dto` | system 字段 string|array | Unit |
| `interface::http::anthropic::handlers` | 分支判断 / 数据预处理纯函数 | Unit（纯函数） |

> **不写 mock 测试**。组合层（refresher.refresh、executor.execute、handler 主体、KiroClient.call_*）的端到端行为完全靠 Phase 8 真实启动冒烟覆盖。

#### Integration Tests

**不引入** `tests/` 目录、**不引入任何 mock**。原契约保护：
- 配置兼容性：在 `src/config/...` 纯函数 unit test 中通过 `include_str!` 加载 fixture，断言加载 OK + save 后老字段消失（Step 1.3）
- Anthropic / Admin 端点契约：handler 主体行为靠 Phase 8 Step 8.1 真实启动冒烟覆盖；handler 内部可拆出的纯函数（分支判断、数据预处理）已在 Step 5.4 / 6.x 单测覆盖

### Manual Verification

- [ ] 用本地真实 `config.json` + `credentials.json` 启动 master 与 refactor/v2 二进制，分别 curl 抓取响应做"语义比较"：JSON 字段名一致、HTTP 关键 header（Content-Type、CORS、鉴权）一致、SSE 事件序列与关键字段值一致；动态项（message id、时间戳、Date/Server header）允许差异
- [ ] 浏览器访问 `/admin`，全功能交互（添加 / 删除 / 禁用 / 启用 / 优先级 / 余额 / 负载均衡模式）
- [ ] `cargo build --no-default-features`（musl 路径）成功
- [ ] **Phase 7/8 完工后** `cargo clippy --all-targets -- -D warnings` 无警告（中间 Phase 仅要求 `cargo clippy --all-targets` 无 error）
- [ ] 启动后 `tracing` 日志格式与 master 分支视觉一致

## Risks and Mitigations

| 风险 | 影响 | 缓解 |
| - | - | - |
| 删除 `count_tokens_*` 三字段（**Breaking Change**）：旧 `config.json` save 后字段消失 | 用户配置文件 diff、用户感知 | serde 默认忽略未知字段使加载不失败（Step 1.3 测试覆盖）；CHANGELOG 必须显式标注；README 同步移除相关章节；首次 save 后字段消失为预期行为 |
| 锁顺序变化导致死锁 | 运行时挂起 | 新 `CredentialPool` 锁顺序固定 `store -> state -> stats`，禁止反向获取，加 doc comment |
| `MAX_RETRIES_PER_CREDENTIAL`、退避魔法数偏移 | 上游打挂 / 体验下降 | 测试覆盖；常量保留于 `infra/http/retry.rs` 文件头并加注释 |
| count_tokens 改 async 后 handler 路径阻塞行为变化 | 高并发抖动 | 内部仍是纯 CPU 计算，未来需要时切 `spawn_blocking`；本次保持 async fn 直返 |
| 凭据回写格式偏离（字段顺序、`priority:0` 出现） | 用户配置文件 diff 噪音 | `serde_json::to_string_pretty` 字段顺序按 struct 定义；保留 `skip_serializing_if = is_zero` |
| Admin UI 资源路径相对引用偏移 | 前端 404 | `interface/http/ui.rs` 保留 `#[folder = "admin-ui/dist"]` 常量，逻辑零改动；Step 6.5 + 8.1 验证 |
| EventReducer 边界 case（thinking 在 chunk 边界、tool_use stop 标志） | 流式响应错乱 | Step 4.5 + 4.7 测试覆盖；Step 8.1 用真实凭据手测一轮 |
| 重构期间 master 出现 hotfix | 合入冲突 | 分支隔离，确认 master 无并发修改；如必要 cherry-pick |
| 静态分析覆盖率下降 | 隐藏 bug | Step 8.4 最终关卡强制 `cargo clippy --all-targets -- -D warnings` 通过 |
| 依赖中 `thiserror` v2 不兼容 | 编译失败 | Step 0.2 先行验证；如有问题降回 v1 |
| selector 借用 store/state/stats 引用跨 .await | UB / 死锁 | trait 签名约束 `select` 同步纯计算；pool 在持锁期内组装 view 调 select，select 返回 id 后再放锁，禁止跨 .await |
| 凭据加载时 ID 顺序变化导致 state/stats 错位 | 选错凭据状态 | store / state / stats 一律按 `HashMap<u64, _>` 而非 `Vec` 索引存储，pool 组装 view 时按 id join |

## Rollback Strategy

`refactor/v2` 是从 master 切出的新分支，未推送主干。若任何阶段验证失败：

1. **Phase 1-2 失败**（基础层不通）：直接 `git reset --hard master`，重启计划
2. **Phase 3-4 失败**（领域 / 服务层）：保留 Phase 1-2 commit，从最近一次绿色 commit 重做后续 Phase
3. **Phase 5-7 失败**（接口 / 装配）：同上
4. **Phase 8 验证不通**（最严重，含手工冒烟语义不一致 / 负载行为偏移 / 最终 `clippy -D warnings` 报错）：保留分支，针对失败用例修复后重跑；不允许把不兼容的代码合入 master

每个 Phase 完成后 `git commit` 一次（或多次，按 Step 颗粒度），保证 rollback 颗粒度。

## Status

- [x] Plan approved
- [x] Implementation started
- [x] Phase 0 complete (准备)
- [x] Phase 1 complete (基建)
- [x] Phase 2 complete (基础设施 + 拆 token_manager)
- [ ] Phase 3 complete (KiroClient)
- [ ] Phase 4 complete (Conversation 协议)
- [ ] Phase 5 complete (interface/http – Anthropic)
- [ ] Phase 6 complete (interface/http – Admin)
- [ ] Phase 7 complete (装配 + 删除)
- [ ] Phase 8 complete (手工冒烟 + 负载验证 + 最终静态检查)
- [ ] Implementation complete

## 阶段并行性提示

| 阶段 | 串行性 | 备注 |
| - | - | - |
| Phase 0 | 必须先于全部 | 准备 fixture |
| Phase 1 | 串行 | 基建必须先 |
| Phase 2 | 串行 | 拆 token_manager |
| Phase 3 | 串行（依赖 Phase 2） | KiroClient |
| Phase 4 | 串行（依赖 Phase 1 + 3） | Conversation |
| **Phase 5 与 Phase 6** | *可并行* | 都依赖 Phase 4，但互不依赖。两个 interface 子模块独立 |
| Phase 7 | 串行（依赖 5/6） | 装配 |
| Phase 8 | 串行（依赖 7） | 手工冒烟 + 负载验证 + 最终 `clippy -D warnings` |

> 单人执行无并行收益；多人协作（如分两个 worktree）可让 Phase 5 / 6 同时推进。
