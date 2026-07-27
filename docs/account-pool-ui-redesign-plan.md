# 账号池管理界面重设计开发方案

## 目标

把当前 Admin UI 从“单凭据卡片管理”重设计为“多账号池运营面板”。新界面需要适合几十到上百个账号的扫描、筛选、批量操作、状态判断和单账号排障，避免截图中已经出现的文字挤压、按钮拥挤、容器超出、长邮箱破坏布局等问题。

本方案只规划开发，不直接修改业务代码。

## 已阅读和关联代码

前端主路径：

- `admin-ui/src/components/dashboard.tsx`
- `admin-ui/src/components/credential-card.tsx`
- `admin-ui/src/components/add-credential-dialog.tsx`
- `admin-ui/src/components/batch-import-dialog.tsx`
- `admin-ui/src/components/kam-import-dialog.tsx`
- `admin-ui/src/components/batch-verify-dialog.tsx`
- `admin-ui/src/components/balance-dialog.tsx`
- `admin-ui/src/hooks/use-credentials.ts`
- `admin-ui/src/api/credentials.ts`
- `admin-ui/src/types/api.ts`
- `admin-ui/src/index.css`
- `admin-ui/package.json`

后端和运行语义：

- `src/admin/types.rs`
- `src/admin/handlers.rs`
- `src/admin/service.rs`
- `src/admin/router.rs`
- `src/kiro/token_manager.rs`
- `src/kiro/model/credentials.rs`
- `src/model/config.rs`
- `src/admin_ui/router.rs`
- `README.md` 中 Admin、多凭据、Region、负载均衡说明

关键结论：

- 账号池列表数据来自 `GET /api/admin/credentials`，字段已经足够支撑新版主列表。
- 余额/订阅信息来自 `GET /api/admin/credentials/:id/balance`，当前是按需查询，不应默认对所有账号自动查询，避免上游压力。
- 删除凭据的业务前置条件是“必须已禁用”，新 UI 必须继续遵守。
- `api_key` 凭据不支持刷新 Token，单卡片已禁用此按钮，但当前批量刷新只过滤启用状态，没有过滤 `api_key`，新版批量动作需要修正交互规则。
- `disabledReason` 可能是 `Manual`、`TooManyFailures`、`TooManyRefreshFailures`、`QuotaExceeded`、`InvalidRefreshToken`、`InvalidConfig`，新 UI 应用它建立健康状态，而不是只显示“已禁用”。
- `currentId` 在 `priority` 模式下更像当前优先使用账号，在 `balanced` 模式下会随请求重新选择。界面文案应避免误导，可叫“当前凭据”并在说明/tooltip 中解释为“管理器最近选中的凭据”。

## 当前界面问题

### 信息架构问题

当前 `Dashboard` 直接承载：

- 页面导航
- 负载均衡切换
- 统计卡片
- 批量选择状态
- 所有批量操作
- 查询信息操作
- 导入入口
- 分页
- 余额缓存
- 验活进度
- 刷新进度

这导致主组件过大，操作和展示耦合。任何新增筛选、排序、详情抽屉、更多菜单都会继续堆在 `Dashboard` 里。

### 主视图不适合号池

`CredentialCard` 把一个账号的信息和动作全部放进卡片：

- 邮箱、状态、禁用原因、认证方式、端点都挤在标题行。
- 优先级、失败次数、刷新失败、订阅等级、成功次数、最后调用、剩余用量、代理、Profile ARN 都放在一个小网格。
- 重置失败、刷新 Token、提高/降低优先级、查看余额、删除全部常驻展示。

这适合 1 到 6 个账号，不适合号池。一旦有 30 个账号，用户无法快速判断哪些可用、哪些异常、哪个优先级高、哪些需要处理。

### 布局挤压和溢出风险

截图中的“凭据管理”和“已选择 1 个”已经被压成竖排，根因是：

