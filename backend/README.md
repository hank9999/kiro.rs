# Backend - Kiro API 代理服务

Rust 后端服务，将 Anthropic API 请求转换为 Kiro API 请求。

## 目录结构

| 文件/目录 | 地位 | 功能 |
|----------|------|------|
| `src/main.rs` | 入口 | 程序启动、路由配置 |
| `src/db.rs` | 核心 | SQLite 数据库管理 |
| `src/jwt.rs` | 核心 | JWT 认证 |
| `src/token.rs` | 功能 | count_tokens 处理 |
| `src/http_client.rs` | 工具 | HTTP 客户端构建 |
| `src/model/` | 核心 | 配置和参数模型 |
| `src/model/config.rs` | 核心 | 启动配置（.env） |
| `src/model/runtime_config.rs` | 核心 | 运行时配置（数据库） |
| `src/model/arg.rs` | 核心 | 命令行参数 |
| `src/anthropic/` | 核心 | Anthropic API 兼容层 |
| `src/kiro/` | 核心 | Kiro API 客户端实现 |
| `src/admin/` | 功能 | Admin API 管理接口 |
| `src/common/` | 工具 | 通用工具函数 |
| `Cargo.toml` | 配置 | Rust 项目配置 |
| `.env.example` | 配置 | 环境变量示例 |
| `Dockerfile` | 部署 | Docker 构建配置 |

## Admin API 端点

### 认证
- `POST /api/admin/login` - 登录
- `POST /api/admin/logout` - 登出
- `GET /api/admin/me` - 获取当前用户
- `POST /api/admin/change-password` - 修改密码

### 凭据管理
- `GET /api/admin/credentials` - 获取所有凭据
- `POST /api/admin/credentials` - 添加凭据
- `PUT /api/admin/credentials/:id` - 更新凭据
- `DELETE /api/admin/credentials/:id` - 删除凭据
- `POST /api/admin/credentials/:id/disabled` - 禁用/启用
- `POST /api/admin/credentials/:id/priority` - 设置优先级
- `POST /api/admin/credentials/:id/reset` - 重置失败计数
- `POST /api/admin/credentials/:id/use` - 使用此账号
- `GET /api/admin/credentials/:id/balance` - 获取余额
- `POST /api/admin/credentials/refresh` - 刷新所有余额

### 导入导出
- `POST /api/admin/credentials/export` - 导出凭据
- `POST /api/admin/credentials/import` - 批量导入凭据

### 系统设置
- `GET /api/admin/settings` - 获取设置
- `POST /api/admin/settings` - 更新设置

## 快速开始

```bash
# 复制配置文件
cp .env.example .env

# 构建
cargo build --release

# 运行
./target/release/kiro-rs
```

## 配置说明

### 启动配置（.env）

| 变量 | 必填 | 默认值 | 说明 |
|------|------|--------|------|
| `HOST` | 否 | `127.0.0.1` | 监听地址 |
| `PORT` | 否 | `8080` | 监听端口 |
| `API_KEY` | 是 | - | API 密钥 |
| `ADMIN_USERNAME` | 否 | `admin` | 初始管理员用户名 |
| `ADMIN_PASSWORD` | 否 | `admin123` | 初始管理员密码 |
| `DB_PATH` | 否 | `data.db` | 数据库路径 |
| `JWT_SECRET` | 否 | 自动生成 | JWT 密钥 |
| `JWT_EXPIRY_HOURS` | 否 | `24` | JWT 过期时间（小时） |

### 运行时配置（数据库）

以下配置存储在数据库中，通过 Admin 后台管理：

| 键 | 默认值 | 说明 |
|----|--------|------|
| `kiro_version` | `0.8.0` | Kiro 版本号 |
| `system_version` | `darwin#24.6.0` | 系统版本 |
| `node_version` | `v22.12.0` | Node.js 版本 |
| `proxy_url` | - | HTTP 代理地址 |
| `count_tokens_api_url` | - | count_tokens API 地址 |
| `count_tokens_api_key` | - | count_tokens API 密钥 |
| `count_tokens_auth_type` | `x-api-key` | count_tokens 认证类型 |
| `min_usage_threshold` | `5` | 最小使用量阈值 |

## 导入导出格式

### 导出格式

```json
[
  {
    "refreshToken": "...",
    "clientId": "",
    "clientSecret": "",
    "region": "us-east-1",
    "proxyUrl": ""
  }
]
```

### 导入格式

支持单个对象或数组：

```json
// 单个凭据
{
  "refreshToken": "...",
  "clientId": "",
  "clientSecret": ""
}

// 多个凭据
[
  { "refreshToken": "token1" },
  { "refreshToken": "token2", "clientId": "...", "clientSecret": "..." }
]
```

字段说明：
- `refreshToken`: 必填
- `clientId` + `clientSecret`: 都有值则为 IdC 模式，否则为 Social 模式
- `region`: 可选，默认 `us-east-1`
- `proxyUrl`: 可选，默认为空
