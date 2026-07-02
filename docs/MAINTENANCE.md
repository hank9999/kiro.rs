# Maintenance Notes

这份文档用于记录当前仓库的本地定制内容，避免后续在没有上下文时不知道哪些改动是本地加的、服务如何保活、以及如何同步上游更新。

## 当前定制内容

本仓库当前不是纯上游状态，额外加入了以下本地改动：

- OpenAI 兼容端点：
  - `POST /v1/chat/completions`
  - `POST /v1/responses`
- Admin 监控能力：
  - Admin 页面实时调用记录
  - Admin 页面成功/失败统计
  - Admin 页面日志 tail 展示
  - Admin API:
    - `GET /api/admin/activity`
    - `GET /api/admin/logs`
- 运行时监听地址改为 `0.0.0.0:8990`
- 本地部署使用 Docker 容器，容器名为 `kiro-rs`
- 提供一键重建并重启脚本：`scripts/rebuild-and-restart.sh`
- 本机额外接入 `GoProxy` 免费优选代理池，Kiro 出站统一走本地稳定代理

核心代码位置：

- `src/openai/`
- `src/monitoring.rs`
- `src/anthropic/router.rs`
- `src/anthropic/middleware.rs`
- `src/anthropic/mod.rs`
- `src/main.rs`
- `src/admin/service.rs`
- `src/admin/handlers.rs`
- `src/admin/router.rs`
- `admin-ui/src/components/activity-monitor.tsx`
- `admin-ui/src/components/dashboard.tsx`

## 2026-04-05 代理接入记录

当前这台机器的实际运行方式与上面的旧说明不同，现网是 Docker 容器在跑，不是 `systemd` 直接托管二进制。

新增的代理链路如下：

- `goproxy` 容器：`ghcr.io/isboyjc/goproxy:latest`
- Docker 网络：`kiro-proxy-net`
- `kiro-rs` 全局代理：`http://goproxy:7777`
- GoProxy 管理面板：`http://127.0.0.1:7778`
- GoProxy 本地数据目录：`goproxy-data/`
- 一键部署脚本：`scripts/deploy-proxy-stack.sh`
- 就绪等待脚本：`scripts/wait-for-goproxy.sh`

说明：

- 这里目前选的是 GoProxy 的随机 HTTP 端口 `7777`。
- 但不是无脑随机，而是随机轮换优先，同时只在高质量代理子集里轮换，避免把长尾低质量 IP 一起抽进去。
- `8990` 仍然是对外服务入口，客户端调用方式不变。

## 重要行为说明

### 1. OpenAI 兼容层是本地定制

上游项目原始定位是 Anthropic/Claude 兼容代理。
当前仓库额外实现了 OpenAI 协议兼容层，但模型本身仍然走 Kiro/Claude 体系，不是 OpenAI 官方模型服务。

### 2. `/v1/models` 来自上游动态拉取

当前 `GET /v1/models` 不再是静态表，而是后台周期任务从上游 `ListAvailableModels` 拉取真实可用模型后缓存的结果。

刷新机制（详见 `src/main.rs` 中的 `dynamic_models` 任务）：

- 服务启动后约 `dynamicModels.initialDelaySecs` 秒（默认 5s）发起首次拉取
- 之后每 `dynamicModels.refreshIntervalSecs` 秒（默认 1800s = 30 分钟）刷新一次
- 单次失败采用指数退避（30s → 60s → 120s → … 最多 300s），不会高频重试，避免触发上游限流
- 拉取成功后写入 `ModelsCacheHandle`，`/v1/models` 与 `/api/admin/models` 都直接读这份共享缓存
- 通过单条 active 凭据请求一次（实际观测多个 KIRO FREE 凭据返回相同列表），不在每个凭据上重复扫描

`config.json` 相关字段（缺省即可，全部带默认值）：

```json
{
  "dynamicModels": {
    "enabled": true,
    "refreshIntervalSecs": 1800,
    "initialDelaySecs": 5
  }
}
```

特殊情况：