- 标题区使用 `flex items-center justify-between`，左右都不允许合理换行。
- 右侧按钮数量过多，未分组，也没有 `flex-wrap` 和响应式降级策略。
- 卡片标题里的邮箱没有 `min-w-0`、`truncate`、`max-w`。
- 标签和开关直接和邮箱抢横向空间。
- 主卡片使用 `grid md:grid-cols-2 lg:grid-cols-3`，卡片最小宽度没有被业务内容约束。

### 操作层级不清

高频操作、低频操作、危险操作混在一起：

- `添加凭据` 应是主操作。
- `批量导入`、`KAM 导入` 应归类为导入操作。
- `批量删除`、`清除已禁用` 是危险操作，不应常驻占据主工具栏。
- `查询信息` 是对当前可见账号的批处理，应和列表上下文绑定。
- 单账号的“提高/降低优先级”不应常驻两个大按钮，更适合行内数字编辑或更多菜单。

## 新设计原则

1. 主列表用于扫描，不承载全部详情。
2. 单账号详情进入右侧抽屉，避免卡片无限膨胀。
3. 批量操作只在选中账号后出现。
4. 长文本必须截断，完整值通过 tooltip、title 或复制动作访问。
5. 状态标签独立成列，不和邮箱抢空间。
6. 危险操作默认收进更多菜单，并保留确认。
7. 余额/订阅保持按需查询，不自动扫全池。
8. 移动端使用紧凑账号卡，桌面端使用表格/列表。
9. 不新增后端接口作为第一阶段前置条件，优先基于现有 API 完成重设计。
10. 代码拆分后每个组件只负责一个层级：数据派生、工具栏、批量栏、账号列表、详情抽屉、弹窗。

## 新页面信息架构

### 桌面端布局

页面从上到下：

1. 顶部导航：`Kiro Admin`、负载均衡模式、刷新、主题、退出。
2. 账号池指标条：总数、可用、异常、已禁用、当前凭据、已查询余额数。
3. 工具栏：搜索、状态筛选、认证方式筛选、端点筛选、排序、导入菜单、添加账号。
4. 批量操作栏：仅在选中账号后出现，sticky 在列表上方。
5. 账号池表格：高密度展示账号行。
6. 右侧详情抽屉：点击账号行或“详情”后打开。
7. 现有弹窗：添加、批量导入、KAM 导入、批量验活、余额查询继续保留，但入口重新整理。

### 移动端布局

移动端不强行塞表格：

- 指标条变为 2 列。
- 工具栏分两行，筛选进入下拉。
- 主列表变成紧凑账号卡，每张只显示账号、状态、优先级、余额状态、最近调用和更多菜单。
- 详情使用全屏或底部/右侧抽屉。

## 账号池主列表设计

### 推荐列

桌面端表格列：

| 列 | 内容 | 规则 |
| --- | --- | --- |
| 选择 | Checkbox | 支持单选、当前页全选 |
| 账号 | 邮箱/凭据 ID/API Key 脱敏值 | 一行截断，副行显示 `#id`、Profile ARN、代理、hash 片段等 |
| 状态 | 健康状态标签 | 独立列，避免挤压账号文本 |
| 认证/端点 | `Social`、`IdC`、`API Key`、`ide` 等 | 小标签，可截断 |
| 优先级 | 数字 + 快捷编辑 | 数字越小越高，支持行内编辑 |
| 调用 | 成功/失败/刷新失败 | 失败突出显示 |
| 余额 | 剩余额度、订阅、未查询 | 未查询不渲染假数据 |
| 最近调用 | 相对时间 | `从未使用`、`2 小时前` |
| 启用 | Switch | 与删除规则联动 |
| 操作 | 详情、更多 | 更多菜单承载低频动作 |

账号列必须使用：

- `min-w-0`
- `truncate`
- `max-w`
- `title` 或 tooltip 展示完整邮箱
- 副行使用更小字号，不影响主行高度

表格容器规则：

- 桌面端允许内部横向滚动，但不能撑爆页面。
- 常用列固定最小宽度，账号列作为弹性列。
- 行高固定在约 56 到 64px，避免状态变化造成跳动。
- 选中行用浅色背景，不用粗黑边框。

