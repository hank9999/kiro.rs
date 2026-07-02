#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

network_name="${GOPROXY_NETWORK:-kiro-proxy-net}"
kiro_container="${KIRO_CONTAINER:-kiro-rs}"
goproxy_dir="${GOPROXY_DIR:-/root/GoProxy}"
goproxy_image="${GOPROXY_IMAGE:-goproxy-local:kiro}"
goproxy_data_dir="${GOPROXY_DATA_DIR:-$PWD/goproxy-data}"

if ! docker network inspect "$network_name" >/dev/null 2>&1; then
  docker network create "$network_name" >/dev/null
fi

python3 - "$goproxy_data_dir" <<'PY'
import json
import os
import sqlite3
import sys

data_dir = sys.argv[1]
cfg_path = os.path.join(data_dir, "config.json")
db_path = os.path.join(data_dir, "proxy.db")

try:
    with open(cfg_path, "r", encoding="utf-8") as f:
        cfg = json.load(f)
except FileNotFoundError:
    raise SystemExit(0)

if not os.path.exists(db_path):
    raise SystemExit(0)

pool_http_ratio = cfg.get("pool_http_ratio", 0.3)
pool_min_per_protocol = cfg.get("pool_min_per_protocol", 10)

if pool_http_ratio >= 1 and pool_min_per_protocol == 0:
    conn = sqlite3.connect(db_path)
    try:
        conn.execute("DELETE FROM proxies WHERE protocol = 'socks5'")
        conn.commit()
    finally:
        conn.close()
PY

docker build -t "$goproxy_image" "$goproxy_dir" >/dev/null
docker rm -f goproxy >/dev/null 2>&1 || true

docker run -d \
  --name goproxy \
  --network "$network_name" \
  --restart unless-stopped \
  --health-cmd='curl -sf http://localhost:7778/ >/dev/null || exit 1' \
  --health-interval=30s \
  --health-timeout=5s \
  --health-retries=3 \
  -p 127.0.0.1:7777:7777 \
  -p 127.0.0.1:7776:7776 \
  -p 127.0.0.1:7778:7778 \
  -p 127.0.0.1:7780:7780 \
  -v "$goproxy_data_dir:/app/data" \
  -e TZ=Etc/UTC \
  -e DATA_DIR=/app/data \
  -e WEBUI_PASSWORD=goproxy-kiro-20260405 \
  -e PROXY_AUTH_ENABLED=false \
  -e BLOCKED_COUNTRIES=CN,RU \
  -e CUSTOM_PROXY_MODE=free_only \
  "$goproxy_image" >/dev/null

docker network connect "$network_name" "$kiro_container" >/dev/null 2>&1 || true
./scripts/wait-for-goproxy.sh
docker restart "$kiro_container" >/dev/null
docker ps --format 'table {{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}'
