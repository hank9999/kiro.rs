# Admin System Prompt Design

## 目标

在 Kiro Admin 管理界面中增加系统提示词管理能力。管理员可以开启或关闭自定义系统提示词，选择覆盖原始提示词或追加到原始提示词后面，并维护简单的字符串替换规则。

## 范围

- 后端配置新增 `systemPrompt` 配置块。
- Admin API 提供读取和保存系统提示词配置的接口。
- Anthropic 请求转换前根据配置合成最终 system prompt。
- Admin UI 设置页增加系统提示词区域。

## 配置模型

配置文件新增 camelCase 字段：

```json
{
  "systemPrompt": {
    "enabled": false,
    "mode": "append",
    "content": "",
    "replacements": [
      { "old": "Old text", "new": "New text" }
    ]
  }
}
```

`enabled` 默认为 `false`，避免升级后自动改变现有请求行为。`mode` 支持 `append` 和 `overwrite`，默认 `append`。`content` 是管理员直接编辑的提示词正文。`replacements` 是普通字符串全量替换规则。

## 请求行为

未启用或 `content` 为空时保持现有行为。启用且 `mode` 为 `append` 时，先保留客户端原始 system prompt，再追加管理员配置的提示词。启用且 `mode` 为 `overwrite` 时，仅使用管理员配置的提示词。替换规则作用于合成后的最终 system prompt。

转换器仍负责追加现有的分块写入策略和 thinking 前缀，确保旧功能不退化。

## Admin API

- `GET /api/admin/config/system-prompt` 返回当前配置。
- `PUT /api/admin/config/system-prompt` 保存配置并写回 `config.json`。

保存时会裁剪 mode 之外的非法值，拒绝空 key 的 replacement，并将内容保存在 `config.json`。本次不引入单独文件路径，减少管理界面的心智负担。

## Admin UI

设置页新增“系统提示词”卡片：

- 启用开关。
- 模式选择按钮：追加、覆盖。
- 大文本框编辑提示词正文。
- 替换规则列表，可添加和删除。
- 保存按钮和错误提示沿用现有 toast 模式。

界面沿用当前后台风格，不做视觉大改。

## 测试

后端单元测试覆盖默认禁用、append、overwrite、replacement。Admin 服务测试覆盖配置读写持久化。前端通过类型检查和构建验证。