### 状态模型

基于 `CredentialStatusItem` 派生 `CredentialHealth`，前端统一使用，不在各组件重复判断。

建议状态优先级：

1. `disabled && disabledReason === "InvalidConfig"`：配置无效
2. `disabled && disabledReason === "InvalidRefreshToken"`：Token 失效
3. `disabled && disabledReason === "QuotaExceeded"`：额度耗尽
4. `disabled && disabledReason === "TooManyRefreshFailures"`：刷新异常
5. `disabled && disabledReason === "TooManyFailures"`：调用异常
6. `disabled && disabledReason === "Manual"` 或无原因：已禁用
7. `!disabled && refreshFailureCount > 0`：刷新不稳
8. `!disabled && failureCount > 0`：调用不稳
9. `!disabled && isCurrent`：当前凭据
10. `!disabled`：可用

颜色建议：

- 可用：绿色
- 当前凭据：蓝色
- 调用不稳/刷新不稳：黄色或橙色
- Token 失效/配置无效/额度耗尽：红色
- 手动禁用：灰色

状态文案必须短，例如：

- 可用
- 当前
- 调用异常
- 刷新异常
- Token 失效
- 额度耗尽
- 配置无效
- 已禁用

详情抽屉里再展示完整解释。

## 工具栏设计

### 默认工具栏

左侧：

- 搜索框：搜索邮箱、`#id`、endpoint、认证方式、masked API Key。
- 状态筛选：全部、可用、当前、异常、已禁用。
- 认证筛选：全部、Social、IdC、API Key。
- 端点筛选：从列表数据中动态收集。

右侧：

- 查询当前页
- 导入菜单
  - 批量 JSON 导入
  - Kiro Account Manager 导入
- 添加凭据

负载均衡模式留在顶部导航，但样式改成更短的 segmented control：

- 优先级
- 均衡

### 批量操作栏

选中账号后，在工具栏下方出现：

`已选择 N 个账号 | 验活 | 刷新 Token | 恢复异常 | 禁用 | 启用 | 删除已禁用 | 取消选择`

规则：

- 批量刷新 Token：只作用于启用且非 `api_key` 的账号，其他账号显示“将跳过 X 个”。
- 批量删除：只删除已禁用账号，保持后端规则一致。
- 批量恢复异常：只对 `failureCount > 0` 或 `refreshFailureCount > 0` 或异常禁用账号有效；`InvalidConfig` 不能恢复，提示需要修正配置。
- 批量验活：默认只验活启用账号；若选择中包含禁用账号，跳过并在结果中显示。
- 批量启用/禁用：现有接口支持逐个设置，前端顺序执行并显示进度。
- 所有批量动作必须支持进行中状态，避免重复点击。

## 单账号详情抽屉

点击行或“详情”打开右侧抽屉。

分组：

1. 身份信息
   - 邮箱
   - 凭据 ID
   - 认证方式
   - 端点
   - API Key 脱敏展示
   - Profile ARN 是否存在
2. 调度信息
   - 当前状态
   - 启用状态
   - 优先级
   - 负载均衡模式说明
3. 健康统计
   - 成功次数
   - 连续失败次数
   - Token 刷新失败次数
   - 最后调用时间
   - Token 过期时间
4. 余额/订阅
   - 未查询时显示“未查询”
   - 查询中显示进度
   - 查询成功显示订阅、剩余额度、使用百分比、重置时间
   - 查询失败显示可读错误
5. 操作
   - 查询余额
   - 刷新 Token
   - 重置失败
   - 编辑优先级
   - 启用/禁用
   - 删除

抽屉实现建议：

- 新增 `admin-ui/src/components/ui/sheet.tsx`，基于已有 `@radix-ui/react-dialog` 封装。
- 不需要新增依赖。
- 抽屉宽度桌面端约 `420px` 到 `520px`，移动端全屏。

## 组件拆分方案

建议新增或重构：

- `admin-ui/src/components/account-pool/credential-dashboard.tsx`
  - 页面编排，不承载大量业务细节。
