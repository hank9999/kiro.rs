#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
container="${KIRO_CONTAINER:-kiro-rs}"
image="${KIRO_IMAGE:-kiro-rs:local}"
network="${KIRO_NETWORK:-kiro-proxy-net}"
config_dir="${KIRO_CONFIG_DIR:-$repo_root/config}"
host_port="${KIRO_HOST_PORT:-8990}"
container_port="${KIRO_CONTAINER_PORT:-8990}"
startup_timeout="${KIRO_STARTUP_TIMEOUT:-60}"

cd "$repo_root"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required but was not found in PATH" >&2
  exit 1
fi

if [[ ! -d "$config_dir" ]]; then
  echo "config directory not found: $config_dir" >&2
  exit 1
fi

if [[ -n "$network" ]] && ! docker network inspect "$network" >/dev/null 2>&1; then
  echo "docker network not found, creating: $network"
  docker network create "$network" >/dev/null
fi

echo "Building image: $image"
docker build -t "$image" "$repo_root"

if docker container inspect "$container" >/dev/null 2>&1; then
  echo "Removing existing container: $container"
  docker rm -f "$container" >/dev/null
fi

run_args=(
  docker run -d
  --name "$container"
  --restart unless-stopped
  -p "${host_port}:${container_port}"
  -v "${config_dir}:/app/config"
)

if [[ -n "$network" ]]; then
  run_args+=(--network "$network")
fi

run_args+=("$image")

echo "Starting container: $container"
container_id="$("${run_args[@]}")"
echo "Container ID: $container_id"

deadline=$((SECONDS + startup_timeout))
while (( SECONDS < deadline )); do
  if curl -sS -o /dev/null "http://127.0.0.1:${host_port}/"; then
    echo "Service is responding on http://127.0.0.1:${host_port}"
    docker ps --filter "name=^/${container}$" --format 'table {{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}'
    docker logs --tail 20 "$container"
    exit 0
  fi

  sleep 2
done

echo "Container started but did not become reachable within ${startup_timeout}s" >&2
docker ps -a --filter "name=^/${container}$" --format 'table {{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}' >&2
docker logs --tail 100 "$container" >&2
exit 1
