# kiro-rs

一个用 Rust 编写的 Anthropic Claude API 兼容代理服务，将 Anthropic API 请求转换为 Kiro API 请求。

## 免责声明

本项目仅供研究使用，Use at your own risk，使用本项目所导致的任何后果由使用人承担，与本项目无关。
本项目与 AWS/KIRO/Anthropic/Claude 等官方无关，本项目不代表官方立场。

## 注意

因 TLS 库从 native-tls 切换至 rustls，你可能需要专门安装证书后才能配置 HTTP PROXY。

## 功能特性

- **Anthropic API 兼容**: 完整支持 Anthropic Claude API 格式
- **流式响应**: 支持 SSE (Server-Sent Events) 流式输出
- **Token 自动刷新**: 自动管理和刷新 OAuth Token
- **多凭据支持**: 支持配置多个凭据，按优先级自动故障转移
- **智能重试**: 单凭据最多重试 3 次，单请求最多重试 9 次
- **Thinking 模式**: 支持 Claude 的 extended thinking 功能
- **工具调用**: 完整支持 function calling / tool use
- **多模型支持**: 支持 Sonnet、Opus、Haiku 系列模型
- **Admin 管理后台**: 提供 Web UI 管理凭据和配置
- **SQLite 存储**: 凭据和配置持久化存储在 SQLite 数据库

## 支持的 API 端点

| 端点 | 方法 | 描述 |
|------|------|------|
| `/v1/models` | GET | 获取可用模型列表 |
| `/v1/messages` | POST | 创建消息（对话） |
| `/v1/messages/count_tokens` | POST | 估算 Token 数量 |
| `/admin` | GET | Admin 管理后台 UI |

## 快速开始

### 方式一：使用构建脚本（推荐）

```bash
# 完整构建（前端 + 后端）
./build.sh all

# 仅构建前端
./build.sh frontend

# 仅构建后端
./build.sh backend

# 清理构建产物
./build.sh clean
```

### 方式二：手动构建

```bash
# 1. 构建前端
cd frontend && npm install && npm run build && cd ..

# 2. 构建后端
cd backend && cargo build --release
```

### 配置文件

复制 `backend/.env.example` 为 `backend/.env` 并修改配置：

```bash
cp backend/.env.example backend/.env
```

**最小配置：**

```env
HOST=127.0.0.1
PORT=8080
API_KEY=sk-kiro-rs-your-secret-key
```

<details>
<summary>完整配置选项</summary>

```env
# 服务器配置
HOST=127.0.0.1
PORT=8080

# API 密钥（必填，用于验证 API 请求）
API_KEY=sk-kiro-rs-your-secret-key

# 默认管理员账号（仅首次初始化时使用）
ADMIN_USERNAME=admin
ADMIN_PASSWORD=admin123

# 数据库路径（可选，默认 data.db）
DB_PATH=data.db

# JWT 配置（可选，不配置则自动生成）
JWT_SECRET=your-jwt-secret-key-at-least-32-characters
JWT_EXPIRY_HOURS=24
```

**注意：** 以下配置项存储在数据库中，通过 Admin 管理后台的「设置」页面进行管理：

- `kiro_version` - Kiro 版本号（默认 0.8.0）
- `system_version` - 系统版本（默认 darwin#24.6.0）
- `node_version` - Node.js 版本（默认 v22.12.0）
- `proxy_url` - HTTP 代理地址
- `count_tokens_api_url` - count_tokens API 地址
- `count_tokens_api_key` - count_tokens API 密钥
- `count_tokens_auth_type` - count_tokens 认证类型（默认 x-api-key）
- `min_usage_threshold` - 最小使用量阈值（默认 5）

</details>

### 凭据管理

凭据存储在 SQLite 数据库中，通过 Admin 管理后台进行管理：

1. 启动服务后访问 `http://127.0.0.1:8080/admin`
2. 使用默认账号登录（admin/admin123，可通过 .env 自定义）
3. 登录后在 Dashboard 页面添加凭据

**支持的凭据类型：**

| 认证方式 | 必填字段 | 说明 |
|----------|----------|------|
| `social` | `refreshToken`, `expiresAt` | Social 登录方式 |
| `idc` | `refreshToken`, `expiresAt`, `clientId`, `clientSecret` | IdC 登录方式 |

### 启动服务

```bash
cd backend && ./target/release/kiro-rs
```

### 使用 API

```bash
curl http://127.0.0.1:8080/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-kiro-rs-your-secret-key" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 1024,
    "messages": [
      {"role": "user", "content": "Hello, Claude!"}
    ]
  }'
```

## 项目结构