- `admin-ui/src/components/account-pool/pool-metrics.tsx`
  - 顶部指标条。
- `admin-ui/src/components/account-pool/pool-toolbar.tsx`
  - 搜索、筛选、排序、导入、添加。
- `admin-ui/src/components/account-pool/batch-action-bar.tsx`
  - 选中后的批量动作。
- `admin-ui/src/components/account-pool/credential-pool-table.tsx`
  - 桌面端账号表格。
- `admin-ui/src/components/account-pool/credential-compact-card.tsx`
  - 移动端紧凑卡片。
- `admin-ui/src/components/account-pool/credential-detail-sheet.tsx`
  - 详情抽屉。
- `admin-ui/src/components/account-pool/credential-row-actions.tsx`
  - 单行更多菜单。
- `admin-ui/src/components/account-pool/status-badge.tsx`
  - 统一状态展示。
- `admin-ui/src/components/account-pool/priority-editor.tsx`
  - 行内优先级编辑。
- `admin-ui/src/lib/credential-status.ts`
  - 派生健康状态、筛选状态、状态颜色。
- `admin-ui/src/lib/credential-format.ts`
  - 时间、余额、账号展示名、认证方式格式化。
- `admin-ui/src/components/ui/dropdown-menu.tsx`
  - 基于 `@radix-ui/react-dropdown-menu`。
- `admin-ui/src/components/ui/tooltip.tsx`
  - 基于 `@radix-ui/react-tooltip`。
- `admin-ui/src/components/ui/sheet.tsx`
  - 基于 `@radix-ui/react-dialog`。

旧组件处理：

- `CredentialCard` 不再作为桌面主视图使用。
- 可以先保留，直到移动端 `CredentialCompactCard` 完成后再删除或停止引用。
- `BalanceDialog` 可先保留作为兼容入口，后续余额信息主入口迁移到详情抽屉。

## 状态和数据流设计

### Dashboard 层状态

保留：

- 登录/退出由 `App` 保持。
- `useCredentials()` 继续 30 秒轮询。
- `balanceMap` 和 `loadingBalanceIds` 保持按需缓存。
- 批量验活对话框继续复用。
- 导入弹窗继续复用。

新增：

- `searchQuery`
- `statusFilter`
- `authMethodFilter`
- `endpointFilter`
- `sortKey`
- `sortDirection`
- `pageSize`
- `activeCredentialId`
- `detailSheetOpen`

派生顺序：

1. 原始 `data.credentials`
2. 生成 `credentialViewModels`
3. 搜索过滤
4. 状态/认证/端点过滤
5. 排序
6. 分页
7. 渲染当前页

注意：

- 分页必须基于过滤后的结果。
- 搜索/筛选变化时回到第 1 页。
- 凭据删除后要清理 `selectedIds`、`activeCredentialId`、`balanceMap` 中不存在的 ID。
- 选中状态允许跨页保留，但批量栏要明确“已选择 N 个”，并提供取消选择。

### 余额查询

不默认全量查询余额。

提供三个入口：

- 查询当前页
- 查询选中项
- 单账号详情里查询

查询策略：

- 默认串行或低并发，保持当前“避免瞬时并发”的思路。
- 结果写入 `balanceMap`。
- 每个账号显示独立 loading。
- 查询失败不清空旧余额，但需要记录本次错误，避免用户误以为是最新成功值。可新增 `balanceErrorMap`。

### 批量动作

建议抽出通用执行器：

`runBatchAction(ids, options)`

能力：

- 过滤可操作项
- 统计跳过原因
- 串行执行
- 进度回调
- 成功/失败汇总
- 最后统一 invalidate `credentials`

这样避免 `Dashboard` 里继续复制多个 `for` 循环。

## 视觉规范

整体风格：

- 操作型后台，不做营销页。
- 背景浅灰，内容白底。
- 表格/列表边框克制，不使用大面积粗黑描边。
- 卡片圆角不超过 8px。
- 不使用大面积单一蓝紫/深蓝色调。
- 主按钮深色，危险操作红色但不常驻暴露。
- 使用 lucide 图标做按钮视觉辅助。