- 启动后首次拉取尚未成功 / `dynamicModels.enabled=false` / 上游连续失败时，会暂时回退到硬编码的 `fallback_models()` 列表（参见 `src/anthropic/handlers.rs`）
- fallback 列表故意保留 `-thinking` 后缀变体，方便客户端在动态列表尚未就绪时继续走老 ID
- 上游真实列表中 ID 是无 `-thinking` 后缀的形式（例如 `claude-sonnet-4.5`）。客户端在 ID 末尾追加 `-thinking` 仍可触发思考模式，由 `handlers` 层 `override_thinking_from_model_name` 处理
- **`refresh_models` 在写入缓存前只放行 `claude-*` 系列**：handlers 的 `map_model` 当前只识别 `sonnet` / `opus` / `haiku` 关键字，若把 `auto` / `deepseek-*` / `minimax-*` / `glm-*` / `qwen*` 等模型也暴露给客户端，调用时会拿到 `400 invalid_request_error: 模型不支持`。如果未来 `map_model` 扩展，请同步放宽 `is_supported_upstream_model` 的过滤
- **空数组保护**：上游返回空 / 全部被过滤掉 / 上游临时故障时，`refresh_models` 不替换缓存，仅记录 `last_error` 并返回 `Err`，让现有缓存继续生效（fallback 或上一次成功结果）

如果调用某些模型时报：

```text
INVALID_MODEL_ID
```

通常不是服务挂了，而是当前凭据不具备该模型权限。已在当前环境实际跑通的模型至少包括：

- `claude-sonnet-4-5-20250929`
- `claude-haiku-4-5-20251001`

校验当前缓存状态：

```bash
# 默认从 config.json 读取 host/port + 第一个有效 API Key
python scripts/test_dynamic_models_refresh.py

# 监听模式：每 5 秒轮询一次，观察 fallback → dynamic 切换
python scripts/test_dynamic_models_refresh.py --watch 5

# 同时校验 admin 接口与 v1 接口共享同一份缓存
python scripts/test_dynamic_models_refresh.py --admin-key <ADMIN_API_KEY>
```

附属诊断脚本：

- `scripts/test_list_models.py`：直接打上游 `ListAvailableModels`，验证凭据 / 代理是否能正常拿到模型列表
- `scripts/diag_compare_models.py`：批量调用多条凭据并对比返回，观察是否需要切换为按凭据各自缓存（当前免费档全部一致，故只用单凭据策略）

### 3. 配置文件不进 Git

以下文件是本地文件，默认被 `.gitignore` 忽略：

- `config.json`
- `credentials.json`
- `kiro.log`

这意味着：

- `git pull` 不会自动覆盖它们
- 但你需要自己备份它们

### 4. Admin 页面带运行监控

当前本地管理页面不只是凭据管理，还额外展示：

- 最近请求活动
- 成功/失败统计
- 最近日志 tail

页面入口：

```text
/admin
```

对应后端接口：

- `GET /api/admin/activity`
- `GET /api/admin/logs`

这部分是本地定制，不是上游默认能力。

### 5. 请求活动记录的含义

当前请求活动面板记录的是 API 网关层的实际访问结果，至少包含：

- method
- path
- status code
- success / failed
- duration
- started / finished 时间

注意：

- 这里的失败统计表示接口层最终返回了非 2xx/3xx
- 如果日志里是 `INVALID_MODEL_ID`，通常是模型权限问题，不代表服务本身崩溃
- 当前活动记录已经足够定位“有没有请求进来”“返回码是什么”“最近是否连续失败”

## 服务保活

当前服务通过 Docker 容器保活，不要再依赖临时终端、`nohup`、手动 `docker cp` 二进制，或旧的 `systemd` 直跑方式。

当前容器默认配置：

- 镜像：`kiro-rs:local`
- 容器名：`kiro-rs`
- 网络：`kiro-proxy-net`
- 端口映射：`8990:8990`
- 配置挂载：`./config -> /app/config`
- 重启策略：`unless-stopped`

常用命令：

```bash
docker ps --filter name=kiro-rs
docker restart kiro-rs
docker stop kiro-rs
docker logs --tail 100 kiro-rs
docker inspect kiro-rs
```

## 重建与发布

代码更新后，使用下面的脚本重建并重启服务：

