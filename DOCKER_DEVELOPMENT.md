# Docker 热重载开发

开发模式不会修改现有生产镜像和 `docker-compose.yml` 的运行方式。

## 启动

确保 `config/` 下已有 `config.json` 和 `credentials.json`，然后运行：

```bash
docker compose -f docker-compose.yml -f docker-compose.dev.yml up --build
```

- Rust API：`http://localhost:18990`
- Admin UI（Vite HMR）：`http://localhost:18991/admin/`

如果宿主机代理监听在 `localhost:7890`，构建容器时需要改用 Docker 可访问的宿主机地址：

```powershell
docker compose -f docker-compose.yml -f docker-compose.dev.yml build `
  --build-arg HTTP_PROXY=http://host.docker.internal:7890 `
  --build-arg HTTPS_PROXY=http://host.docker.internal:7890
docker compose -f docker-compose.yml -f docker-compose.dev.yml up
```

修改 `src/`、`Cargo.toml` 或 `Cargo.lock` 后，`cargo-watch` 会自动重新编译并重启 API。修改 `admin-ui/` 下的前端代码后，Vite 会自动触发浏览器热更新。

## 停止

```bash
docker compose -f docker-compose.yml -f docker-compose.dev.yml down
```

需要清理开发依赖和 Rust 编译缓存时：

```bash
docker compose -f docker-compose.yml -f docker-compose.dev.yml down -v
```