尺寸规则：

- 顶部导航高度约 56px。
- 指标条高度约 72px，不再使用很高的统计卡。
- 表格行高约 56 到 64px。
- 详情抽屉宽度桌面端 420 到 520px。
- 工具栏按钮高度统一 36px。

文本规则：

- 不使用 viewport width 缩放字体。
- 不使用负 letter spacing。
- 长邮箱、endpoint、proxyUrl、错误信息都必须截断。
- 账号列主文本单行，副文本单行。
- 错误详情可以在详情抽屉或 tooltip 中换行展示。
- 表格单元格默认 `whitespace-nowrap`，可换行的只允许详情区域。

## 响应式规则

断点建议：

- `< 768px`：紧凑卡片模式。
- `768px - 1279px`：表格隐藏部分次要列，例如成功次数、端点、副标签。
- `>= 1280px`：完整表格。

窄屏隐藏优先级：

1. 代理/Profile ARN 标签
2. 成功次数
3. 端点
4. 订阅等级
5. 余额详情

不能隐藏：

- 账号名
- 状态
- 启用状态
- 基本操作入口

## 业务一致性要求

必须保持：

- 删除前必须禁用。
- API Key 凭据不能刷新 Token。
- `InvalidConfig` 不能通过“恢复异常”直接启用。
- 余额查询仍然按需触发。
- 批量导入失败回滚逻辑不改变。
- KAM 导入兼容旧格式和 1.8.3+ 平铺格式。
- `priority` 数字越小优先级越高。
- `balanced` 模式按成功次数较少优先，前端文案不能说成随机轮询。
- `currentId === 0` 时显示 `-`，不能显示 `#0` 为有效账号。

## 开发阶段清单

每个阶段完成后都要自检对应代码、边界和显示结果。不要等全部做完才统一检查。

### 阶段 0：基线确认

- [ ] 运行 `pnpm build` 确认当前前端基线。
- [ ] 记录当前截图问题对应组件和 CSS 根因。
- [ ] 确认没有未提交的用户改动被误覆盖。
- [ ] 确认当前接口返回字段和 `admin-ui/src/types/api.ts` 一致。

自检：

- [ ] 没有改业务代码。
- [ ] 没有产生乱码。
- [ ] 已确认 `api_key`、`idc`、`social` 三类账号展示需求。
- [ ] 已确认 `disabledReason` 的所有枚举映射。

### 阶段 1：补齐 UI 基础组件和工具函数

- [ ] 新增 `dropdown-menu` 封装。
- [ ] 新增 `tooltip` 封装。
- [ ] 新增 `sheet` 封装。
- [ ] 新增账号状态派生工具 `credential-status.ts`。
- [ ] 新增格式化工具 `credential-format.ts`。

自检：

- [ ] 不新增不必要依赖，优先使用 package 中已有 Radix 依赖。
- [ ] 所有中文文案为 UTF-8。
- [ ] 状态映射覆盖所有 `disabledReason`。
- [ ] `api_key` 刷新限制在工具层可被复用判断。
- [ ] 格式化函数处理 `null`、空字符串、非法日期。

### 阶段 2：重构 Dashboard 数据派生

- [ ] 把搜索、筛选、排序、分页从 JSX 中拆出清晰派生逻辑。
- [ ] 新增 `pageSize`。
- [ ] 搜索/筛选变化时回到第 1 页。
- [ ] 数据变化时清理失效选中项、余额缓存和详情选中项。
- [ ] 派生指标条数据。

自检：

- [ ] 过滤后分页数量正确。
- [ ] 当前页为空但总数据不为空时能自动回退到有效页。
- [ ] 选中跨页时不会误删或误操作不可见账号。
- [ ] `currentId` 为 0 或对应账号不存在时显示安全。

### 阶段 3：实现桌面端账号池表格