```
kiro-rs/
├── backend/                    # Rust 后端服务
│   ├── src/
│   │   ├── main.rs             # 程序入口
│   │   ├── db.rs               # SQLite 数据库
│   │   ├── jwt.rs              # JWT 认证
│   │   ├── token.rs            # count_tokens 处理
│   │   ├── http_client.rs      # HTTP 客户端
│   │   ├── model/              # 配置和参数模型
│   │   │   ├── config.rs       # 启动配置（.env）
│   │   │   ├── runtime_config.rs # 运行时配置（数据库）
│   │   │   └── arg.rs          # 命令行参数
│   │   ├── anthropic/          # Anthropic API 兼容层
│   │   │   ├── handlers.rs     # 请求处理
│   │   │   ├── converter.rs    # 格式转换
│   │   │   ├── stream.rs       # 流式响应
│   │   │   ├── types.rs        # 类型定义
│   │   │   └── websearch.rs    # WebSearch 工具
│   │   ├── kiro/               # Kiro API 客户端
│   │   │   ├── provider.rs     # API 调用
│   │   │   ├── token_manager.rs # Token 管理
│   │   │   ├── machine_id.rs   # 设备指纹
│   │   │   ├── model/          # Kiro 数据模型
│   │   │   └── parser/         # 响应解析
│   │   ├── admin/              # Admin API 管理接口
│   │   │   ├── handlers.rs     # 请求处理
│   │   │   ├── router.rs       # 路由定义
│   │   │   ├── service.rs      # 业务逻辑
│   │   │   ├── middleware.rs   # 认证中间件
│   │   │   └── ui.rs           # 前端静态资源
│   │   └── common/             # 公共模块
│   ├── Cargo.toml              # Rust 项目配置
│   ├── .env.example            # 环境变量示例
│   └── Dockerfile              # Docker 构建配置
├── frontend/                   # React 前端 (Admin UI)
│   ├── src/
│   │   ├── pages/              # 页面组件
│   │   │   ├── Dashboard.tsx   # 凭据管理
│   │   │   ├── Settings.tsx    # 系统设置
│   │   │   └── Login.tsx       # 登录页面
│   │   ├── components/         # UI 组件
│   │   └── api/                # API 服务层
│   ├── package.json            # npm 配置
│   └── vite.config.ts          # Vite 配置
├── .github/workflows/          # GitHub Actions
├── build.sh                    # 构建脚本
└── README.md                   # 项目文档
```

## 技术栈

**后端：**
- [Axum](https://github.com/tokio-rs/axum) 0.8 - Web 框架
- [Tokio](https://tokio.rs/) - 异步运行时
- [Reqwest](https://github.com/seanmonstar/reqwest) - HTTP 客户端
- [SQLite](https://www.sqlite.org/) - 数据库（凭据和配置存储）
- [JWT](https://jwt.io/) - 认证

**前端：**
- [React](https://react.dev/) 19 - UI 框架
- [Vite](https://vitejs.dev/) - 构建工具
- [Tailwind CSS](https://tailwindcss.com/) 4 - CSS 框架
- [Shadcn UI](https://ui.shadcn.com/) - 组件库

## 模型映射

| Anthropic 模型 | Kiro 模型 |
|----------------|-----------|
| `*sonnet*` | `claude-sonnet-4.5` |
| `*opus*` | `claude-opus-4.5` |
| `*haiku*` | `claude-haiku-4.5` |

## 高级功能

### Thinking 模式

```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 16000,
  "thinking": {
    "type": "enabled",
    "budget_tokens": 10000
  },
  "messages": [...]
}
```

### 工具调用

```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 1024,
  "tools": [
    {
      "name": "get_weather",
      "description": "获取指定城市的天气",
      "input_schema": {
        "type": "object",
        "properties": {
          "city": {"type": "string"}
        },
        "required": ["city"]
      }
    }
  ],
  "messages": [...]
}
```

### 流式响应

```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 1024,
  "stream": true,
  "messages": [...]
}
```

## 认证方式

支持两种 API Key 认证方式：

```
x-api-key: sk-your-api-key
```

或

```
Authorization: Bearer sk-your-api-key
```

## Docker 部署

```bash
# 构建镜像
docker build -t kiro-rs -f backend/Dockerfile .

# 运行容器
docker run -d -p 8080:8080 \
  -e API_KEY=sk-kiro-rs-your-secret-key \
  -v /path/to/data.db:/app/data.db \
  kiro-rs
```

## 环境变量

```bash
RUST_LOG=debug ./target/release/kiro-rs
```

## 注意事项

1. **数据安全**: 请妥善保管 `data.db` 数据库文件和 `.env` 配置文件
2. **Token 刷新**: 服务会自动刷新过期的 Token，无需手动干预
3. **WebSearch 工具**: 支持 Anthropic 的 `web_search` 工具，会自动转换为 Kiro MCP 调用

## License

MIT

## 致谢

本项目的实现离不开前辈的努力：
- [kiro.rs](https://github.com/hank9999/kiro.rs) - 原始项目
- [kiro2api](https://github.com/caidaoli/kiro2api)
- [proxycast](https://github.com/aiclientproxy/proxycast)

本项目部分逻辑参考了以上的项目，再次由衷的感谢！