```bash
./scripts/rebuild-and-restart.sh
```

这个脚本会执行：

1. `docker build -t kiro-rs:local .`
2. 删除旧的 `kiro-rs` 容器
3. 用默认的端口、挂载和网络重新启动容器
4. 等待本地 `8990` 端口可访问
5. 输出容器状态和最近日志

注意：

- 不要再执行 `cargo build --release` 然后 `docker cp` 到 Alpine 容器，这会把宿主机的 GNU/glibc 二进制塞进容器，导致启动时报 `exec ./kiro-rs: no such file or directory`
- 如果改了 Admin 前端，不需要单独手工构建，`Dockerfile` 在镜像构建阶段会自动执行前端构建

可选环境变量：

- `KIRO_IMAGE`
- `KIRO_CONTAINER`
- `KIRO_NETWORK`
- `KIRO_CONFIG_DIR`
- `KIRO_HOST_PORT`
- `KIRO_CONTAINER_PORT`
- `KIRO_STARTUP_TIMEOUT`

## Git 分支与上游同步

不要在 `master` 上直接做本地长期改动。
当前本地定制分支是：

```text
openai-compat
```

建议长期在这个分支上维护所有本地定制。

当前需要长期保留的本地定制至少包括：

- OpenAI 兼容层
- Docker 部署与保活
- Admin 监控页面与相关 API

### 推荐同步流程

```bash
cd /path/to/kiro.rs
git switch openai-compat
git status
git add .
git commit -m "Save local custom changes"
git fetch origin
git merge origin/master
./scripts/rebuild-and-restart.sh
```

如果 `git status` 不是干净的，不要直接 merge 上游，先把本地改动提交。
否则这类尚未提交的本地定制最容易在后续操作中丢失。

如果 merge 过程中有冲突，优先检查以下文件是否被上游改动：

- `src/anthropic/router.rs`
- `src/anthropic/middleware.rs`
- `src/main.rs`
- `src/monitoring.rs`
- `src/admin/service.rs`
- `src/admin/handlers.rs`
- `src/admin/router.rs`
- `README.md`
- `docs/MAINTENANCE.md`
- `src/openai/`
- `admin-ui/src/components/activity-monitor.tsx`
- `admin-ui/src/components/dashboard.tsx`

### 为什么这样做

因为本地定制内容已经提交在 `openai-compat` 分支里。
后续拉取上游时，只要继续把 `origin/master` 合并进 `openai-compat`，本地改动就不会凭空丢失。

## 建议的远端备份方式

更稳妥的做法是把当前仓库 fork 到你自己的 GitHub，并把 `openai-compat` 推到你自己的远端。
这样即使机器损坏或目录丢失，本地定制也能恢复。

示例：

```bash
git remote add myfork <your-fork-url>
git push -u myfork openai-compat
```

## 故障排查

如果你怀疑服务挂了，先按这个顺序检查：

```bash
docker ps -a --filter name=kiro-rs
docker logs --tail 100 kiro-rs
ss -ltnp | rg 8990
curl -i http://127.0.0.1:8990/
```

判断方式：

- 如果 `docker ps` 显示 `Up`，说明容器还活着
- 如果端口 `8990` 没有监听，说明服务没有成功启动
- 如果日志里出现 `exec ./kiro-rs: no such file or directory`，优先怀疑是把宿主机二进制错误地拷进了 Alpine 容器
- 如果日志中出现 `INVALID_MODEL_ID`，这是模型权限问题，不是服务崩溃
- 如果日志中出现 `Address already in use`，说明有多个实例抢占同一端口

### Admin 监控自检

如果要确认 Admin 监控功能还在，可以直接验证：

```bash
curl -sS http://127.0.0.1:8990/api/admin/activity \
  -H 'x-api-key: <admin-api-key>'

curl -sS http://127.0.0.1:8990/api/admin/logs?lines=20 \
  -H 'x-api-key: <admin-api-key>'
```

预期结果：

- `/activity` 返回 `summary` 和 `records`
- `/logs` 返回 `path`、`lines`、`available`

如果这两个接口不存在，说明当前运行的不是带本地监控定制的版本。