- [ ] 新增 `CredentialPoolTable`。
- [ ] 实现固定列宽和账号列截断。
- [ ] 状态独立列展示。
- [ ] 行 hover、选中、当前凭据样式。
- [ ] 行级启用 Switch。
- [ ] 行级更多菜单。

自检：

- [ ] 长邮箱不会挤压标签和开关。
- [ ] 长 endpoint、proxyUrl、错误原因不会撑爆容器。
- [ ] 表格内部可滚动但页面不横向溢出。
- [ ] 30、100 个账号时扫描仍清楚。
- [ ] 深色模式可读。

### 阶段 4：实现工具栏和批量操作栏

- [ ] 新增 `PoolToolbar`。
- [ ] 新增 `BatchActionBar`。
- [ ] 导入操作收进菜单。
- [ ] 批量操作只在选中后出现。
- [ ] 批量刷新跳过禁用和 API Key。
- [ ] 批量删除只对已禁用账号生效。
- [ ] 批量恢复跳过 `InvalidConfig`。
- [ ] 选中项中不可操作的账号显示跳过计数。

自检：

- [ ] 工具栏在 1024px、1366px、1920px 都不挤压。
- [ ] 窄屏按钮能换行或折叠到菜单。
- [ ] 危险动作不常驻主工具栏。
- [ ] 批量执行中按钮禁用，不能重复提交。
- [ ] toast 文案准确汇总成功、失败、跳过。

### 阶段 5：实现详情抽屉

- [ ] 新增 `CredentialDetailSheet`。
- [ ] 分组展示身份、调度、健康、余额、操作。
- [ ] 支持从表格行打开。
- [ ] 支持查询余额并写入 `balanceMap`。
- [ ] 支持编辑优先级。
- [ ] 支持启用/禁用。
- [ ] 支持删除确认。

自检：

- [ ] 抽屉中文和长错误信息不溢出。
- [ ] 移动端全屏或接近全屏可用。
- [ ] 详情中操作和行操作状态一致。
- [ ] 删除当前打开账号后抽屉关闭或切换到安全空态。

### 阶段 6：移动端紧凑卡片

- [ ] 新增 `CredentialCompactCard`。
- [ ] 只展示账号、状态、优先级、余额摘要、最近调用和更多菜单。
- [ ] 保留选择能力。
- [ ] 详情进入抽屉。

自检：

- [ ] 375px 宽度不出现横向滚动。
- [ ] 长邮箱截断。
- [ ] 批量栏不遮挡列表内容。
- [ ] 操作按钮可点击区域足够。

### 阶段 7：统一弹窗和进度体验

- [ ] 保持新增凭据弹窗字段完整。
- [ ] 保持批量 JSON 导入逻辑完整。
- [ ] 保持 KAM 导入兼容逻辑完整。
- [ ] 批量验活结果可从批量栏打开。
- [ ] 查询当前页和查询选中项使用统一进度文案。

自检：

- [ ] 导入失败回滚仍然先禁用再删除。
- [ ] KAM error 状态跳过逻辑仍然可用。
- [ ] 验活过程可后台运行。
- [ ] 进度条分母不为 0。

### 阶段 8：验证和修正

- [ ] `pnpm build`
- [ ] 浏览器检查桌面视口：1920x1080、1366x768、1024x768。
- [ ] 浏览器检查移动视口：390x844、375x667。
- [ ] 准备 mock 或临时测试数据覆盖：1 个、12 个、30 个、100 个账号。
- [ ] 检查长邮箱、长 endpoint、长 proxyUrl、长 disabledReason。
- [ ] 检查暗色模式。
- [ ] 检查无账号空态。
- [ ] 检查全部账号禁用。
- [ ] 检查全部账号异常。
- [ ] 检查仅 API Key 账号。
- [ ] 检查 `currentId` 不存在或为 0。

自检：

- [ ] 无 TypeScript 错误。
- [ ] 无 React key 警告。
- [ ] 无控制台运行时错误。
- [ ] 无页面横向溢出。
- [ ] 无文字被挤成竖排。
- [ ] 无乱码。
- [ ] 删除、禁用、恢复、刷新、查询余额都符合后端规则。

