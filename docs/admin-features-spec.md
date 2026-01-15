# Admin 功能增强需求规格

**来源**: https://github.com/hank9999/kiro.rs/issues/35  
**日期**: 2026-01-15  
**状态**: 待实现

---

## 概述

在 `/admin` 管理界面增加以下功能：
1. 官方 token.json 批量导入
2. 凭证检测（按模型批量验证）
3. 失效凭证一键删除
4. 凭证轮训（round-robin）

---

## PR-1: 官方 token.json 批量导入

### 功能描述
支持以官方 token.json 文件格式进行批量导入凭证。

### 导入规则
- 按 `provider` 字段判断认证方式：
  - `BuilderId` → `authMethod = "builder-id"`
  - `IdC` → `authMethod = "idc"`
  - `Social` / 空 → `authMethod = "social"`
  - 其它 provider → 记为 invalid
- 仅提取"新增凭据"表单字段：
  - `refreshToken` (必填)
  - `authMethod` (按 provider 映射)
  - `clientId` (IdC/BuilderId 需要)
  - `clientSecret` (IdC/BuilderId 需要)
  - `priority` (可选，默认 0)
- 忽略字段：`region`, `machineId`, `accessToken` 等

### token.json 样例结构
```json
{
  "provider": "BuilderId",
  "refreshToken": "aorAAAAA...",
  "clientId": "jWC-Ac9XZsJI6NKKyfY4jHVzLWVhc3QtMQ",
  "clientSecret": "eyJraWQi...",
  "authMethod": "IdC"
}
```

### API 设计
```
POST /api/admin/credentials/import-token-json
Request:
{
  "dryRun": boolean,
  "items": TokenJson | TokenJson[]
}

Response:
{
  "summary": {
    "parsed": number,
    "added": number,
    "skipped": number,
    "invalid": number
  },
  "items": [{
    "index": number,
    "fingerprint": string,
    "action": "added" | "skipped" | "invalid",
    "reason"?: string
  }]
}
```

### 成功标准
- [ ] `dryRun=true` 不写盘
- [ ] `dryRun=false` 写盘结果与 dry-run 报告一致
- [ ] 响应/日志不包含 token 明文（只返回 fingerprint）
- [ ] adminApiKey 鉴权正常

### UI 交互
- Dashboard 增加"导入 token.json"按钮
- 弹窗：粘贴 JSON / 上传文件
- 预览：dry-run 结果展示
- 确认：二次确认后执行导入

---

## PR-2: 凭证检测（按模型批量验证）

### 功能描述
支持依据所选模型，对勾选的凭证展开批量验证。

### 检测方式
- 对所选 model 发起最小 probe 请求（真实 API 调用）
- 不仅仅是 refresh token 成功

### API 设计
```
POST /api/admin/credentials/validate
Request:
{
  "credentialIds": number[],
  "model": string,
  "timeoutMs"?: number,      // 默认 10000
  "maxConcurrency"?: number  // 默认 3
}

Response:
{
  "results": [{
    "credentialId": number,
    "model": string,
    "status": "ok" | "denied" | "invalid" | "transient",
    "detail"?: string,
    "latencyMs": number
  }]
}
```

### 状态分类
- `ok`: 验证通过
- `denied`: 401/403，凭证无权限访问该模型
- `invalid`: 凭证结构无效或 refresh 失败
- `transient`: 429/5xx/网络错误（不影响凭证状态）

### 成功标准
- [ ] 返回每个 credential 的结果
- [ ] 并发/超时可控
- [ ] transient 错误不禁用凭证、不影响失败计数
- [ ] 不依赖外网测试（本地 mock server）

### UI 交互
- Dashboard 凭证列表增加勾选框
- 模型下拉选择（sonnet/opus/haiku）
- "批量检测"按钮
- 结果展示（状态标签）

---

## PR-3: 失效凭证一键删除

### 功能描述
实现对所有出现错误的凭证进行一键删除功能。

### 失效判定
- `disabled == true` 且 `disabled_reason != Manual`
- 即：自动禁用的凭证（TooManyFailures / QuotaExceeded）
- 手动禁用的凭证不会被删除

### API 设计
```
POST /api/admin/credentials/bulk-delete-invalid
Request:
{
  "dryRun": boolean
}

Response:
{
  "matched": number,
  "deleted": number,
  "ids": number[]
}
```

### 成功标准
- [ ] dry-run 列表与实际删除一致
- [ ] 手动禁用的凭证不会被删除
- [ ] UI 必须二次确认

### UI 交互
- Dashboard 增加"一键删除失效"按钮
- 点击后先 dry-run 预览
- 展示将删除的凭证列表
- 二次确认后执行删除

---

## PR-4: 凭证轮训（Round-Robin）

### 功能描述
提供凭证轮训功能，管理端所有未禁用的凭证进行轮流调用。

### 行为定义
- 开启轮训后：所有未禁用凭证轮流调用
- 优先级不再作为选择顺序（仅作为展示字段）
- 默认保持当前 priority 模式

### 配置
```json
// config.json
{
  "credentialRotation": "priority" | "roundRobin"
}
```

### API 设计
```
GET /api/admin/settings
Response:
{
  "credentialRotation": "priority" | "roundRobin"
}

POST /api/admin/settings
Request:
{
  "credentialRotation": "priority" | "roundRobin"
}
Response:
{
  "success": true,
  "message": "设置已更新并立即生效"
}
```

### 生效方式
- **立即生效**：运行期更新 `MultiTokenManager`，并同步写回 config.json
- 无需重启

### 成功标准
- [ ] 默认行为不变（priority 模式）
- [ ] 轮训开启后请求分布可观测
- [ ] 线程安全（并发下游标正确）
- [ ] /admin 开关可切换并写回配置

### UI 交互
- Dashboard 增加"凭证选择策略"设置区域
- 下拉选择：优先级模式 / 轮训模式
- 切换后立即生效

---

## 实现顺序

1. **PR-1**: 官方 token.json 批量导入
2. **PR-2**: 凭证检测（按模型批量验证）
3. **PR-3**: 失效凭证一键删除
4. **PR-4**: 凭证轮训

每个 PR 为独立分支，包含完整的后端 + 前端实现。

---

## 技术约束

- 后端：Rust (Axum)，遵循现有 `src/admin/*` 模式
- 前端：React + TypeScript + Tailwind，遵循现有 `admin-ui/src/*` 模式
- 所有新 endpoint 必须经过 adminApiKey 鉴权
- 响应/日志不得包含 token 明文
- 使用 camelCase JSON 字段命名
