# Maintenance Notes

这份文档用于记录当前仓库的本地定制内容，避免后续在没有上下文时不知道哪些改动是本地加的、服务如何保活、以及如何同步上游更新。

## 当前定制内容

本仓库当前不是纯上游状态，额外加入了以下本地改动：

- OpenAI 兼容端点：
  - `POST /v1/chat/completions`
  - `POST /v1/responses`
- 运行时监听地址改为 `0.0.0.0:8990`
- 本地部署使用 `systemd` 托管，服务名为 `kiro-rs`
- 提供一键重建并重启脚本：`scripts/rebuild-and-restart.sh`

核心代码位置：

- `src/openai/`
- `src/anthropic/router.rs`
- `src/anthropic/mod.rs`
- `src/main.rs`

## 重要行为说明

### 1. OpenAI 兼容层是本地定制

上游项目原始定位是 Anthropic/Claude 兼容代理。
当前仓库额外实现了 OpenAI 协议兼容层，但模型本身仍然走 Kiro/Claude 体系，不是 OpenAI 官方模型服务。

### 2. `/v1/models` 是静态模型表

当前 `GET /v1/models` 返回的是静态列表，不保证每个模型都对当前凭据实际可用。
如果调用某些模型时报：

```text
INVALID_MODEL_ID
```

通常不是服务挂了，而是当前凭据不具备该模型权限。

已在当前环境实际跑通过的模型包括：

- `claude-sonnet-4-5-20250929`
- `claude-haiku-4-5-20251001`

### 3. 配置文件不进 Git

以下文件是本地文件，默认被 `.gitignore` 忽略：

- `config.json`
- `credentials.json`
- `kiro.log`

这意味着：

- `git pull` 不会自动覆盖它们
- 但你需要自己备份它们

## 服务保活

当前服务通过 `systemd` 保活，不要再依赖临时终端、`nohup` 或手动常驻 shell。

服务文件：

- 仓库模板：`deploy/kiro-rs.service`
- 已安装路径：`/etc/systemd/system/kiro-rs.service`

常用命令：

```bash
sudo systemctl status kiro-rs
sudo systemctl restart kiro-rs
sudo systemctl stop kiro-rs
sudo journalctl -u kiro-rs -n 100 --no-pager
tail -f /home/ubuntu/kiro-rs/kiro.log
```

服务启动命令使用 release 二进制：

```bash
/home/ubuntu/kiro-rs/target/release/kiro-rs \
  --config /home/ubuntu/kiro-rs/config.json \
  --credentials /home/ubuntu/kiro-rs/credentials.json
```

服务配置里启用了：

- `Restart=always`
- `RestartSec=3`

这表示进程退出后会自动拉起。

## 重建与发布

代码更新后，使用下面的脚本重建并重启服务：

```bash
./scripts/rebuild-and-restart.sh
```

这个脚本会执行：

1. `cargo build --release`
2. `sudo systemctl restart kiro-rs`
3. 输出 `kiro-rs` 当前状态

## Git 分支与上游同步

不要在 `master` 上直接做本地长期改动。
当前本地定制分支是：

```text
openai-compat
```

建议长期在这个分支上维护所有本地定制。

### 推荐同步流程

```bash
cd /home/ubuntu/kiro-rs
git switch openai-compat
git fetch origin
git merge origin/master
./scripts/rebuild-and-restart.sh
```

如果 merge 过程中有冲突，优先检查以下文件是否被上游改动：

- `src/anthropic/router.rs`
- `src/main.rs`
- `README.md`
- `src/openai/`

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
sudo systemctl status kiro-rs
ss -ltnp | rg 8990
tail -n 100 /home/ubuntu/kiro-rs/kiro.log
```

判断方式：

- 如果 `systemctl` 显示 `active (running)`，说明服务仍在
- 如果端口 `8990` 没有监听，说明服务没有成功启动
- 如果日志中出现 `INVALID_MODEL_ID`，这是模型权限问题，不是服务崩溃
- 如果日志中出现 `Address already in use`，说明有多个实例抢占同一端口