## 验收标准

功能验收：

- 账号数量多时，主界面仍能快速扫描状态。
- 搜索、筛选、排序、分页可组合使用。
- 批量操作不会误操作不可操作账号。
- 单账号详情完整，且不挤占主列表。
- 余额查询按需触发，查询结果能缓存到当前页面状态。
- 导入、验活、删除、刷新、启用/禁用等现有能力不丢失。

视觉验收：

- 桌面端 1366px 宽度下工具栏不压缩标题成竖排。
- 长邮箱不会撑破卡片或表格。
- 按钮不会溢出容器。
- 表格或列表在暗色模式可读。
- 空态、加载态、错误态完整。

代码验收：

- 主组件不再继续无限膨胀。
- 状态派生逻辑集中，避免多个组件重复判断。
- 业务规则集中复用，特别是删除、刷新、恢复异常的可操作性。
- 新增 UI primitives 与现有 `button`、`badge`、`dialog` 风格一致。
- 不引入无关重构。

## 方案自检

### 是否符合要求

符合。方案围绕“多个账号、号池、避免挤压和超出、美观、交互好”展开，主方案从卡片改为表格/列表和详情抽屉，更适合多账号。

### 是否完善

基本完善。方案覆盖：

- 信息架构
- 状态模型
- 工具栏
- 批量操作
- 单账号详情
- 响应式
- 组件拆分
- 数据流
- 业务一致性
- 阶段清单
- 阶段自检

### 是否完整

完整度足够进入开发。现有所有关键入口都被纳入：

- 添加凭据
- 批量导入
- KAM 导入
- 批量验活
- 查询余额
- 刷新 Token
- 重置失败
- 修改优先级
- 启用/禁用
- 删除
- 负载均衡模式

### 是否正确

与现有后端逻辑一致：

- 删除必须禁用。
- API Key 不刷新 Token。
- `disabledReason` 被完整映射。
- `priority` 规则未反转。
- 余额查询不默认扫全量。
- `balanced` 模式没有被误描述为随机。

### 是否规范一致

与当前技术栈一致：

- React 18
- TypeScript
- Tailwind
- Radix primitives
- lucide-react
- React Query
- 现有 shadcn 风格组件

不要求替换框架，不引入重 UI 库。

### 是否存在方案自身缺陷

有几个需要开发时注意的点：

- 表格方案对上百账号足够，但如果未来达到几千账号，仅前端分页和过滤会变重，届时需要后端分页/搜索接口。
- 余额列依赖按需查询，未查询时信息不完整，这是为了避免批量打上游；不能为了视觉完整而默认全量查询。
- 详情抽屉会新增 UI primitive，虽然无新依赖，但需要保证和现有 `Dialog` 不产生焦点管理冲突。
- `CredentialCard` 的去留要分阶段处理，不能一开始删除，避免移动端或旧逻辑遗漏。
- 如果同时做大量组件拆分和视觉重构，回归面较大，必须按阶段开发和自检。

### 是否有上下设计冲突

已检查主要冲突：

- 桌面表格和移动卡片不冲突，因为是响应式两套展示，同一业务状态模型复用。
- 批量删除入口减少不冲突，因为删除规则仍保留，并在批量栏和更多菜单中提供。
- 余额从弹窗迁移到抽屉不冲突，第一阶段可保留 `BalanceDialog` 兼容。
- 当前凭据文案需要谨慎，避免和 `balanced` 模式每次选择的语义冲突。
- 低频动作进入更多菜单不影响功能完整性，反而减少主界面拥挤。

## 建议开发顺序

优先顺序：

1. 状态模型和工具函数
2. 表格主视图
3. 工具栏和批量栏
4. 详情抽屉
5. 移动端紧凑卡
6. 弹窗和进度体验统一
7. 全视口验证

不要先做视觉微调。当前问题的根因是信息架构不适合号池，必须先换主承载方式。
